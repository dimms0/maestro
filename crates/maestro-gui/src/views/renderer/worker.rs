use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use midi_parser::MidiFile;

use crate::utils::{fmt_count, fmt_dur};
use crate::{
    MainWindow, RenderPhase, RenderStatus, SlintMidiRenderEntry, config::load_config_cache, errors,
};
use maestro_core::{
    audio_params::AudioParameters,
    file_renderer::{
        MaestroFileRenderer, MaestroFileRendererStatistics, OutputSettings, RenderOutcome,
    },
    renderer::{
        config::{EventProcessorConfig, PostProcessorConfig, RendererConfig},
        pool_threads,
    },
    system_cfg::{ConfigComponent, MaestroConfigManager},
};

use super::jobs::{JobRegistry, RenderJob, set_entry_status};

const ERROR_TITLE: &str = "MIDI Conversion Failed";
const CONVERTER_COMPONENT: &str = "converter";

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone)]
pub struct ConverterRenderSettings {
    renderer: RendererConfig,
    audio_params: AudioParameters,
    output: OutputSettings,
    /// `None` means "next to the MIDI": either the switch is off or the
    /// configured directory is not there any more.
    output_dir_override: Option<PathBuf>,
    event_processor: Option<EventProcessorConfig>,
    post_processor: Option<PostProcessorConfig>,
    pub multithreaded: bool,
    pub threads: usize,
    pool: Option<Arc<rayon::ThreadPool>>,
}

impl ConverterRenderSettings {
    pub fn load() -> Self {
        let path =
            MaestroConfigManager::default().get_component_config_path(&ConfigComponent::Converter);
        let cfg = load_config_cache(CONVERTER_COMPONENT.to_string(), path).unwrap();

        let threads = pool_threads(&cfg.renderer, usize::MAX);
        let pool = (threads > 1)
            .then(|| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .ok()
            })
            .flatten()
            .map(Arc::new);

