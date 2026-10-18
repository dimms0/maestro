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

pub(crate) struct NpsBatch {
    max: Option<u64>,
    total: u64,
    current: u64,
    notes: u64,
    pub events: u64,
}

fn over_limit(total: u64, current: u64, vel: u8, max: u64) -> bool {
    let threshold = (vel as u64) * max / 127;
    let short_nps = current * (1000 / NPS_WINDOW_MILLISECONDS) * 4 / 3;
    total.max(short_nps) >= threshold
}

pub(crate) struct NpsTracker {
    current_window_sum: Arc<AtomicU64>,
    total_window_sum: Arc<AtomicU64>,
    current_events: Arc<AtomicU64>,
    active_notes: Arc<[AtomicU64]>,
    limiter: Arc<LoadLimiter>,
    limited: Arc<AtomicBool>,
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

        let limiter = Arc::new(LoadLimiter::new(
            limit.map_or(0, |l| l.max as u64),
            limit.is_some_and(|l| l.load_limiter),
            stats.clone(),
        ));

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
            limited: Arc::new(AtomicBool::new(limit.is_some())),
            stop,
            join_handle,
        })
    }

    pub(crate) fn limiter(&self) -> Arc<LoadLimiter> {
        self.limiter.clone()
    }

    pub(crate) fn set_limit(&self, limit: Option<NpsLimit>) {
        self.limited.store(limit.is_some(), Ordering::Relaxed);
        self.limiter.configure(
            limit.map_or(0, |l| l.max as u64),
            limit.is_some_and(|l| l.load_limiter),
        );
    }

    #[inline]
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
        self.limited
            .load(Ordering::Relaxed)
            .then(|| self.limiter.max_nps())
    }

    fn admit(&self, vel: u8, max: u64) -> bool {
        let admitted = self
            .total_window_sum
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |total| {
                let current = self.current_window_sum.load(Ordering::Relaxed);
                (!over_limit(total, current, vel, max)).then_some(total + 1)
            })
            .is_ok();

        if admitted {
            self.current_window_sum.fetch_add(1, Ordering::Relaxed);
        }
        admitted
    }

    pub(crate) fn batch(&self) -> NpsBatch {
        let max = self.max_nps();
        let load = |sum: &AtomicU64| max.map_or(0, |_| sum.load(Ordering::Relaxed));
        NpsBatch {
            max,
            total: load(&self.total_window_sum),
            current: load(&self.current_window_sum),
            notes: 0,
            events: 0,
        }
    }

    #[inline]
    pub(crate) fn batch_note_on(
        &self,
        batch: &mut NpsBatch,
        channel: usize,
        key: u8,
        vel: u8,
    ) -> bool {
        let Some(active) = self.active_notes.get(channel * 128 + key as usize) else {
            return false;
        };

        if let Some(max) = batch.max
            && over_limit(
                batch.total + batch.notes,
                batch.current + batch.notes,
                vel,
                max,
            )
        {
            return false;
        }

        batch.notes += 1;
        active.fetch_add(1, Ordering::Relaxed);
        true
    }

    pub(crate) fn commit(&self, batch: &NpsBatch) {
        if batch.notes > 0 {
            self.total_window_sum
                .fetch_add(batch.notes, Ordering::Relaxed);
            self.current_window_sum
                .fetch_add(batch.notes, Ordering::Relaxed);
        }
        if batch.events > 0 {
            self.current_events
                .fetch_add(batch.events, Ordering::Relaxed);
        }
    }

    fn count_note(&self) {
        self.total_window_sum.fetch_add(1, Ordering::Relaxed);
        self.current_window_sum.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn count_event(&self) {
        self.current_events.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn event_counter(&self) -> Arc<AtomicU64> {
        self.current_events.clone()
    }

    #[inline]
    pub(crate) fn note_off(&self, channel: usize, key: u8) -> bool {
        self.active_notes
            .get(channel * 128 + key as usize)
            .is_some_and(|n| {
                n.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                    (n > 0).then(|| n - 1)
                })
                .is_ok()
            })
    }

    pub(crate) fn reset(&self) {
        for n in self.active_notes.iter() {
            n.store(0, Ordering::Relaxed);
        }
        self.limiter.reset();
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
            limited: self.limited.clone(),
            stop: self.stop.clone(),
            join_handle: None,
        }
    }
}
