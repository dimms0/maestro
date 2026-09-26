use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
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
}
