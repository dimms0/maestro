use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use crate::{audio_params::AudioParameters, realtime::host_time};

const NANOS_PER_SEC: u64 = 1_000_000_000;

/// Slack kept between the event timeline and the renderer, in render blocks.
/// Events are stamped this far ahead of the block the renderer is working on,
/// so they land in audio that still has to be produced and can be placed at
/// their exact sample position.
const TARGET_LEAD_BLOCKS: u64 = 2;

/// How far past the committed block a timestamped event may be scheduled.
pub const MAX_SCHEDULE_SECS: u64 = 2;

/// How often the timeline is measured against the renderer's progress.
const SYNC_WINDOW_NS: u64 = 500_000_000;

/// Clock used to timestamp events during realtime playback.
///
/// Every event is stamped from a host clock time: the one it was sent with, or
/// its arrival when `auto_timestamp` is on. A time of 0 means neither, and the
/// event is unstamped and plays at the start of the next block.
///
/// Events have to keep the exact spacing the sender gave them, so the mapping
/// from wall time to sample positions is a plain affine one:
///
/// ```text
/// position = offset + elapsed * sample_rate
/// ```
///
/// Only `offset` ever moves, which means the distance between two events is
/// always the wall-clock distance between them, down to the sample.
///
/// What `offset` has to account for is the renderer. The buffered renderer does
/// not produce audio at a steady pace: it renders a burst of blocks whenever
/// the audio device drains the queue, then idles until the next callback, so
/// its position is a staircase whose steps are as large as the device's buffer.
/// Events can only be placed in blocks that have not been rendered yet, so the
/// timeline has to stay above the *peaks* of that staircase. `offset` is
/// therefore corrected from the closest the timeline came to the renderer over
/// a window of blocks, which filters the burstiness out, and only by the amount
/// that drift between the audio device and its nominal sample rate calls for.
/// Anything sudden — the first block, a render stall, a paused stream — is a
/// desync and snaps the timeline back to the renderer in one go.
///
/// A correction can move the timeline backwards, and two events that cross one
/// would come out in the wrong order: the synths place events by the position
/// they carry, so a note off landing before its note on leaves the note playing
/// forever. `handed_out` latches the stamps to the furthest one taken, which
/// costs a stamp one read-modify-write and holds the timeline still for as long
/// as the correction reaches back, instead of letting it rewind. Corrections
/// stay within the lookahead, so events scheduled past it skip the latch: the
/// ports order them anyway, and latching them would drag every event sent
/// after them out to where they were scheduled.
///
/// Everything else the render thread keeps for itself, and no state is
/// published alongside these values, so nothing here needs an ordering beyond
/// `Relaxed`.
pub(crate) struct RealtimeClock {
    /// Frame offset between wall time and the playback timeline, and the only
    /// part of the mapping that ever moves.
    offset: AtomicI64,
    /// `committed` plus the lookahead limit, past which stamps skip the
    /// latch. Kept ready-made rather than added up on every event.
    lookahead_end: AtomicU64,

    /// Frames the renderer has committed to producing, i.e. where the block
    /// after the one being rendered starts. Events stamped while a block
    /// renders are consumed by the block after it, so this is what they are
    /// measured against.
    committed: AtomicU64,
    /// How far ahead of `committed` the timeline may run. Tells a desync from
    /// the render thread's normal swing.
    max_lookahead: AtomicU64,

    unstamped: AtomicU32,
    auto_timestamp: AtomicBool,

    /// Frames between the device being handed audio and playing it.
    output_delay: AtomicU64,
    /// Frames from an event stamped now until it is heard.
    latency: AtomicU64,

    /// The furthest position any event has been stamped at. No stamp may ever
    /// land before it, whatever the corrections do to the timeline.
    handed_out: AtomicU64,

    /// Closest and furthest the timeline came to the renderer in the current
    /// window, and the frame the window runs out at.
    window_min: AtomicI64,
    window_max: AtomicI64,
    window_end: AtomicU64,

    /// Host clock time the timeline starts at
    start: u64,
    sample_rate: u64,
    channels: u64,
    /// [`SYNC_WINDOW_NS`] in frames, so that measuring the window costs nothing
    /// more than the elapsed frames every block needs anyway.
    sync_window: u64,
    /// Floor the lookahead limit never drops below, a tenth of a second.
    min_lookahead: u64,
    /// [`MAX_SCHEDULE_SECS`] in frames.
    max_schedule: u64,
}

impl RealtimeClock {
    pub fn new(audio_params: &AudioParameters, auto_timestamp: bool) -> Self {
        let sample_rate = audio_params.sample_rate as u64;
        let min_lookahead = sample_rate / 10;
        let sync_window = sample_rate * SYNC_WINDOW_NS / NANOS_PER_SEC;

        Self {
            offset: AtomicI64::new(0),
            lookahead_end: AtomicU64::new(min_lookahead),
            committed: AtomicU64::new(0),
            max_lookahead: AtomicU64::new(min_lookahead),
            unstamped: AtomicU32::new(0),
            auto_timestamp: AtomicBool::new(auto_timestamp),
            output_delay: AtomicU64::new(0),
            latency: AtomicU64::new(0),
            handed_out: AtomicU64::new(0),
            window_min: AtomicI64::new(i64::MAX),
            window_max: AtomicI64::new(i64::MIN),
            window_end: AtomicU64::new(sync_window),
            start: host_time::host_now(),
            sample_rate,
            channels: u16::from(audio_params.channels) as u64,
            sync_window,
            min_lookahead,
            max_schedule: sample_rate * MAX_SCHEDULE_SECS,
        }
    }

