use std::{
    sync::atomic::{AtomicI64, AtomicU64, Ordering},
    time::Instant,
};

use midi_parser::Division;

use crate::{audio_params::AudioParameters, tempo::TempoClock};

const NANOS_PER_SEC: u64 = 1_000_000_000;

pub(super) struct RendererClock {
    pos: AtomicU64,
    channels: u64,
    sample_rate: u32,
    tempo: TempoClock,
}

impl RendererClock {
    pub fn new(audio_params: &AudioParameters) -> Self {
        Self {
            pos: AtomicU64::new(0),
            channels: u16::from(audio_params.channels) as u64,
            sample_rate: audio_params.sample_rate,
            tempo: TempoClock::new(Division::Ppq(96), audio_params.sample_rate),
        }
    }

    pub fn set_division(&mut self, division: Division) {
        self.tempo = TempoClock::new(division, self.sample_rate);
    }

    pub fn set_tempo(&mut self, micros_per_quarter: u32) {
        self.tempo.set_tempo(micros_per_quarter);
    }

    pub fn advance_by_ticks(&mut self, ticks: u64) {
        let samples = self.tempo.advance(ticks);
        self.advance_by_samples(samples);
    }

    pub fn advance_by_samples(&self, mono_samples: u64) {
        self.pos.store(
            self.pos.load(Ordering::Relaxed) + mono_samples,
            Ordering::Relaxed,
        );
    }

    pub fn get_position(&self) -> u64 {
        self.pos.load(Ordering::Relaxed) * self.channels
    }
}

