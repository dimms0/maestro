use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Instant;

use rayon::prelude::*;

use midi_parser::MidiFile;

use crate::utils::{fmt_count, fmt_dur};
use crate::{
    MainWindow, RenderPhase, RenderStatus, SlintMidiRenderEntry, config::load_config_cache, errors,
};
use maestro_core::{
    audio_params::AudioParameters,
    file_renderer::{MaestroFileRenderer, OutputSettings},
    renderer::config::{EventProcessorConfig, PostProcessorConfig, RendererConfig},
    system_cfg::{ConfigComponent, MaestroConfigManager},
};

use super::jobs::{JobRegistry, RenderJob, set_entry_status};

const CANCEL_PANIC: &str = "__maestro_render_canceled__";
const ERROR_TITLE: &str = "MIDI Conversion Failed";
const CONVERTER_COMPONENT: &str = "converter";

pub fn is_cancel_panic(payload: &(dyn std::any::Any + Send)) -> bool {
    payload
        .downcast_ref::<&str>()
        .is_some_and(|s| *s == CANCEL_PANIC)
}

#[derive(Clone)]
pub struct ConverterRenderSettings {
    renderer: RendererConfig,
    audio_params: AudioParameters,
    output: OutputSettings,
    output_dir_override: String,
    event_processor: Option<EventProcessorConfig>,
    post_processor: Option<PostProcessorConfig>,
    pub multithreaded: bool,
    pub threads: usize,
}

impl ConverterRenderSettings {
    pub fn load() -> Self {
        let path =
            MaestroConfigManager::default().get_component_config_path(&ConfigComponent::Converter);
        let cfg = load_config_cache(CONVERTER_COMPONENT.to_string(), path).unwrap();

        Self {
            renderer: cfg.renderer,
            audio_params: cfg.audio_params,
            output: cfg.converter_custom.output_settings(),
            output_dir_override: cfg.converter_custom.output_path,
            event_processor: cfg.has_event_processor.then_some(cfg.event_processor),
            post_processor: cfg.has_post_processor.then_some(cfg.post_processor),
            multithreaded: cfg.converter_custom.multithreaded_export,
            threads: cfg.converter_custom.export_threads,
        }
    }

    fn build_renderer(
        &self,
        entry: &SlintMidiRenderEntry,
    ) -> Result<MaestroFileRenderer, Box<dyn std::error::Error>> {
        let midi_path = PathBuf::from(entry.midi_path.as_str());
        let output_dir = if self.output_dir_override.trim().is_empty() {
            midi_path.parent().unwrap_or(Path::new(".")).to_path_buf()
        } else {
            PathBuf::from(self.output_dir_override.as_str())
        };
        let sflist = MaestroConfigManager::default()
            .get_soundfont_list(entry.sflist_name.as_str())
            .unwrap_or_default();

        let mut renderer = MaestroFileRenderer::new(
            self.renderer,
            self.audio_params,
            sflist,
            &midi_path,
            output_dir,
        )?
        .with_output_settings(self.output);

        if let Some(ep) = &self.event_processor {
            renderer = renderer.with_event_processor(ep.clone());
        }
        if let Some(pp) = &self.post_processor {
            renderer = renderer.with_post_processor(*pp);
        }
        Ok(renderer)
    }
}

pub struct RenderRun<'a> {
    pub settings: &'a ConverterRenderSettings,
    pub cancel: &'a Arc<AtomicBool>,
    pub ui: &'a slint::Weak<MainWindow>,
    pub registry: &'a JobRegistry,
}