    fn frames(&self, at: u64) -> u64 {
        let elapsed = at.saturating_sub(self.start);
        let per_sec = host_time::ticks_per_sec();

        // Splitting the elapsed ticks at the second keeps this to 64-bit
        // arithmetic: the whole seconds scale exactly, and the ticks left over
        // are far too few to overflow when scaled. The result is the same
        // `elapsed * sample_rate / ticks_per_sec` down to the frame.

        elapsed / per_sec * self.sample_rate + elapsed % per_sec * self.sample_rate / per_sec
    }

    fn timeline(&self, elapsed: u64) -> i64 {
        self.offset.load(Ordering::Relaxed) + elapsed as i64
    }

    fn clear_measurements(&self) {
        self.window_min.store(i64::MAX, Ordering::Relaxed);
        self.window_max.store(i64::MIN, Ordering::Relaxed);
    }

    /// `queued` is how many samples are rendered but not yet handed to the device.
    pub fn begin_block(&self, samples: u64, queued: u64) {
        let frames = samples / self.channels;
        let now = self.frames(host_time::host_now());

        let lookahead = self.max_lookahead.load(Ordering::Relaxed);
        let committed = self.committed.load(Ordering::Relaxed) + frames;
        self.committed.store(committed, Ordering::Relaxed);
        self.lookahead_end
            .store(committed + lookahead, Ordering::Relaxed);
        self.unstamped.store(
            (committed.saturating_sub(self.sample_rate) * self.channels) as u32,
            Ordering::Relaxed,
        );

        let offset = self.offset.load(Ordering::Relaxed);
        let target = (TARGET_LEAD_BLOCKS * frames) as i64;
        let lead = offset + now as i64 - committed as i64;

        let desynced = lead < 0 || lead > lookahead as i64;

        // An event stamped now lands `lead` past this block, which plays once
        // everything queued ahead of it has gone through the device.
        let lead_after = if desynced { target } else { lead };
        let ahead = queued / self.channels + frames + self.output_delay.load(Ordering::Relaxed);
        self.latency
            .store(ahead + lead_after as u64, Ordering::Relaxed);

        if desynced {
            // Either the renderer caught up with the timeline, so events would
            // land in audio that is already rendered, or it ran away from the
            // renderer while nothing was being produced (a stall, a paused
            // stream, the very first block). Neither can wait for the window:
            // correct the whole error at once and drop the measurements, which
            // were taken against the old offset.
            self.offset.store(offset + target - lead, Ordering::Relaxed);
            self.clear_measurements();
            return;
        }

        let window_min = self.window_min.load(Ordering::Relaxed).min(lead);
        let window_max = self.window_max.load(Ordering::Relaxed).max(lead);

        if now < self.window_end.load(Ordering::Relaxed) {
            self.window_min.store(window_min, Ordering::Relaxed);
            self.window_max.store(window_max, Ordering::Relaxed);
            return;
        }

        // The closest the timeline came to the renderer over the window is the
        // trough of its staircase, with the burstiness filtered out. Nudging
        // that onto the target absorbs the drift between the audio device and
        // its nominal rate: a handful of frames per window, far too little to
        // disturb the spacing of the events.
        self.offset
            .store(offset + target - window_min, Ordering::Relaxed);

        // How far the renderer's pacing moved during the window that just
        // ended. Leaving twice that much room on top of a tenth of a second
        // keeps the lookahead limit clear of a healthy render thread.
        let swing = (window_max - window_min).max(0) as u64;
        let lookahead = (2 * swing + target as u64).max(self.min_lookahead);

        self.max_lookahead.store(lookahead, Ordering::Relaxed);
        self.lookahead_end
            .store(committed + lookahead, Ordering::Relaxed);
        self.clear_measurements();
        self.window_end
            .store(now + self.sync_window, Ordering::Relaxed);
    }

    pub fn set_output_delay(&self, delay: Duration) {
        let frames = delay.as_nanos() * self.sample_rate as u128 / NANOS_PER_SEC as u128;
        self.output_delay.store(frames as u64, Ordering::Relaxed);
    }

    pub fn latency(&self) -> Duration {
        let frames = self.latency.load(Ordering::Relaxed);
        Duration::from_nanos(frames * NANOS_PER_SEC / self.sample_rate)
    }

    pub fn set_auto_timestamp(&self, auto_timestamp: bool) {
        self.auto_timestamp.store(auto_timestamp, Ordering::Relaxed);
    }

    pub fn stamp(&self, at: u64) -> u32 {
        let at = match at {
            0 if self.auto_timestamp.load(Ordering::Relaxed) => host_time::host_now(),
            0 => return self.unstamped.load(Ordering::Relaxed),
            at => at,
        };

        // A bogus timestamp cannot hold an event back indefinitely
        let committed = self.committed.load(Ordering::Relaxed);
        let frames =
            (self.timeline(self.frames(at)).max(0) as u64).min(committed + self.max_schedule);

        // Stamps landing on an already handed out frame skip the read-modify-write
        let handed_out = self.handed_out.load(Ordering::Relaxed);
        let frames = if frames > self.lookahead_end.load(Ordering::Relaxed) {
            frames
        } else if frames > handed_out {
            frames.max(self.handed_out.fetch_max(frames, Ordering::Relaxed))
        } else {
            handed_out
        };

        (frames * self.channels) as u32
    }
}
