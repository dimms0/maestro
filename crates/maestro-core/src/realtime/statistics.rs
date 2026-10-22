use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicU64, Ordering},
};

use crate::statistics::MaestroRenderStatistics;

#[derive(Clone, Default)]
pub struct MaestroRealtimeStatistics {
    inner: Arc<Statistics>,
}

#[derive(Default)]
struct Statistics {
    renderer: Arc<MaestroRenderStatistics>,
    nps_limit: AtomicU64,
    notes_per_second: AtomicU64,
    events_per_second: AtomicU64,
    render_peak: AtomicU32,
}

impl MaestroRealtimeStatistics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_renderer(&self) -> &MaestroRenderStatistics {
        &self.inner.renderer
    }

    pub fn read_nps_limit(&self) -> u64 {
        self.inner.nps_limit.load(Ordering::Relaxed)
    }

    pub fn read_nps(&self) -> u64 {
        self.inner.notes_per_second.load(Ordering::Relaxed)
    }

    pub fn read_eps(&self) -> u64 {
        self.inner.events_per_second.load(Ordering::Relaxed)
    }

    pub fn read_render_peak(&self) -> f32 {
        f32::from_bits(self.inner.render_peak.load(Ordering::Relaxed))
    }

    pub(super) fn renderer_handle(&self) -> Arc<MaestroRenderStatistics> {
        self.inner.renderer.clone()
    }

    pub(super) fn set_nps_limit(&self, limit: Option<u64>) {
        self.inner
            .nps_limit
            .store(limit.unwrap_or(0), Ordering::Relaxed);
    }

    pub(super) fn set_rates(&self, nps: u64, eps: u64) {
        self.inner.notes_per_second.store(nps, Ordering::Relaxed);
        self.inner.events_per_second.store(eps, Ordering::Relaxed);
    }

    pub(super) fn set_render_peak(&self, peak: f32) {
        self.inner
            .render_peak
            .store(peak.to_bits(), Ordering::Relaxed);
    }
}

pub(super) struct PeakWindow {
    peak: f32,
    left: u64,
    length: u64,
}

impl PeakWindow {
    pub fn new(length: u64) -> Self {
        Self {
            peak: 0.0,
            left: length,
            length,
        }
    }

    pub fn record(&mut self, value: f32, samples: u64) -> Option<f32> {
        self.peak = self.peak.max(value);

        self.left = self.left.saturating_sub(samples);
        if self.left > 0 {
            return None;
        }

        self.left = self.length;
        Some(std::mem::take(&mut self.peak))
    }
}
