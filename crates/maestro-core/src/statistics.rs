use std::{
    collections::VecDeque,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Debug, Default)]
pub struct MaestroRenderStatistics {
    active_voice_count: Arc<AtomicU64>,
    render_time_history: Arc<RwLock<VecDeque<f32>>>,
}

impl MaestroRenderStatistics {
    pub fn new() -> Self {
        Self {
            active_voice_count: Arc::new(AtomicU64::new(0)),
            render_time_history: Arc::new(RwLock::new(VecDeque::new())),
        }
    }

    pub(crate) fn set_voices(&self, voices: u64) {
        self.active_voice_count.store(voices, Ordering::Relaxed);
    }

    pub(crate) fn add_render_time(&self, render_time: f32) {
        let mut lock = self.render_time_history.write().unwrap();

        lock.push_back(render_time);
        if lock.len() > 100 {
            lock.pop_front();
        }
    }

    pub fn read_voice_count(&self) -> u64 {
        self.active_voice_count.load(Ordering::Relaxed)
    }

    pub fn get_last_render_time(&self) -> f32 {
        let lock = self.render_time_history.read().unwrap();
        *lock.back().unwrap_or(&0.0)
    }

    pub fn get_average_render_time(&self) -> f32 {
        let lock = self.render_time_history.read().unwrap();

        let mut sum = 0.0;
        for v in lock.iter() {
            sum += v;
        }

        sum / lock.len().max(1) as f32
    }
}
