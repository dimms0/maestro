use std::{
    path::{Path, PathBuf},
    thread,
};

use midi_parser::{EventRef, FileKind, MidiFile};

use crate::{
    audio_params::AudioParameters,
    error::{FileRendererError, MaestroComponentError},
    event::{MaestroEvent, MidiTranslator},
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
    output: OutputSettings,

    batch_size: f64,

    evproc: Option<EventProcessorConfig>,
    postproc: Option<PostProcessorConfig>,
    port_mode: PortMode,
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
            output: OutputSettings::default(),
            batch_size: 0.1,
            evproc: None,
            postproc: None,
            port_mode: PortMode::Multi { max_ports: None },
            audio_params,
        })
    }

    pub fn with_batch_size(self, batch_size: f64) -> Self {
        Self { batch_size, ..self }
    }

    pub fn with_output_settings(self, output: OutputSettings) -> Self {
        Self { output, ..self }
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

    pub fn render(
        self,
        mut status_callback: Option<&mut dyn FnMut(&MaestroFileRendererStatistics)>,
    ) -> Result<(), MaestroComponentError> {
        let stem = self
            .midi_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        std::fs::create_dir_all(&self.output_dir).map_err(FileRendererError::from)?;
        let audio_path = unique_output_path(&self.output_dir, stem, self.output.extension());

        let midi = MidiFile::open(&self.midi_path).map_err(|e| {
            FileRendererError::InvalidFile(format!("{}: {e}", self.midi_path.display()))
        })?;

        let info = midi
            .scan()
            .map_err(|e| FileRendererError::InvalidFile(format!("{e}")))?;

        let mut max_port = 0;
        let mut port_maps = vec![0u8; info.track_ports.len().max(1)];
        if let PortMode::Multi { max_ports } = self.port_mode {
            port_maps.copy_from_slice(&info.track_ports);
            max_port = info.track_ports.iter().copied().max().unwrap_or(0);

            if let Some(max) = max_ports {
                max_port = max_port.min(max).clamp(1, 16);
            }
        }

        let (snd, rcv) = crossbeam_channel::bounded(1000);
        let midi_path = self.midi_path.clone();
        thread::spawn(move || {
            let Ok(midi) = MidiFile::open(&midi_path) else {
                return;
            };
            let Ok(mut merged) = midi.merged() else {
                return;
            };

            let translator = MidiTranslator::new();
            let mut batch = Vec::new();
            let mut events = Vec::new();
            let mut pending = 0u64;

            while let Some(Ok((delta, track, group))) = merged.next_batch(&mut batch) {
                pending += delta;

                let mut notes = 0u64;
                let mut tempo = None;
                let mut port = None;

                events.clear();
                for event in &batch {
                    match *event {
                        EventRef::Midi(m) => {
                            if m.kind() == 0x90 && m.data2 > 0 {
                                notes += 1;
                            }
                            translator.short(0, m.as_short(), |event| events.push(event));
                        }

                        EventRef::SysEx(data) => {
                            events.push(MaestroEvent::SystemExclusive(data.into()));
                        }

                        EventRef::Meta(m) => {
                            if let Some(micros) = m.tempo() {
                                tempo = Some(micros);
                            }
                            if let Some(assigned) = m.port() {
                                port = Some(assigned);
                            }
                        }

                        EventRef::Ump(packet) => {
                            if let Some(micros) = packet.tempo() {
                                tempo = Some(micros);
                            }
                            translator.ump(packet.words(), |_, event| events.push(event));
                        }

                        EventRef::Escape(data) => {
                            translator.long(0, data, |event| events.push(event));
                        }
                    }
                }

                let batch = RenderBatch {
                    ticks: pending,
                    track,
                    group,
                    events: std::mem::take(&mut events),
                    counted: batch.len() as u64,
                    notes,
                    tempo,
                    port,
                };

                pending = 0;
                if snd.send(batch).is_err() {
                    break;
                }
            }
        });

        let mut renderer = MaestroRenderer::new(
            max_port + 1,
            self.config,
            &self.audio_params,
            self.evproc,
            self.postproc,
        )?;
        renderer.load_soundfonts(&self.soundfonts)?;
        renderer.set_clock_division(midi.division());

        let mut stats = MaestroFileRendererStatistics::new(renderer.get_statistics());

        let encoder = AudioEncoder::new(&audio_path, &self.audio_params, self.output)?;

        let mut helper = RendererHelper::new(self.audio_params, renderer, encoder);

        let samples_per_second =
            self.audio_params.sample_rate as f64 * u16::from(self.audio_params.channels) as f64;
        let mut last_render = 0.0;

        for batch in rcv {
            if let Some(port) = batch.port
                && let Some(slot) = port_maps.get_mut(batch.track as usize)
            {
                *slot = port;
            }

            let port = match (self.port_mode, midi.kind()) {
                (PortMode::Single, _) => 0,
                (PortMode::Multi { .. }, FileKind::Clip) => batch.group.min(max_port),
                (PortMode::Multi { .. }, FileKind::Smf) => port_maps
                    .get(batch.track as usize)
                    .copied()
                    .unwrap_or(0)
                    .min(max_port),
            };

            if let Some(micros) = batch.tempo {
                helper.renderer_mut().set_clock_tempo(micros);
            }

            helper.renderer_mut().process_events_ticks(
                port,
                batch.events.into_iter(),
                batch.ticks,
            )?;

            let pos = helper.renderer_mut().clock_position() as f64 / samples_per_second;
            stats.set_time(pos);
            stats.add_processed(batch.counted, batch.notes);

            if pos - last_render > self.batch_size {
                helper.render_batch()?;
                last_render = pos;

                if let Some(cb) = &mut status_callback {
                    cb(&stats);
                }
            }
        }

        helper.render_batch()?;
        helper.finalize()?;

        Ok(())
    }
}

struct RenderBatch {
    ticks: u64,
    track: u32,
    group: u8,
    events: Vec<MaestroEvent>,
    counted: u64,
    notes: u64,
    tempo: Option<u32>,
    port: Option<u8>,
}
