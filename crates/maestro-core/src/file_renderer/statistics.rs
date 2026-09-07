use std::sync::Arc;

use crate::statistics::MaestroRenderStatistics;

pub struct MaestroFileRendererStatistics {
    renderer: Arc<MaestroRenderStatistics>,
    current_time: f64,
    events_processed: u64,
    notes_processed: u64,
}

impl MaestroFileRendererStatistics {
    pub fn new(renderer: Arc<MaestroRenderStatistics>) -> Self {
        Self {
            renderer,
            current_time: 0.0,
            events_processed: 0,
            notes_processed: 0,
        }
    }

    pub fn get_renderer(&self) -> &MaestroRenderStatistics {
        &self.renderer
    }

    pub fn get_events(&self) -> u64 {
        self.events_processed
    }

    pub fn get_notes(&self) -> u64 {
        self.notes_processed
    }

    pub fn get_time(&self) -> f64 {
        self.current_time
    }

    pub(super) fn set_time(&mut self, seconds: f64) {
        self.current_time = seconds;
    }

    pub(super) fn add_processed(&mut self, events: u64, notes: u64) {
        self.events_processed += events;
        self.notes_processed += notes;
    }
}
