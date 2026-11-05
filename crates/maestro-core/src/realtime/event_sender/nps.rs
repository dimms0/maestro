use std::{
    collections::VecDeque,
    io,
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::realtime::{MaestroRealtimeStatistics, host_time};

// Original code by Arduano for XSynth (LGPL-3.0)
// (https://github.com/BlackMIDIDevs/xsynth/blob/master/realtime/src/event_senders/nps.rs)
//
// Reworked for Maestro

const STATS_WINDOW_MILLISECONDS: u64 = 10;
const BURST_NANOS: u64 = 25_000_000;
const FRAC_BITS: u32 = 8;

fn background_loop(counts: &AtomicU64, stats: &MaestroRealtimeStatistics, stop: &AtomicBool) {
    let mut windows: VecDeque<(Instant, u64, u64)> = VecDeque::new();
    let mut total_notes: u64 = 0;
    let mut total_events: u64 = 0;

    while !stop.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(STATS_WINDOW_MILLISECONDS));
        let now = Instant::now();

        let counts = counts.swap(0, Ordering::AcqRel);
        let (notes, events) = (counts >> 32, counts & u32::MAX as u64);
        total_notes += notes;
        total_events += events;
        if counts > 0 {
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
    /// Ticks per note at the limit
    interval: AtomicU64,
    /// Time the bucket runs empty
    tat: AtomicU64,
}

impl Bucket {
    fn new() -> Self {
        Self {
            epoch: host_time::host_now(),
            second: host_time::ticks_per_sec() << FRAC_BITS,
            burst: host_time::ns_to_ticks(BURST_NANOS) << FRAC_BITS,
            interval: AtomicU64::new(0),
            tat: AtomicU64::new(0),
        }
    }

    #[inline]
    fn now(&self, host: u64) -> u64 {
        host.saturating_sub(self.epoch) << FRAC_BITS
    }

    fn set_limit(&self, limit: Option<usize>) {
        let interval = match limit {
            None => 0,
            Some(0) => u64::MAX,
            Some(max) => (self.second / max as u64).max(1),
        };
        self.interval.store(interval, Ordering::Relaxed);
    }

    #[inline]
    fn interval(&self) -> u64 {
        self.interval.load(Ordering::Relaxed)
    }

    fn admit(&self, now: u64, vel: u8, interval: u64) -> bool {
        interval != u64::MAX
            && self
                .tat
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
    /// Note interval and start time while limited
    limit: Option<(u64, u64)>,
    tat: u64,
    notes: u64,
    pub events: u64,
}

pub(crate) struct NpsTracker {
    bucket: Arc<Bucket>,
    // high 32 bits are notes and low 32 bits are events
    counts: Arc<AtomicU64>,
    active_notes: Arc<[AtomicU32]>,
    stop: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl NpsTracker {
    pub(crate) fn new(
        channels: usize,
        limit: Option<usize>,
        stats: MaestroRealtimeStatistics,
    ) -> Result<NpsTracker, io::Error> {
        let counts = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        let join_handle = {
            let counts = counts.clone();
            let stop = stop.clone();
            Some(
                thread::Builder::new()
                    .name("nps_tracker".to_string())
                    .spawn(move || background_loop(&counts, &stats, &stop))?,
            )
        };

        let bucket = Bucket::new();
        bucket.set_limit(limit);

        Ok(NpsTracker {
            bucket: Arc::new(bucket),
            counts,
            active_notes: (0..channels * 128).map(|_| AtomicU32::new(0)).collect(),
            stop,
            join_handle,
        })
    }

    pub(crate) fn set_limit(&self, limit: Option<usize>) {
        self.bucket.set_limit(limit);
    }

    #[inline]
    pub(crate) fn note_on(
        &self,
        channel: usize,
        key: u8,
        vel: u8,
        now: u64,
        dropped: bool,
    ) -> bool {
        let Some(active) = self.active_notes.get(channel * 128 + key as usize) else {
            return false;
        };

        let played = !dropped
            && match self.bucket.interval() {
                0 => true,
                interval => self.bucket.admit(self.bucket.now(now), vel, interval),
            };

        if played {
            active.fetch_add(1, Ordering::Relaxed);
            self.counts.fetch_add(1 << 32 | 1, Ordering::Relaxed);
        }
        played
    }

    pub(crate) fn batch(&self, now: u64) -> NpsBatch {
        let limit = match self.bucket.interval() {
            0 => None,
            interval => Some((interval, self.bucket.now(now))),
        };
        let tat = limit.map_or(0, |(_, now)| {
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
        dropped: bool,
    ) -> bool {
        let Some(active) = self.active_notes.get(channel * 128 + key as usize) else {
            return false;
        };

        let played = !dropped
            && match batch.limit {
                None => true,
                Some((interval, now)) => {
                    interval != u64::MAX && fits(self.bucket.burst, batch.tat - now, vel)
                }
            };

        if played {
            active.fetch_add(1, Ordering::Relaxed);
            if let Some((interval, _)) = batch.limit {
                batch.tat += interval;
            }
            batch.notes += 1;
        }
        played
    }

    pub(crate) fn commit(&self, batch: &NpsBatch) {
        if batch.notes > 0
            && let Some((interval, now)) = batch.limit
        {
            self.bucket.take(now, interval, batch.notes);
        }
        let counts = batch.notes << 32 | batch.events;
        if counts > 0 {
            self.counts.fetch_add(counts, Ordering::Relaxed);
        }
    }

    pub(crate) fn count_event(&self) {
        self.counts.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn note_off(&self, channel: usize, key: u8) -> bool {
        self.active_notes
            .get(channel * 128 + key as usize)
            .is_some_and(|n| {
                n.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
                    .is_ok()
            })
    }

    pub(crate) fn clear(&self, channels: Range<usize>) {
        if let Some(notes) = self
            .active_notes
            .get(channels.start * 128..channels.end * 128)
        {
            for n in notes {
                n.store(0, Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn reset(&self) {
        self.clear(0..self.active_notes.len() / 128);
        self.bucket.tat.store(0, Ordering::Relaxed);
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
            counts: self.counts.clone(),
            active_notes: self.active_notes.clone(),
            stop: self.stop.clone(),
            join_handle: None,
        }
    }
}
