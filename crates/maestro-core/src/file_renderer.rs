use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use midi_parser::{Emit, EventRef, FileKind, MidiFile, MidiInfo, TempoMap};

use crate::{
    audio_params::AudioParameters,
    error::{FileRendererError, MaestroComponentError},
    event::{MaestroEvent, MaestroTimedEvent, MidiTranslator, sysex},
    renderer::{
        MaestroRenderer,
        config::{EventProcessorConfig, PostProcessorConfig, RendererConfig},
    },
    soundfont::SoundFontList,
};

mod encoder;
mod helper;
pub use encoder::{
    AudioEncoder, BITRATES_KBPS, DEFAULT_BITRATE_KBPS, OutputFormat, OutputSettings, WavBitDepth,
};
use helper::RendererHelper;
mod statistics;
pub use statistics::MaestroFileRendererStatistics;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderOutcome {
    Completed,
    Cancelled,
}

#[derive(Clone, Copy)]
pub enum PortMode {
    Single,
    Multi { max_ports: Option<u8> },
}

fn unique_output_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|candidate| !candidate.exists())
        .unwrap()
}

pub struct MaestroFileRenderer {
    config: RendererConfig,
    audio_params: AudioParameters,
    soundfonts: SoundFontList,
    midi_path: PathBuf,
    output_dir: PathBuf,
    output_file: Option<PathBuf>,
    output: OutputSettings,

    batch_size: f64,

    evproc: Option<EventProcessorConfig>,
    postproc: Option<PostProcessorConfig>,
    port_mode: PortMode,

    stats: MaestroFileRendererStatistics,
    cancel: Option<Arc<AtomicBool>>,
}

impl MaestroFileRenderer {
    pub fn new(
        config: RendererConfig,
        audio_params: AudioParameters,
        soundfonts: SoundFontList,
        midi_path: impl Into<PathBuf>,
        output_dir: impl Into<PathBuf>,
    ) -> Result<Self, FileRendererError> {
        let midi_path = midi_path.into();
        let output_dir = output_dir.into();

        midi_path.file_name().ok_or(FileRendererError::InvalidFile(
            midi_path.to_string_lossy().to_string(),
        ))?;

        Ok(Self {
            config,
            soundfonts,
            midi_path,
            output_dir,
            output_file: None,
            output: OutputSettings::default(),
            batch_size: 0.1,
            evproc: None,
            postproc: None,
            port_mode: PortMode::Multi { max_ports: None },
            audio_params,
            stats: MaestroFileRendererStatistics::new(),
            cancel: None,
        })
    }

    pub fn with_batch_size(self, batch_size: f64) -> Self {
        Self { batch_size, ..self }
    }

    pub fn with_output_settings(self, output: OutputSettings) -> Self {
        Self { output, ..self }
    }

    /// Writes exactly this file instead of a unique name in the output directory.
    pub fn with_output_file(self, path: impl Into<PathBuf>) -> Self {
        Self {
            output_file: Some(path.into()),
            ..self
        }
    }

    pub fn with_event_processor(self, config: EventProcessorConfig) -> Self {
        Self {
            evproc: Some(config),
            ..self
        }
    }

    pub fn with_post_processor(self, config: PostProcessorConfig) -> Self {
        Self {
            postproc: Some(config),
            ..self
        }
    }

    pub fn with_port_mode(self, port_mode: PortMode) -> Self {
        Self { port_mode, ..self }
    }

    pub fn with_cancel_flag(self, cancel: Arc<AtomicBool>) -> Self {
        Self {
            cancel: Some(cancel),
            ..self
        }
    }

    pub fn get_statistics(&self) -> MaestroFileRendererStatistics {
        self.stats.clone()
    }

