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

use crate::realtime::{MaestroRealtimeStatistics, config::NpsLimit, load::LoadLimiter};

// Original code by Arduano for XSynth (LGPL-3.0)
// (https://github.com/BlackMIDIDevs/xsynth/blob/master/realtime/src/event_senders/nps.rs)
//
// Reworked for Maestro

const NPS_WINDOW_MILLISECONDS: u64 = 1;

struct NpsWindow {
    time: u64,
    notes: u64,
    events: u64,
}

fn background_loop(
    current_window_sum: &AtomicU64,
    total_window_sum: &AtomicU64,
    current_events: &AtomicU64,
    stats: &MaestroRealtimeStatistics,
    stop: &AtomicBool,
) {
    let mut windows: VecDeque<NpsWindow> = VecDeque::new();
    let mut virtual_time: u64 = 0;
    let mut now = Instant::now();
    let mut total_events: u64 = 0;

    while !stop.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(NPS_WINDOW_MILLISECONDS));
        virtual_time += now.elapsed().as_millis() as u64;
        now = Instant::now();

        let notes = current_window_sum.swap(0, Ordering::AcqRel);
        let events = current_events.swap(0, Ordering::AcqRel);
        total_events += events;
        if notes > 0 || events > 0 {
            windows.push_back(NpsWindow {
                time: virtual_time,
                notes,
                events,
            });
        }

        let cutoff = virtual_time.saturating_sub(1000);
        while let Some(front) = windows.front() {
            if front.time < cutoff {
                total_window_sum.fetch_sub(front.notes, Ordering::AcqRel);
                total_events -= front.events;
                windows.pop_front();
            } else {
                break;
            }
        }

        stats.set_rates(total_window_sum.load(Ordering::Acquire), total_events);
    }
}

pub(crate) struct NpsTracker {
    current_window_sum: Arc<AtomicU64>,
    total_window_sum: Arc<AtomicU64>,
    current_events: Arc<AtomicU64>,
    active_notes: Arc<[AtomicU64]>,
    limiter: Option<Arc<LoadLimiter>>,
    max_nps: Option<u64>,
    stop: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl NpsTracker {
    pub(crate) fn new(
        channels: usize,
        limit: Option<NpsLimit>,
        stats: MaestroRealtimeStatistics,
    ) -> Result<NpsTracker, io::Error> {
        let current_window_sum = Arc::new(AtomicU64::new(0));
        let total_window_sum = Arc::new(AtomicU64::new(0));
        let current_events = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        let max_nps = limit.map(|limit| limit.max as u64);
        let limiter = limit
            .filter(|limit| limit.load_limiter)
            .map(|limit| Arc::new(LoadLimiter::new(limit.max as u64, stats.clone())));

        let join_handle = {
            let current = current_window_sum.clone();
            let total = total_window_sum.clone();
            let events = current_events.clone();
            let stop = stop.clone();
            Some(
                thread::Builder::new()
                    .name("nps_tracker".to_string())
                    .spawn(move || background_loop(&current, &total, &events, &stats, &stop))?,
            )
        };

        Ok(NpsTracker {
            current_window_sum,
            total_window_sum,
            current_events,
            active_notes: (0..channels * 128).map(|_| AtomicU64::new(0)).collect(),
            limiter,
            max_nps,
            stop,
            join_handle,
        })
    }

    pub(crate) fn limiter(&self) -> Option<Arc<LoadLimiter>> {
        self.limiter.clone()
    }

    pub(crate) fn note_on(&self, channel: usize, key: u8, vel: u8) -> bool {
        let Some(active) = self.active_notes.get(channel * 128 + key as usize) else {
            return false;
        };

        match self.max_nps() {
            Some(max) if !self.admit(vel, max) => return false,
            Some(_) => {}
            None => self.count_note(),
        }

        active.fetch_add(1, Ordering::Relaxed);
        true
    }

    fn max_nps(&self) -> Option<u64> {
        match &self.limiter {
            Some(limiter) => Some(limiter.max_nps()),
            None => self.max_nps,
        }
    }

    fn admit(&self, vel: u8, max: u64) -> bool {
        let threshold = (vel as u64) * max / 127;

        let admitted = self
            .total_window_sum
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |total| {
                let current = self.current_window_sum.load(Ordering::Relaxed);
                let short_nps = current * (1000 / NPS_WINDOW_MILLISECONDS) * 4 / 3;

                (total.max(short_nps) < threshold).then_some(total + 1)
            })
            .is_ok();

        if admitted {
            self.current_window_sum.fetch_add(1, Ordering::AcqRel);
        }
        admitted
    }

    fn count_note(&self) {
        self.total_window_sum.fetch_add(1, Ordering::AcqRel);
        self.current_window_sum.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn count_event(&self) {
        self.current_events.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn event_counter(&self) -> Arc<AtomicU64> {
        self.current_events.clone()
    }

    pub(crate) fn note_off(&self, channel: usize, key: u8) -> bool {
        self.active_notes
            .get(channel * 128 + key as usize)
            .is_some_and(|n| {
                n.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                    (n > 0).then(|| n - 1)
                })
                .is_ok()
            })
    }

    pub(crate) fn reset(&self) {
        for n in self.active_notes.iter() {
            n.store(0, Ordering::Relaxed);
        }
        if let Some(limiter) = &self.limiter {
            limiter.reset();
        }
    }
}

impl Drop for NpsTracker {
    fn drop(&mut self) {
        if let Some(handle) = self.join_handle.take() {
            self.stop.store(true, Ordering::Release);
            if handle.join().is_err() {
                eprintln!("nps tracker thread panicked during shutdown");
            }
        }
    }
}

impl Clone for NpsTracker {
    fn clone(&self) -> Self {
        Self {
            current_window_sum: self.current_window_sum.clone(),
            total_window_sum: self.total_window_sum.clone(),
            current_events: self.current_events.clone(),
            active_notes: self.active_notes.clone(),
            limiter: self.limiter.clone(),
            max_nps: self.max_nps,
            stop: self.stop.clone(),
            join_handle: None,
        }
    }
}
