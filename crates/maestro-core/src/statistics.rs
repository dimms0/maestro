use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering},
};

const HISTORY_LEN: usize = 20;

#[derive(Clone, Debug, Default)]
pub struct MaestroRenderStatistics {
    active_voice_count: Arc<AtomicU64>,
    render_time_history: Arc<[AtomicU32; HISTORY_LEN]>,
    render_time_count: Arc<AtomicUsize>,
}

impl MaestroRenderStatistics {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set_voices(&self, voices: u64) {
        self.active_voice_count.store(voices, Ordering::Relaxed);
    }

    pub(crate) fn add_render_time(&self, render_time: f32) {
        let count = self.render_time_count.load(Ordering::Relaxed);
        self.render_time_history[count % HISTORY_LEN]
            .store(render_time.to_bits(), Ordering::Relaxed);
        self.render_time_count.store(count + 1, Ordering::Relaxed);
    }

    pub fn read_voice_count(&self) -> u64 {
        self.active_voice_count.load(Ordering::Relaxed)
    }

    pub fn get_last_render_time(&self) -> f32 {
        match self.render_time_count.load(Ordering::Relaxed) {
            0 => 0.0,
            count => f32::from_bits(
                self.render_time_history[(count - 1) % HISTORY_LEN].load(Ordering::Relaxed),
            ),
        }
    }

    pub fn get_average_render_time(&self) -> f32 {
        let filled = self
            .render_time_count
            .load(Ordering::Relaxed)
            .min(HISTORY_LEN);

        let mut sum = 0.0;
        for v in &self.render_time_history[..filled] {
            sum += f32::from_bits(v.load(Ordering::Relaxed));
        }

        sum / filled.max(1) as f32
    }
}
