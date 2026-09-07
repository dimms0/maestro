use std::{
    collections::VecDeque,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

// Original code by Arduano for XSynth (LGPL-3.0)
// (https://github.com/BlackMIDIDevs/xsynth/blob/master/realtime/src/event_senders/nps.rs)
//
// Reworked for Maestro so the per-note hot path is fully lock-free: the sliding
// windows and the NPS estimate are maintained by the background ticker thread,
// and `note_on` only touches atomics.

const NPS_WINDOW_MILLISECONDS: u64 = 1;

struct NpsWindow {
    time: u64,
    notes: u64,
}

struct ChannelNpsTracker {
    current_window_sum: AtomicU64,
    total_window_sum: AtomicU64,
    cached_nps: AtomicU64,
    skipped_notes: [AtomicU64; 128],
}

impl ChannelNpsTracker {
    fn new() -> Self {
        Self {
            current_window_sum: AtomicU64::new(0),
            total_window_sum: AtomicU64::new(0),
            cached_nps: AtomicU64::new(0),
            skipped_notes: [const { AtomicU64::new(0) }; 128],
        }
    }

    fn add_note(&self) {
        self.current_window_sum.fetch_add(1, Ordering::Relaxed);
        self.total_window_sum.fetch_add(1, Ordering::Relaxed);
    }

    fn add_skipped_note(&self, key: u8) {
        self.skipped_notes[key as usize].fetch_add(1, Ordering::Relaxed);
    }

    fn has_skipped_notes(&self, key: u8) -> bool {
        self.skipped_notes[key as usize].load(Ordering::Relaxed) > 0
    }

    fn sub_skipped_note(&self, key: u8) {
        self.skipped_notes[key as usize].fetch_sub(1, Ordering::Relaxed);
    }

    fn reset(&self) {
        for n in &self.skipped_notes {
            n.store(0, Ordering::Relaxed);
        }
    }
}

fn background_loop(channels: &[ChannelNpsTracker], stop: &AtomicBool) {
    let mut windows: Vec<VecDeque<NpsWindow>> =
        (0..channels.len()).map(|_| VecDeque::new()).collect();
    let mut virtual_time: u64 = 0;
    let mut now = Instant::now();

    while !stop.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(NPS_WINDOW_MILLISECONDS));
        virtual_time += now.elapsed().as_millis() as u64;
        now = Instant::now();

        let cutoff = virtual_time.saturating_sub(1000);

        for (channel, ch) in channels.iter().enumerate() {
            let win = &mut windows[channel];

            let notes = ch.current_window_sum.swap(0, Ordering::AcqRel);
            win.push_back(NpsWindow {
                time: virtual_time,
                notes,
            });

            while let Some(front) = win.front() {
                if front.time < cutoff {
                    ch.total_window_sum.fetch_sub(front.notes, Ordering::AcqRel);
                    win.pop_front();
                } else {
                    break;
                }
            }

            let short_nps = notes * (1000 / NPS_WINDOW_MILLISECONDS) * 4 / 3;
            let long_nps = ch.total_window_sum.load(Ordering::Relaxed);
            ch.cached_nps
                .store(short_nps.max(long_nps), Ordering::Relaxed);
        }
    }
}

pub(crate) struct NpsTracker {
    channels: Arc<[ChannelNpsTracker]>,
    max_nps: u64,
    stop: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl NpsTracker {
    pub(crate) fn new(channels: usize, max_nps: u64) -> Result<NpsTracker, io::Error> {
        let trackers: Arc<[ChannelNpsTracker]> = (0..channels)
            .map(|_| ChannelNpsTracker::new())
            .collect::<Vec<_>>()
            .into();

        let stop = Arc::new(AtomicBool::new(false));

        let join_handle = {
            let channels = trackers.clone();
            let stop = stop.clone();
            thread::Builder::new()
                .name("nps_tracker".to_string())
                .spawn(move || background_loop(&channels, &stop))?
        };

        Ok(NpsTracker {
            channels: trackers,
            max_nps,
            stop,
            join_handle: Some(join_handle),
        })
    }

    pub(crate) fn note_on(&self, channel: usize, key: u8, vel: u8) -> bool {
        if let Some(ch) = self.channels.get(channel) {
            let curr = ch.cached_nps.load(Ordering::Relaxed);

            if should_send_for_vel_and_nps(vel, curr, self.max_nps) {
                ch.add_note();
                true
            } else {
                ch.add_skipped_note(key);
                false
            }
        } else {
            false
        }
    }

    pub(crate) fn note_off(&self, channel: usize, key: u8) -> bool {
        if let Some(ch) = self.channels.get(channel) {
            if ch.has_skipped_notes(key) {
                ch.sub_skipped_note(key);
                false
            } else {
                true
            }
        } else {
            false
        }
    }

    pub(crate) fn reset(&self) {
        for c in self.channels.iter() {
            c.reset();
        }
    }
}

impl Drop for NpsTracker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.join_handle.take()
            && handle.join().is_err()
        {
            eprintln!("nps tracker thread panicked during shutdown");
        }
    }
}

impl Clone for NpsTracker {
    fn clone(&self) -> Self {
        Self {
            channels: self.channels.clone(),
            max_nps: self.max_nps,
            stop: self.stop.clone(),
            join_handle: None,
        }
    }
}

fn should_send_for_vel_and_nps(vel: u8, nps: u64, max: u64) -> bool {
    (vel as u64) * max / 127 > nps
}