impl RenderRun<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn run_sequential(&self, queue: &[SlintMidiRenderEntry]) {
        for (idx, entry) in queue.iter().enumerate() {
            if !self.render_one(idx, entry) {
                break;
            }
        }
    }

    pub fn run_parallel(&self, queue: &[SlintMidiRenderEntry]) {
        let render_all = || {
            queue.par_iter().enumerate().for_each(|(idx, entry)| {
                let _ = self.render_one(idx, entry);
            });
        };

        let threads = self.settings.threads.min(queue.len());
        let pool = (threads > 0)
            .then(|| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .ok()
            })
            .flatten();

        match pool {
            Some(pool) => pool.install(render_all),
            None => render_all(),
        }
    }

    fn render_one(&self, idx: usize, entry: &SlintMidiRenderEntry) -> bool {
        if self.cancelled() {
            return false;
        }

        let job_id = idx as u64;
        set_entry_status(self.ui, idx, RenderStatus::Rendering);
        self.registry
            .push(RenderJob::scanning(job_id, entry.midi_path.to_string()));

        let total_pos = self.scan(entry, job_id);
        if self.cancelled() {
            set_entry_status(self.ui, idx, RenderStatus::Pending);
            self.registry.remove(job_id);
            return false;
        }

        let keep_going = self.render(idx, entry, job_id, total_pos);
        self.registry.remove(job_id);
        keep_going && !self.cancelled()
    }

    fn scan(&self, entry: &SlintMidiRenderEntry, job_id: u64) -> f64 {
        let registry = self.registry.clone();
        midi_total_pos(
            Path::new(entry.midi_path.as_str()),
            self.cancel,
            move |done, total| {
                let stats = format!("Scanning… track {done} of {total}");
                registry.update(job_id, |j| j.stats = stats);
            },
        )
    }

    fn render(
        &self,
        idx: usize,
        entry: &SlintMidiRenderEntry,
        job_id: u64,
        total_pos: f64,
    ) -> bool {
        self.registry.update(job_id, |j| {
            j.phase = RenderPhase::Rendering;
            j.indeterminate = false;
            j.progress = 0.0;
            j.stats = "Starting render…".to_string();
        });

        let renderer = match self.settings.build_renderer(entry) {
            Ok(renderer) => renderer,
            Err(e) => {
                set_entry_status(self.ui, idx, RenderStatus::Failed);
                errors::report(
                    ERROR_TITLE,
                    format!(
                        "Could not set up the renderer for \"{}\":\n\n{e}",
                        entry.midi_path
                    ),
                );
                return true;
            }
        };

        let cancel = self.cancel.clone();
        let registry = self.registry.clone();
        let started = Instant::now();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let mut last_percent = -1;
            renderer.render(Some(&mut |stats| {
                if cancel.load(Ordering::SeqCst) {
                    std::panic::panic_any(CANCEL_PANIC);
                }
                let frac = (stats.get_time() / total_pos).clamp(0.0, 1.0);
                let percent = (frac * 100.0) as i32;
                if percent == last_percent {
                    return;
                }
                last_percent = percent;

                let elapsed = started.elapsed().as_secs_f64();
                let line = format!(
                    "Notes: {} · Events: {} · Voices: {}\nElapsed {} · ETA {}",
                    fmt_count(stats.get_notes()),
                    fmt_count(stats.get_events()),
                    fmt_count(stats.get_renderer().read_voice_count()),
                    fmt_dur(elapsed),
                    fmt_dur(eta(elapsed, frac)),
                );
                registry.update(job_id, |j| {
                    j.progress = frac as f32;
                    j.stats = line;
                });
            }))
        }));

        match outcome {
            Ok(Ok(())) => {
                set_entry_status(self.ui, idx, RenderStatus::Done);
                true
            }
            // Render returned an error (e.g. a soundfont failed to load):
            // surface it and move on rather than silently producing nothing.
            Ok(Err(e)) => {
                set_entry_status(self.ui, idx, RenderStatus::Failed);
                errors::report(
                    ERROR_TITLE,
                    format!("Failed to render \"{}\":\n\n{e}", entry.midi_path),
                );
                true
            }
            // Unwound — a cancel or an unexpected panic (a real panic also
            // trips the fatal screen via the global hook). Stop the run.
            Err(_) => {
                set_entry_status(self.ui, idx, RenderStatus::Pending);
                false
            }
        }
    }
}

/// TODO: make this more accurate
fn eta(elapsed: f64, frac: f64) -> f64 {
    match frac > 0.005 {
        true => elapsed / frac * (1.0 - frac),
        false => 0.0,
    }
}

fn midi_total_pos(
    midi_path: &Path,
    cancel: &AtomicBool,
    on_progress: impl Fn(usize, usize) + Sync,
) -> f64 {
    let Ok(midi) = MidiFile::open(midi_path) else {
        return 1.0;
    };

    let total = midi.track_count();
    let done = AtomicUsize::new(0);
    let info = midi.scan_with(
        |_| on_progress(done.fetch_add(1, Ordering::Relaxed) + 1, total),
        cancel,
    );

    match info {
        Ok(info) if info.duration > 0.0 => info.duration,
        _ => 1.0,
    }
}
