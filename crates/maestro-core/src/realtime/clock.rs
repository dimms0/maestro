use std::{
    sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering},
    time::Instant,
};

use crate::audio_params::AudioParameters;

const NANOS_PER_SEC: u64 = 1_000_000_000;

/// Slack kept between the event timeline and the renderer, in render blocks.
/// Events are stamped this far ahead of the block the renderer is working on,
/// so they land in audio that still has to be produced and can be placed at
/// their exact sample position.
const TARGET_LEAD_BLOCKS: u64 = 2;

/// How far past the committed block a timestamped event may be scheduled.
const MAX_SCHEDULE_SECS: u64 = 2;

/// How often the timeline is measured against the renderer's progress.
const SYNC_WINDOW_NS: u64 = 500_000_000;

/// Clock used to timestamp events during realtime playback.
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
/// as the correction reaches back, instead of letting it rewind.
///
/// Everything else the render thread keeps for itself, and no state is
/// published alongside these values, so nothing here needs an ordering beyond
/// `Relaxed`.
pub(crate) struct RealtimeClock {
    /// Frame offset between wall time and the playback timeline, and the only
    /// part of the mapping that ever moves.
    offset: AtomicI64,
    /// The furthest frame an event may be stamped at, i.e. `committed` plus the
    /// lookahead limit. Kept ready-made rather than added up on every event.
    cap: AtomicU64,

    /// Frames the renderer has committed to producing, i.e. where the block
    /// after the one being rendered starts. Events stamped while a block
    /// renders are consumed by the block after it, so this is what they are
    /// measured against.
    committed: AtomicU64,
    /// How far ahead of `committed` an event may be stamped. Bounds how far
    /// into the future a stalled renderer can push events, and doubles as the
    /// threshold that tells a desync from the render thread's normal swing.
    max_lookahead: AtomicU64,

    unstamped: AtomicU32,

    /// The furthest position any event has been stamped at. No stamp may ever
    /// land before it, whatever the corrections do to the timeline.
    handed_out: AtomicU64,

    /// Closest and furthest the timeline came to the renderer in the current
    /// window, and the frame the window runs out at.
    window_min: AtomicI64,
    window_max: AtomicI64,
    window_end: AtomicU64,

    start: Instant,
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
    pub fn new(audio_params: &AudioParameters) -> Self {
        let sample_rate = audio_params.sample_rate as u64;
        let min_lookahead = sample_rate / 10;
        let sync_window = sample_rate * SYNC_WINDOW_NS / NANOS_PER_SEC;

        Self {
            offset: AtomicI64::new(0),
            cap: AtomicU64::new(min_lookahead),
            committed: AtomicU64::new(0),
            max_lookahead: AtomicU64::new(min_lookahead),
            unstamped: AtomicU32::new(0),
            handed_out: AtomicU64::new(0),
            window_min: AtomicI64::new(i64::MAX),
            window_max: AtomicI64::new(i64::MIN),
            window_end: AtomicU64::new(sync_window),
            start: Instant::now(),
            sample_rate,
            channels: u16::from(audio_params.channels) as u64,
            sync_window,
            min_lookahead,
            max_schedule: sample_rate * MAX_SCHEDULE_SECS,
        }
    }

    fn frames(&self, at: Instant) -> u64 {
        let elapsed = at.saturating_duration_since(self.start);

        // Splitting the elapsed time at the second keeps this to 64-bit
        // arithmetic: the whole seconds scale exactly, and the nanoseconds left
        // over are far too few to overflow when scaled. The result is the same
        // `elapsed_ns * sample_rate / 1e9` down to the frame.

        elapsed.as_secs() * self.sample_rate
            + u64::from(elapsed.subsec_nanos()) * self.sample_rate / NANOS_PER_SEC
    }

    fn timeline(&self, elapsed: u64) -> i64 {
        self.offset.load(Ordering::Relaxed) + elapsed as i64
    }

    fn clear_measurements(&self) {
        self.window_min.store(i64::MAX, Ordering::Relaxed);
        self.window_max.store(i64::MIN, Ordering::Relaxed);
    }

    pub fn begin_block(&self, samples: u64) {
        let frames = samples / self.channels;
        let now = self.frames(Instant::now());

        let lookahead = self.max_lookahead.load(Ordering::Relaxed);
        let committed = self.committed.load(Ordering::Relaxed) + frames;
        self.committed.store(committed, Ordering::Relaxed);
        self.cap.store(committed + lookahead, Ordering::Relaxed);
        self.unstamped.store(
            (committed.saturating_sub(self.sample_rate) * self.channels) as u32,
            Ordering::Relaxed,
        );

        let offset = self.offset.load(Ordering::Relaxed);
        let target = (TARGET_LEAD_BLOCKS * frames) as i64;
        let lead = offset + now as i64 - committed as i64;

        if lead < 0 || lead > lookahead as i64 {
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
        self.cap.store(committed + lookahead, Ordering::Relaxed);
        self.clear_measurements();
        self.window_end
            .store(now + self.sync_window, Ordering::Relaxed);
    }

    pub fn get_position(&self) -> u64 {
        let timeline = self.timeline(self.frames(Instant::now()));
        let frames = (timeline.max(0) as u64).min(self.cap.load(Ordering::Relaxed));

        // Events keep the order they were sent in, even across a correction
        // that rewound the timeline under them.
        let frames = frames.max(self.handed_out.fetch_max(frames, Ordering::Relaxed));

        frames * self.channels
    }

    /// Position of an event that carries its own timestamp.
    ///
    /// Neither the lookahead cap nor the `handed_out` latch applies here: the
    /// ports hold these events back until the block they fall into and order
    /// them there, so they may land well past the committed block, or before
    /// stamps that were already handed out. Only a timestamp further ahead
    /// than [`MAX_SCHEDULE_SECS`] is pulled in, so that a bogus one cannot
    /// hold an event back indefinitely.
    pub fn position_for(&self, at: Instant) -> u64 {
        let timeline = self.timeline(self.frames(at)).max(0) as u64;
        let limit = self.committed.load(Ordering::Relaxed) + self.max_schedule;

        timeline.min(limit) * self.channels
    }

    pub fn unstamped_position(&self) -> u32 {
        self.unstamped.load(Ordering::Relaxed)
    }
}