    pub fn render(self) -> Result<RenderOutcome, MaestroComponentError> {
        let audio_path = match &self.output_file {
            Some(path) => path.clone(),
            None => {
                let stem = self
                    .midi_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("output");
                std::fs::create_dir_all(&self.output_dir).map_err(FileRendererError::from)?;
                unique_output_path(&self.output_dir, stem, self.output.extension())
            }
        };

        let midi = MidiFile::open(&self.midi_path).map_err(|e| {
            FileRendererError::InvalidFile(format!("{}: {e}", self.midi_path.display()))
        })?;

        let info = midi
            .scan()
            .map_err(|e| FileRendererError::InvalidFile(format!("{e}")))?;

        let mut max_port = 0;
        if let PortMode::Multi { max_ports } = self.port_mode {
            max_port = info.track_ports.iter().copied().max().unwrap_or(0);

            if let Some(max) = max_ports {
                max_port = max_port.min(max).clamp(1, 16);
            }
        }

        let (port_mode, kind) = (self.port_mode, midi.kind());
        let route = move |track_port: u8, group: u8| match (port_mode, kind) {
            (PortMode::Single, _) => 0,
            (PortMode::Multi { .. }, FileKind::Clip) => group.min(max_port),
            (PortMode::Multi { .. }, FileKind::Smf) => track_port.min(max_port),
        };

        let sample_rate = self.audio_params.sample_rate;
        let channels = u64::from(u16::from(self.audio_params.channels));
        let window = ((self.batch_size * f64::from(sample_rate)) as u64).max(1);

        // run a whole window ahead of the synth
        let (snd, rcv) = crossbeam_channel::bounded(2);
        thread::spawn(move || {
            produce(
                &midi,
                &info,
                route,
                usize::from(max_port) + 1,
                window,
                u64::from(sample_rate),
                channels,
                |w| snd.send(w).is_ok(),
            )
        });

        let mut renderer = MaestroRenderer::new(
            max_port + 1,
            self.config,
            &self.audio_params,
            self.evproc,
            self.postproc,
        )?;
        renderer.load_soundfonts(&self.soundfonts)?;
        renderer.set_statistics(self.stats.renderer_handle());

        let stats = &self.stats;

        let encoder = AudioEncoder::new(&audio_path, &self.audio_params, self.output)?;

        let mut helper = RendererHelper::new(self.audio_params, renderer, encoder);

        for window in rcv {
            if self
                .cancel
                .as_ref()
                .is_some_and(|c| c.load(Ordering::Relaxed))
            {
                return Ok(RenderOutcome::Cancelled);
            }

            for (port, events) in window.ports.into_iter().enumerate() {
                helper
                    .renderer_mut()
                    .process_timed_events(port as u8, events.into_iter())?;
            }
            stats.set_time(window.frames as f64 / f64::from(sample_rate));
            stats.add_processed(window.counted, window.notes);
            helper.render_batch(window.frames * channels)?;
        }

        helper.finalize()?;

        Ok(RenderOutcome::Completed)
    }
}

struct Window {
    ports: Vec<Vec<MaestroTimedEvent>>,
    frames: u64,
    counted: u64,
    notes: u64,
}

struct TrackState {
    port: u8,
    translator: MidiTranslator,
    hint: usize,
    counted: u64,
    notes: u64,
}

#[allow(clippy::too_many_arguments)]
fn produce(
    midi: &MidiFile,
    info: &MidiInfo,
    route: impl Fn(u8, u8) -> u8 + Copy + Sync,
    port_count: usize,
    window: u64,
    sample_rate: u64,
    channels: u64,
    mut send: impl FnMut(Window) -> bool,
) {
    let map = TempoMap::new(info.division, &info.tempos, sample_rate);
    let total = map.at(info.total_ticks);

    let Ok(mut merged) = midi.merged_par(port_count, |index| TrackState {
        port: info.track_ports[index],
        translator: MidiTranslator::new(),
        hint: 0,
        counted: 0,
        notes: 0,
    }) else {
        return;
    };

    let visit = |state: &mut TrackState,
                 tick,
                 group,
                 event: EventRef,
                 emit: &mut Emit<MaestroTimedEvent>| {
        state.counted += 1;

        if let EventRef::Meta(m) = event
            && let Some(port) = m.port()
        {
            state.port = port;
        }

        let pos = (map.at_from(tick, &mut state.hint) * channels) as u32;
        let port = usize::from(route(state.port, group));
        let mut push = |event| emit.push(port, MaestroTimedEvent { event, pos });

        match event {
            EventRef::Midi(m) => {
                if m.kind() == 0x90 && m.data2 > 0 {
                    state.notes += 1;
                }
                state.translator.short(0, m.as_short(), push);
            }
            EventRef::SysEx(data) => {
                if let Some(id) = sysex::store(data) {
                    push(MaestroEvent::SystemExclusive { id });
                }
            }
            EventRef::Ump(packet) => state.translator.ump(packet.words(), |_, event| push(event)),
            EventRef::Escape(data) => state.translator.long(0, data, push),
            // Tempo is in the map already, and ports were taken above.
            EventRef::Meta(_) => {}
        }
    };

    let (mut start, mut start_tick) = (0, 0);
    loop {
        let end = start + window;
        let last = end >= total;
        let end_tick = if last {
            u64::MAX
        } else {
            map.first_tick_at(end, start_tick, info.total_ticks)
        };

        let Ok(ports) = merged.window(end_tick, visit) else {
            return;
        };
        let (counted, notes) =
            merged
                .states_mut()
                .iter_mut()
                .fold((0, 0), |(counted, notes), state| {
                    let sums = (counted + state.counted, notes + state.notes);
                    (state.counted, state.notes) = (0, 0);
                    sums
                });

        let frames = if last { total } else { end };
        if !send(Window {
            ports,
            frames,
            counted,
            notes,
        }) || last
        {
            return;
        }
        (start, start_tick) = (end, end_tick);
    }
}
