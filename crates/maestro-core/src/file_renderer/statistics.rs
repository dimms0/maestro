use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use crate::statistics::MaestroRenderStatistics;

#[derive(Clone, Default)]
pub struct MaestroFileRendererStatistics {
    inner: Arc<Statistics>,
}

#[derive(Default)]
struct Statistics {
    renderer: Arc<MaestroRenderStatistics>,
    current_time: AtomicU64,
    events_processed: AtomicU64,
    notes_processed: AtomicU64,
}

impl MaestroFileRendererStatistics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_renderer(&self) -> &MaestroRenderStatistics {
        &self.inner.renderer
    }

    pub fn get_events(&self) -> u64 {
        self.inner.events_processed.load(Ordering::Relaxed)
    }

    pub fn get_notes(&self) -> u64 {
        self.inner.notes_processed.load(Ordering::Relaxed)
    }

    pub fn get_time(&self) -> f64 {
        f64::from_bits(self.inner.current_time.load(Ordering::Relaxed))
    }

    pub(super) fn renderer_handle(&self) -> Arc<MaestroRenderStatistics> {
        self.inner.renderer.clone()
    }

    pub(super) fn set_time(&self, seconds: f64) {
        self.inner
            .current_time
            .store(seconds.to_bits(), Ordering::Relaxed);
    }

    pub(super) fn add_processed(&self, events: u64, notes: u64) {
        self.inner
            .events_processed
            .fetch_add(events, Ordering::Relaxed);
        self.inner
            .notes_processed
            .fetch_add(notes, Ordering::Relaxed);
    }
}