/// Slack kept between the event timeline and the renderer, in render blocks.
/// Events are stamped this far ahead of the block the renderer is working on,
/// so they land in audio that still has to be produced and can be placed at
/// their exact sample position.
const TARGET_LEAD_BLOCKS: u64 = 2;

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
/// Everything the render thread keeps for itself sits below `cap`. It is the
/// only writer, so nothing here needs a read-modify-write, and no state is
/// published alongside these values, so nothing here needs an ordering beyond
/// `Relaxed`: stamping an event is left with two loads.
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
            window_min: AtomicI64::new(i64::MAX),
            window_max: AtomicI64::new(i64::MIN),
            window_end: AtomicU64::new(sync_window),
            start: Instant::now(),
            sample_rate,
            channels: u16::from(audio_params.channels) as u64,
            sync_window,
            min_lookahead,
        }
    }

    fn elapsed_frames(&self) -> u64 {
        let elapsed = self.start.elapsed();

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
        let now = self.elapsed_frames();

        let lookahead = self.max_lookahead.load(Ordering::Relaxed);
        let committed = self.committed.load(Ordering::Relaxed) + frames;
        self.committed.store(committed, Ordering::Relaxed);
        self.cap.store(committed + lookahead, Ordering::Relaxed);

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
        let timeline = self.timeline(self.elapsed_frames());
        let frames = (timeline.max(0) as u64).min(self.cap.load(Ordering::Relaxed));

        frames * self.channels
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, atomic::AtomicBool},
        thread::{self, sleep},
        time::Duration,
    };

    use super::*;

    const BLOCK: u64 = 480 * 2; // 5 ms of stereo audio at the default 48 kHz
    const RATE: u64 = 48_000;
    const CHANNELS: u64 = 2;

    fn clock() -> RealtimeClock {
        RealtimeClock::new(&AudioParameters::default())
    }

    /// Runs the clock through `blocks` blocks the way the buffered renderer
    /// would: bursts of `burst` blocks, one burst per device callback.
    fn run_blocks(clock: &RealtimeClock, blocks: usize, burst: usize) {
        for i in 0..blocks {
            clock.begin_block(BLOCK);
            if (i + 1) % burst == 0 {
                let callback = BLOCK * burst as u64 / CHANNELS;
                sleep(Duration::from_nanos(callback * 1_000_000_000 / RATE));
            }
        }
    }

    #[test]
    fn positions_are_whole_frames() {
        let clock = clock();
        run_blocks(&clock, 8, 4);

        for _ in 0..20 {
            assert_eq!(clock.get_position() % CHANNELS, 0);
            sleep(Duration::from_micros(137));
        }
    }

    #[test]
    fn event_spacing_matches_the_wall_clock() {
        let clock = clock();
        run_blocks(&clock, 8, 4);

        // The whole point of the timeline: two events sent 40 ms apart have to
        // be 40 ms apart in the rendered audio, not rounded onto whatever block
        // the renderer happened to be working on.
        let first = clock.get_position();
        sleep(Duration::from_millis(40));
        let second = clock.get_position();

        let expected = 40 * RATE * CHANNELS / 1000;
        let spacing = second - first;
        let error = spacing.abs_diff(expected);
        assert!(
            error < expected / 20,
            "spacing {spacing} is off the expected {expected} by {error}"
        );
    }

    #[test]
    fn events_stay_ahead_of_the_renderer() {
        let clock = clock();

        // A whole second of blocks, rendered in bursts like the buffered
        // renderer does. Events must keep landing in blocks that have not been
        // rendered yet, otherwise they get played at the start of the block
        // instead of where they belong.
        for _ in 0..40 {
            run_blocks(&clock, 5, 5);
            let committed = 5 * BLOCK;
            assert!(
                clock.get_position() >= committed,
                "timeline fell behind the renderer"
            );
        }
    }

    #[test]
    fn a_player_sending_at_a_steady_rate_keeps_its_spacing() {
        let clock = Arc::new(clock());
        let stop = Arc::new(AtomicBool::new(false));

        // The renderer, pacing itself the way the buffered renderer does: a
        // burst of blocks whenever the device drains the queue, then idle.
        let renderer = {
            let clock = clock.clone();
            let stop = stop.clone();
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    run_blocks(&clock, 5, 5);
                }
            })
        };

        // The player, sending an event every 10 ms.
        let gap = Duration::from_millis(10);
        let mut next = Instant::now() + gap;
        let mut positions = Vec::new();
        for _ in 0..30 {
            spin_sleep::sleep(next.saturating_duration_since(Instant::now()));
            positions.push(clock.get_position());
            next += gap;
        }

        stop.store(true, Ordering::Relaxed);
        renderer.join().unwrap();

        let expected = gap.as_millis() as u64 * RATE * CHANNELS / 1000;
        let tolerance = 3 * RATE * CHANNELS / 1000; // 3 ms
        for pair in positions.windows(2) {
            let spacing = pair[1] - pair[0];
            assert!(
                spacing.abs_diff(expected) < tolerance,
                "event spacing {spacing} strays from the {expected} it was sent with"
            );
        }

        // And no drift over the whole run.
        let span = positions.last().unwrap() - positions.first().unwrap();
        let expected_span = expected * (positions.len() as u64 - 1);
        assert!(
            span.abs_diff(expected_span) < tolerance,
            "the timeline drifted: {span} samples instead of {expected_span}"
        );
    }

    #[test]
    fn a_stalled_renderer_cannot_push_events_into_the_future() {
        let clock = clock();
        run_blocks(&clock, 8, 4);

        // Nothing is rendered while the renderer is stuck (loading soundfonts,
        // overloaded, paused), so events pile up at the lookahead limit and
        // play as soon as it catches up, rather than a second late.
        sleep(Duration::from_millis(500));

        let committed = 8 * BLOCK;
        let lookahead = clock.get_position() - committed;
        assert!(
            lookahead <= RATE * CHANNELS / 4,
            "stalled renderer stamped events {lookahead} samples ahead"
        );
    }

    #[test]
    fn the_timeline_recovers_after_a_stall() {
        let clock = clock();
        run_blocks(&clock, 8, 4);
        sleep(Duration::from_millis(500));

        // The first block after the stall snaps the timeline back onto the
        // renderer, so the events that follow are timed normally again.
        run_blocks(&clock, 8, 4);

        let committed = 16 * BLOCK;
        let lead = clock.get_position() - committed;
        assert!(
            lead <= 8 * BLOCK,
            "timeline still {lead} samples ahead after recovering"
        );
    }
}
