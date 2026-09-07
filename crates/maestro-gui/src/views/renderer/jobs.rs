use std::sync::{Arc, Mutex};

use slint::{Global, Model, ModelRc, VecModel};

use crate::{MainWindow, RenderPhase, RenderStatus, RendererState, SlintRenderJob};

#[derive(Clone)]
pub struct RenderJob {
    pub id: u64,
    pub midi_path: String,
    pub phase: RenderPhase,
    pub progress: f32,
    pub indeterminate: bool,
    pub stats: String,
}

impl RenderJob {
    pub fn scanning(id: u64, midi_path: String) -> Self {
        Self {
            id,
            midi_path,
            phase: RenderPhase::Scanning,
            progress: 0.0,
            indeterminate: true,
            stats: "Scanning MIDI…".to_string(),
        }
    }

    fn to_slint(&self) -> SlintRenderJob {
        SlintRenderJob {
            midi_path: self.midi_path.as_str().into(),
            phase: self.phase,
            progress: self.progress,
            indeterminate: self.indeterminate,
            stats: self.stats.as_str().into(),
        }
    }
}

#[derive(Clone)]
pub struct JobRegistry {
    ui: slint::Weak<MainWindow>,
    jobs: Arc<Mutex<Vec<RenderJob>>>,
}

impl JobRegistry {
    pub fn new(ui: &slint::Weak<MainWindow>) -> Self {
        Self {
            ui: ui.clone(),
            jobs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn push(&self, job: RenderJob) {
        {
            let mut jobs = self.jobs.lock().unwrap();
            let pos = jobs.partition_point(|j| j.id < job.id);
            jobs.insert(pos, job);
        }
        self.publish();
    }

    pub fn update(&self, id: u64, f: impl FnOnce(&mut RenderJob)) {
        {
            let mut jobs = self.jobs.lock().unwrap();
            if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
                f(job);
            }
        }
        self.publish();
    }

    pub fn remove(&self, id: u64) {
        self.jobs.lock().unwrap().retain(|j| j.id != id);
        self.publish();
    }

    fn publish(&self) {
        let snapshot: Vec<SlintRenderJob> = self
            .jobs
            .lock()
            .unwrap()
            .iter()
            .map(RenderJob::to_slint)
            .collect();
        let ui = self.ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui.upgrade() {
                RendererState::get(&ui).set_render_jobs(ModelRc::new(VecModel::from(snapshot)));
            }
        });
    }
}

pub fn set_entry_status(ui_weak: &slint::Weak<MainWindow>, idx: usize, status: RenderStatus) {
    let ui = ui_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui.upgrade() {
            let statuses = RendererState::get(&ui).get_render_statuses();
            if idx < statuses.row_count() {
                statuses.set_row_data(idx, status);
            }
        }
    });
}
