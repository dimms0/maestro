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
    lookahead_end: AtomicI64,
    schedule_end: AtomicI64,

    /// Frames the renderer has committed to producing, i.e. where the block
    /// after the one being rendered starts. Events stamped while a block
    /// renders are consumed by the block after it, so this is what they are
    /// measured against.
    committed: AtomicI64,

    /// Host time the timeline passes `lookahead_end` at, which means the
    /// renderer has fallen behind. `u64::MAX` while nothing is skipped.
    behind_at: AtomicU64,
    unstamped: AtomicU32,
    auto_timestamp: AtomicBool,
    skip_behind: AtomicBool,

    handed_out: Padded<AtomicI64>,

    /// How far ahead of `committed` the timeline may run. Tells a desync from
    /// the render thread's normal swing.
    max_lookahead: AtomicI64,
    /// Frames between the device being handed audio and playing it.
    output_delay: AtomicU64,
    /// Frames from an event stamped now until it is heard.
    latency: AtomicU64,

    /// Closest and furthest the timeline came to the renderer in the current
    /// window, and the frame the window runs out at.
    window_min: AtomicI64,
    window_max: AtomicI64,
    window_end: AtomicI64,

    /// Frames per host tick in 0.64 fixed point
    scale: u64,
    sample_rate: u64,
    channels: u64,
    /// [`SYNC_WINDOW_NS`] in frames, so that measuring the window costs nothing
    /// more than the elapsed frames every block needs anyway.
    sync_window: i64,
    /// Floor the lookahead limit never drops below, a tenth of a second.
    min_lookahead: i64,
    /// [`MAX_SCHEDULE_SECS`] in frames.
    max_schedule: i64,
}

/// Keeps the latch that every stamp writes off the cache line the stamps read
#[repr(align(64))]
struct Padded<T>(T);

impl RealtimeClock {
    pub fn new(audio_params: &AudioParameters, auto_timestamp: bool, skip_behind: bool) -> Self {
        host_time::calibrate();

        let sample_rate = audio_params.sample_rate as u64;
        let min_lookahead = sample_rate as i64 / 10;

        Self {
            offset: AtomicI64::new(0),
            lookahead_end: AtomicI64::new(min_lookahead),
            schedule_end: AtomicI64::new(0),
            committed: AtomicI64::new(0),
            behind_at: AtomicU64::new(u64::MAX),
            unstamped: AtomicU32::new(0),
            auto_timestamp: AtomicBool::new(auto_timestamp),
            skip_behind: AtomicBool::new(skip_behind),
            handed_out: Padded(AtomicI64::new(0)),
            max_lookahead: AtomicI64::new(min_lookahead),
            output_delay: AtomicU64::new(0),
            latency: AtomicU64::new(0),
            window_min: AtomicI64::new(i64::MAX),
            window_max: AtomicI64::new(i64::MIN),
            window_end: AtomicI64::new(0),
            scale: ((u128::from(sample_rate) << 64) / u128::from(host_time::ticks_per_sec()))
                .min(u64::MAX as u128) as u64,
            sample_rate,
            channels: u16::from(audio_params.channels) as u64,
            sync_window: (sample_rate * SYNC_WINDOW_NS / NANOS_PER_SEC) as i64,
            min_lookahead,
            max_schedule: (sample_rate * MAX_SCHEDULE_SECS) as i64,
        }
    }

    #[inline]
    fn frames(&self, at: u64) -> i64 {
        ((u128::from(at) * u128::from(self.scale)) >> 64) as i64
    }

    fn ticks(&self, frames: i64) -> u64 {
        (frames.max(0) as u128 * u128::from(host_time::ticks_per_sec())
            / u128::from(self.sample_rate))
        .min(u64::MAX as u128) as u64
    }

    fn clear_measurements(&self) {
        self.window_min.store(i64::MAX, Ordering::Relaxed);
        self.window_max.store(i64::MIN, Ordering::Relaxed);
    }

    pub fn begin_block(&self, samples: u64, queued: u64) {
        self.sync(host_time::resync(), samples, queued);
    }

    fn sync(&self, host: u64, samples: u64, queued: u64) {
        let frames = (samples / self.channels) as i64;
        let now = self.frames(host);

        let mut lookahead = self.max_lookahead.load(Ordering::Relaxed);
        let committed = self.committed.load(Ordering::Relaxed) + frames;
        self.committed.store(committed, Ordering::Relaxed);
        self.schedule_end
            .store(committed + self.max_schedule, Ordering::Relaxed);
        self.unstamped.store(
            ((committed - self.sample_rate as i64).max(0) as u64 * self.channels) as u32,
            Ordering::Relaxed,
        );

        let mut offset = self.offset.load(Ordering::Relaxed);
        let target = TARGET_LEAD_BLOCKS as i64 * frames;
        let lead = offset + now - committed;

        let desynced = lead < 0 || lead > lookahead;

        let lead_after = if desynced { target } else { lead };
        let ahead =
            queued / self.channels + frames as u64 + self.output_delay.load(Ordering::Relaxed);
        self.latency
            .store(ahead + lead_after as u64, Ordering::Relaxed);

        if desynced {
            offset += target - lead;
            self.clear_measurements();
        } else {
            let window_min = self.window_min.load(Ordering::Relaxed).min(lead);
            let window_max = self.window_max.load(Ordering::Relaxed).max(lead);

            if now < self.window_end.load(Ordering::Relaxed) {
                self.window_min.store(window_min, Ordering::Relaxed);
                self.window_max.store(window_max, Ordering::Relaxed);
            } else {
                offset += target - window_min;
                lookahead = (2 * (window_max - window_min) + target).max(self.min_lookahead);
                self.max_lookahead.store(lookahead, Ordering::Relaxed);
                self.clear_measurements();
                self.window_end
                    .store(now + self.sync_window, Ordering::Relaxed);
            }
        }

        let lookahead_end = committed + lookahead;
        self.offset.store(offset, Ordering::Relaxed);
        self.lookahead_end.store(lookahead_end, Ordering::Relaxed);

        let behind_at = match self.skip_behind.load(Ordering::Relaxed) {
            true => self.ticks(lookahead_end - offset),
            false => u64::MAX,
        };
        self.behind_at.store(behind_at, Ordering::Relaxed);
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

    pub fn set_skip_behind(&self, skip: bool) {
        self.skip_behind.store(skip, Ordering::Relaxed);
        if !skip {
            self.behind_at.store(u64::MAX, Ordering::Relaxed);
        }
    }

    #[inline]
    pub fn behind(&self, now: u64) -> bool {
        now > self.behind_at.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn stamp(&self, at: u64, now: u64, behind: bool) -> u32 {
        let frames = if behind {
            let committed = self.committed.load(Ordering::Relaxed);
            self.handed_out.0.load(Ordering::Relaxed).max(committed)
        } else {
            let at = match at {
                0 if self.auto_timestamp.load(Ordering::Relaxed) => now,
                0 => return self.unstamped.load(Ordering::Relaxed),
                at => at,
            };

            let frames = (self.offset.load(Ordering::Relaxed) + self.frames(at))
                .min(self.schedule_end.load(Ordering::Relaxed));

            let handed_out = self.handed_out.0.load(Ordering::Relaxed);
            if frames > self.lookahead_end.load(Ordering::Relaxed) {
                frames
            } else if frames > handed_out {
                frames.max(self.handed_out.0.fetch_max(frames, Ordering::Relaxed))
            } else {
                handed_out
            }
        };

        (frames as u64 * self.channels) as u32
    }
}