        Self {
            renderer: cfg.renderer,
            audio_params: cfg.audio_params,
            output: cfg.converter_custom.output_settings(),
            output_dir_override: cfg.converter_custom.output_dir(),
            event_processor: cfg.has_event_processor.then_some(cfg.event_processor),
            post_processor: cfg.has_post_processor.then_some(cfg.post_processor),
            multithreaded: cfg.converter_custom.multithreaded_export,
            threads: cfg.converter_custom.export_threads,
            pool,
        }
    }

    fn build_renderer(
        &self,
        entry: &SlintMidiRenderEntry,
        midi: midi_parser::Result<MidiFile<'static>>,
    ) -> Result<MaestroFileRenderer, Box<dyn std::error::Error>> {
        let midi_path = PathBuf::from(entry.midi_path.as_str());
        let name = match entry.entry.as_str() {
            "" => midi_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            entry => entry.to_string(),
        };
        let output_dir = self
            .output_dir_override
            .clone()
            .unwrap_or_else(|| midi_path.parent().unwrap_or(Path::new(".")).to_path_buf());
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
        .with_midi(midi?, name)
        .with_output_settings(self.output);

        if let Some(ep) = &self.event_processor {
            renderer = renderer.with_event_processor(ep.clone());
        }
        if let Some(pp) = &self.post_processor {
            renderer = renderer.with_post_processor(*pp);
        }
        if let Some(pool) = &self.pool {
            renderer = renderer.with_threadpool(pool.clone());
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
        let threads = match self.settings.threads {
            0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
            threads => threads,
        };
        let next = AtomicUsize::new(0);

        std::thread::scope(|scope| {
            for _ in 0..threads.min(queue.len()) {
                scope.spawn(|| {
                    loop {
                        let idx = next.fetch_add(1, Ordering::Relaxed);
                        let Some(entry) = queue.get(idx) else { break };
                        let _ = self.render_one(idx, entry);
                    }
                });
            }
        });
    }

    fn render_one(&self, idx: usize, entry: &SlintMidiRenderEntry) -> bool {
        if self.cancelled() {
            return false;
        }

        let job_id = idx as u64;
        set_entry_status(self.ui, idx, RenderStatus::Rendering);
        self.registry
            .push(RenderJob::scanning(job_id, label(entry)));

        let midi = match entry.entry.as_str() {
            "" => MidiFile::open(entry.midi_path.as_str()),
            name => MidiFile::open_entry(entry.midi_path.as_str(), name),
        };
        let total_pos = self.scan(midi.as_ref().ok(), job_id);
        if self.cancelled() {
            set_entry_status(self.ui, idx, RenderStatus::Pending);
            self.registry.remove(job_id);
            return false;
        }

        let keep_going = self.render(idx, entry, midi, job_id, total_pos);
        self.registry.remove(job_id);
        keep_going && !self.cancelled()
    }

    fn scan(&self, midi: Option<&MidiFile>, job_id: u64) -> f64 {
        let registry = self.registry.clone();
        midi_total_pos(midi, self.cancel, move |done, total| {
            let stats = format!("Scanning… track {done} of {total}");
            registry.update(job_id, |j| j.stats = stats);
        })
    }

    fn render(
        &self,
        idx: usize,
        entry: &SlintMidiRenderEntry,
        midi: midi_parser::Result<MidiFile<'static>>,
        job_id: u64,
        total_pos: f64,
    ) -> bool {
        self.registry.update(job_id, |j| {
            j.phase = RenderPhase::Rendering;
            j.indeterminate = false;
            j.progress = 0.0;
            j.stats = "Starting render…".to_string();
        });

        let renderer = match self.settings.build_renderer(entry, midi) {
            Ok(renderer) => renderer,
            Err(e) => {
                set_entry_status(self.ui, idx, RenderStatus::Failed);
                errors::report(
                    ERROR_TITLE,
                    format!(
                        "Could not set up the renderer for \"{}\":\n\n{e}",
                        label(entry)
                    ),
                );
                return true;
            }
        };

        let renderer = renderer.with_cancel_flag(self.cancel.clone());
        let stats = renderer.get_statistics();
        let registry = self.registry.clone();
        let started = Instant::now();

        let rendering = Arc::new(AtomicBool::new(true));
        let outcome = std::thread::scope(|scope| {
            let progress = {
                let rendering = rendering.clone();
                let stats = stats.clone();
                let registry = registry.clone();
                scope.spawn(move || {
                    while rendering.load(Ordering::Relaxed) {
                        report_progress(&registry, job_id, &stats, total_pos, started);
                        std::thread::sleep(PROGRESS_INTERVAL);
                    }
                })
            };

            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| renderer.render()));
            rendering.store(false, Ordering::Relaxed);
            let _ = progress.join();
            outcome
        });

        match outcome {
            Ok(Ok(RenderOutcome::Completed)) => {
                set_entry_status(self.ui, idx, RenderStatus::Done);
                true
            }
            Ok(Ok(RenderOutcome::Cancelled)) => {
                set_entry_status(self.ui, idx, RenderStatus::Pending);
                false
            }
            // Render returned an error (e.g. a soundfont failed to load):
            // surface it and move on rather than silently producing nothing.
            Ok(Err(e)) => {
                set_entry_status(self.ui, idx, RenderStatus::Failed);
                errors::report(
                    ERROR_TITLE,
                    format!("Failed to render \"{}\":\n\n{e}", label(entry)),
                );
                true
            }
            // Unwound — an unexpected panic (which also trips the fatal
            // screen via the global hook). Stop the run.
            Err(_) => {
                set_entry_status(self.ui, idx, RenderStatus::Pending);
                false
            }
        }
    }
}

fn report_progress(
    registry: &JobRegistry,
    job_id: u64,
    stats: &MaestroFileRendererStatistics,
    total_pos: f64,
    started: Instant,
) {
    let frac = (stats.get_time() / total_pos).clamp(0.0, 1.0);
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
}

/// TODO: make this more accurate
fn eta(elapsed: f64, frac: f64) -> f64 {
    match frac > 0.005 {
        true => elapsed / frac * (1.0 - frac),
        false => 0.0,
    }
}

fn label(entry: &SlintMidiRenderEntry) -> String {
    match entry.entry.as_str() {
        "" => entry.midi_path.to_string(),
        name => format!("{} › {name}", entry.midi_path),
    }
}

fn midi_total_pos(
    midi: Option<&MidiFile>,
    cancel: &AtomicBool,
    on_progress: impl Fn(usize, usize) + Sync,
) -> f64 {
    let Some(midi) = midi else {
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
