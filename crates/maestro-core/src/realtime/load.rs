use std::sync::atomic::{AtomicU32, Ordering};

use crate::realtime::MaestroRealtimeStatistics;

const TARGET_LOAD: f32 = 0.80;
const RECOVERY_PER_SECOND: f32 = 3.0;
const AVERAGE_WEIGHT: f32 = 0.05;
const MIN_NPS: f32 = 100.0;

/// Kept well under `u64::MAX / 127`, where the NPS limiter's
/// `vel * max / 127` would overflow.
const MAX_CAP: f32 = 1_000_000_000.0;

pub(crate) struct LoadLimiter {
    max_nps: AtomicU32,
    average: AtomicU32,
    cap: f32,
    stats: MaestroRealtimeStatistics,
}

impl LoadLimiter {
    pub(crate) fn new(cap: u64, stats: MaestroRealtimeStatistics) -> Self {
        let cap = (cap as f32).min(MAX_CAP);

        Self {
            max_nps: AtomicU32::new(cap.to_bits()),
            average: AtomicU32::new(0),
            cap,
            stats,
        }
    }

    pub(crate) fn update(&self, load: f32, block_seconds: f32) -> Option<u64> {
        let average = f32::from_bits(self.average.load(Ordering::Relaxed));
        let average = average + (load - average) * AVERAGE_WEIGHT;
        self.average.store(average.to_bits(), Ordering::Relaxed);

        let max = self.limit();

        let next = if load > TARGET_LOAD {
            let played = self.stats.read_nps() as f32;
            (max.min(played) * TARGET_LOAD / load).max(MIN_NPS)
        } else {
            let headroom = ((TARGET_LOAD - average) / TARGET_LOAD).max(0.0);
            max * (1.0 + RECOVERY_PER_SECOND * headroom * block_seconds)
        }
        .min(self.cap);

        self.max_nps.store(next.to_bits(), Ordering::Relaxed);

        let lowered = (next < self.cap).then_some(next as u64);
        self.stats.set_nps_limit(lowered);
        lowered
    }

    pub(crate) fn max_nps(&self) -> u64 {
        self.limit() as u64
    }

    pub(crate) fn reset(&self) {
        self.max_nps.store(self.cap.to_bits(), Ordering::Relaxed);
        self.average.store(0, Ordering::Relaxed);
        self.stats.set_nps_limit(None);
    }

    fn limit(&self) -> f32 {
        f32::from_bits(self.max_nps.load(Ordering::Relaxed))
    }
}
