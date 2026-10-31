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

use crate::realtime::{MaestroRealtimeStatistics, config::NpsLimit, host_time, load::LoadLimiter};

// Original code by Arduano for XSynth (LGPL-3.0)
// (https://github.com/BlackMIDIDevs/xsynth/blob/master/realtime/src/event_senders/nps.rs)
//
// Reworked for Maestro

const STATS_WINDOW_MILLISECONDS: u64 = 10;
const BURST_NANOS: u64 = 25_000_000;
const FRAC_BITS: u32 = 8;

fn background_loop(
    current_notes: &AtomicU64,
    current_events: &AtomicU64,
    stats: &MaestroRealtimeStatistics,
    stop: &AtomicBool,
) {
    let mut windows: VecDeque<(Instant, u64, u64)> = VecDeque::new();
    let mut total_notes: u64 = 0;
    let mut total_events: u64 = 0;

    while !stop.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(STATS_WINDOW_MILLISECONDS));
        let now = Instant::now();

        let notes = current_notes.swap(0, Ordering::AcqRel);
        let events = current_events.swap(0, Ordering::AcqRel);
        total_notes += notes;
        total_events += events;
        if notes > 0 || events > 0 {
            windows.push_back((now, notes, events));
        }

        while let Some(&(time, notes, events)) = windows.front() {
            if now.duration_since(time) < Duration::from_secs(1) {
                break;
            }
            total_notes -= notes;
            total_events -= events;
            windows.pop_front();
        }

        stats.set_rates(total_notes, total_events);
    }
}

struct Bucket {
    epoch: u64,
    second: u64,
    burst: u64,
    /// Time the bucket runs empty
    tat: AtomicU64,
}

impl Bucket {
    fn new() -> Self {
        Self {
            epoch: host_time::host_now(),
            second: host_time::ticks_per_sec() << FRAC_BITS,
            burst: host_time::ns_to_ticks(BURST_NANOS) << FRAC_BITS,
            tat: AtomicU64::new(0),
        }
    }

    #[inline]
    fn now(&self, host: u64) -> u64 {
        host.saturating_sub(self.epoch) << FRAC_BITS
    }

    #[inline]
    fn interval(&self, max: u64) -> Option<u64> {
        (max > 0).then(|| (self.second / max).max(1))
    }

    fn admit(&self, now: u64, vel: u8, max: u64) -> bool {
        let Some(interval) = self.interval(max) else {
            return false;
        };
        self.tat
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |tat| {
                let start = tat.max(now);
                fits(self.burst, start - now, vel).then_some(start + interval)
            })
            .is_ok()
    }

    fn take(&self, now: u64, interval: u64, notes: u64) {
        let _ = self
            .tat
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |tat| {
                Some(tat.max(now) + interval * notes)
            });
    }
}

#[inline]
fn fits(burst: u64, level: u64, vel: u8) -> bool {
    level * 127 < burst * vel as u64
}

pub(crate) struct NpsBatch {
    /// Note interval, start time and burst while limited
    limit: Option<(u64, u64, u64)>,
    tat: u64,
    notes: u64,
    pub events: u64,
}

pub(crate) struct NpsTracker {
    bucket: Arc<Bucket>,
    current_notes: Arc<AtomicU64>,
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
        let current_notes = Arc::new(AtomicU64::new(0));
        let current_events = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        let limiter = Arc::new(LoadLimiter::new(
            limit.map_or(0, |l| l.max as u64),
            limit.is_some_and(|l| l.load_limiter),
            stats.clone(),
        ));

        let join_handle = {
            let notes = current_notes.clone();
            let events = current_events.clone();
            let stop = stop.clone();
            Some(
                thread::Builder::new()
                    .name("nps_tracker".to_string())
                    .spawn(move || background_loop(&notes, &events, &stats, &stop))?,
            )
        };

        Ok(NpsTracker {
            bucket: Arc::new(Bucket::new()),
            current_notes,
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
    pub(crate) fn note_on(
        &self,
        channel: usize,
        key: u8,
        vel: u8,
        now: impl FnOnce() -> u64,
    ) -> bool {
        let Some(active) = self.active_notes.get(channel * 128 + key as usize) else {
            return false;
        };

        if let Some(max) = self.max_nps()
            && !self.bucket.admit(self.bucket.now(now()), vel, max)
        {
            return false;
        }

        self.current_notes.fetch_add(1, Ordering::Relaxed);
        active.fetch_add(1, Ordering::Relaxed);
        true
    }

    fn max_nps(&self) -> Option<u64> {
        self.limited
            .load(Ordering::Relaxed)
            .then(|| self.limiter.max_nps())
    }

    pub(crate) fn batch(&self, now: impl FnOnce() -> u64) -> NpsBatch {
        let limit = self.max_nps().map(|max| {
            let now = self.bucket.now(now());
            // A limit of 0 lets nothing through
            let interval = self.bucket.interval(max).unwrap_or(u64::MAX);
            (interval, now, self.bucket.burst)
        });
        let tat = limit.map_or(0, |(_, now, _)| {
            self.bucket.tat.load(Ordering::Relaxed).max(now)
        });
        NpsBatch {
            limit,
            tat,
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

        if let Some((interval, now, burst)) = batch.limit {
            if interval == u64::MAX || !fits(burst, batch.tat - now, vel) {
                return false;
            }
            batch.tat += interval;
        }

        batch.notes += 1;
        active.fetch_add(1, Ordering::Relaxed);
        true
    }

    pub(crate) fn commit(&self, batch: &NpsBatch) {
        if batch.notes > 0 {
            if let Some((interval, now, _)) = batch.limit {
                self.bucket.take(now, interval, batch.notes);
            }
            self.current_notes.fetch_add(batch.notes, Ordering::Relaxed);
        }
        if batch.events > 0 {
            self.current_events
                .fetch_add(batch.events, Ordering::Relaxed);
        }
    }

    pub(crate) fn count_event(&self) {
        self.current_events.fetch_add(1, Ordering::Relaxed);
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
        self.bucket.tat.store(0, Ordering::Relaxed);
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
            bucket: self.bucket.clone(),
            current_notes: self.current_notes.clone(),
            current_events: self.current_events.clone(),
            active_notes: self.active_notes.clone(),
            limiter: self.limiter.clone(),
            limited: self.limited.clone(),
            stop: self.stop.clone(),
            join_handle: None,
        }
    }
}
