use std::{collections::HashMap, sync::Arc, time::Instant};

use midi_parser::Division;

use crate::{
    audio_params::AudioParameters,
    error::RendererError,
    event::{MaestroEvent, MaestroTimedEvent},
    helpers::{Jitter, prepapre_cache_vec, sum_buffer},
    renderer::{
        clock::RendererClock,
        config::{EventProcessorConfig, PostProcessorConfig, RendererConfig, SynthConfig},
        port::{PortRenderer, PortSynthManager},
        post_processor::AudioPostProcessor,
        sender::PortEventSender,
        synth::{
            SoundFontHandle, SynthLibrary,
            bassmidi::{BASSMIDILib, BASSMIDISynth},
            fluidsynth::{FluidSynthLib, FluidSynthSynth},
        },
    },
    soundfont::{SoundFont, SoundFontList},
    statistics::MaestroRenderStatistics,
};

mod clock;
pub mod config;
mod event_processor;
mod port;
mod post_processor;
pub mod sender;
mod synth;

pub use synth::probe::{LibraryStatus, probe_libraries};

pub(crate) use clock::RealtimeClock;
use rayon::iter::{IndexedParallelIterator, IntoParallelRefMutIterator, ParallelIterator};

pub(crate) struct MaestroRenderer {
    library: Arc<dyn SynthLibrary>,
    ports: Box<[PortRenderer]>,
    render_buffers: Box<[Vec<f32>]>,
    port_soundfonts: Box<[Vec<SoundFontHandle>]>,

    config: SynthConfig,
    clock: RendererClock,
    rt_clock: Option<Arc<RealtimeClock>>,
    post_proc: Option<AudioPostProcessor>,

    stats: Arc<MaestroRenderStatistics>,
    audio_dur_div: f32,
    audio_channels: usize,
    precision_threshold: usize,
    precision_jitter: Jitter,

    threadpool: Option<rayon::ThreadPool>,
}

impl MaestroRenderer {
    pub fn new(
        port_count: u8,
        config: RendererConfig,
        audio_params: &AudioParameters,
        evproc: Option<EventProcessorConfig>,
        postproc: Option<PostProcessorConfig>,
    ) -> Result<Self, RendererError> {
        let mut ports = Vec::new();
        let library: Arc<dyn SynthLibrary> = match config.synth {
            SynthConfig::BASSMIDI(cfg) => {
                let library = BASSMIDILib::load()?;

                for i in 0..port_count {
                    let newsynth = BASSMIDISynth::new(library.clone(), &cfg, audio_params)?;
                    let port = PortSynthManager::<BASSMIDISynth>::new(i, newsynth, evproc.clone());
                    ports.push(PortRenderer::BASSMIDI(port));
                }

                library
            }
            SynthConfig::FluidSynth(cfg) => {
                let library = FluidSynthLib::load()?;

                for i in 0..port_count {
                    let newsynth = FluidSynthSynth::new(library.clone(), &cfg, audio_params)?;
                    let port =
                        PortSynthManager::<FluidSynthSynth>::new(i, newsynth, evproc.clone());
                    ports.push(PortRenderer::FluidSynth(port));
                }

                library
            }
        };

        let audio_dur_div =
            audio_params.sample_rate as f32 * u16::from(audio_params.channels) as f32;

        let threadpool = if let Some(threads) = config.port_threads
            && threads > 1
        {
            let tp = rayon::ThreadPoolBuilder::new()
                .num_threads(threads.min(port_count as usize))
                .build()?;
            Some(tp)
        } else {
            None
        };

        let post_proc = if let Some(config) = postproc {
            Some(AudioPostProcessor::new(config, audio_params)?)
        } else {
            None
        };

        let precision_threshold = config
            .render_fps
            .map(|t| (audio_dur_div as f64 * (1.0 / t)) as usize)
            .unwrap_or(0);
        let precision_jitter = Jitter::new(config.render_fps_variation);

        Ok(Self {
            library,
            ports: ports.into_boxed_slice(),
            render_buffers: vec![Vec::new(); port_count as usize].into_boxed_slice(),
            port_soundfonts: vec![Vec::new(); port_count as usize].into_boxed_slice(),
            config: config.synth,
            clock: RendererClock::new(audio_params),
            rt_clock: None,
            // ev_proc: evproc.map(|c| EventProcessor::new(port_count, c)),
            post_proc,
            stats: Arc::new(MaestroRenderStatistics::new()),
            audio_dur_div,
            audio_channels: audio_params.channels as usize,
            precision_threshold,
            precision_jitter,
            threadpool,
        })
    }

    pub(crate) fn set_realtime_clock(&mut self, clock: Arc<RealtimeClock>) {
        self.rt_clock = Some(clock);
    }

    pub(crate) fn clock_position(&self) -> u64 {
        self.clock.get_position()
    }

    pub(crate) fn set_clock_division(&mut self, division: Division) {
        self.clock.set_division(division);
    }

    pub(crate) fn set_clock_tempo(&mut self, micros_per_quarter: u32) {
        self.clock.set_tempo(micros_per_quarter);
    }

    pub fn process_events_ticks<I>(
        &mut self,
        port: u8,
        events: I,
        ticks: u64,
    ) -> Result<(), RendererError>
    where
        I: Iterator<Item = MaestroEvent>,
    {
        self.clock.advance_by_ticks(ticks);
        let pos = self.clock.get_position();
        self.process_timed_events(port, events.map(|e| MaestroTimedEvent { event: e, pos }))
    }

    pub fn process_timed_events<I>(&mut self, port: u8, events: I) -> Result<(), RendererError>
    where
        I: Iterator<Item = MaestroTimedEvent>,
    {
        let portmgr = self
            .ports
            .get_mut(port as usize)
            .ok_or(RendererError::InvalidPort(port))?;

        for event in events {
            portmgr.process_event(event);
        }

        Ok(())
    }

    fn block_precision_threshold(&mut self) -> usize {
        if self.precision_threshold == 0 || !self.precision_jitter.active() {
            return self.precision_threshold;
        }

        self.precision_jitter
            .apply(self.precision_threshold as f32)
            .max(0.0) as usize
    }

    pub fn render(&mut self, buffer: &mut [f32]) -> usize {
        let start = Instant::now();

        buffer.fill(0.0);
        let render_len = buffer.len();
        if render_len < self.audio_channels || !render_len.is_multiple_of(self.audio_channels) {
            return 0;
        }

        if let Some(clock) = &self.rt_clock {
            clock.begin_block(render_len as u64);
        }

        let precision_threshold = self.block_precision_threshold();

        if let Some(pool) = &self.threadpool {
            let ports = &mut self.ports;
            let buffers = &mut self.render_buffers;

            pool.install(|| {
                ports
                    .par_iter_mut()
                    .zip(buffers.par_iter_mut())
                    .for_each(|(port, buf)| {
                        prepapre_cache_vec(buf, render_len, 0.0);
                        port.read_audio(buf, precision_threshold);
                    });

                for buf in buffers {
                    sum_buffer(buf, buffer);
                }
            });
        } else {
            for (port, port_buffer) in self.ports.iter_mut().zip(self.render_buffers.iter_mut()) {
                prepapre_cache_vec(port_buffer, render_len, 0.0);
                port.read_audio(port_buffer, precision_threshold);
            }

            for buf in self.render_buffers.iter_mut() {
                sum_buffer(buf, buffer);
            }
        }

        if let Some(p) = &mut self.post_proc {
            p.process(buffer);
        }

        let render_duration = start.elapsed().as_secs_f32();
        let audio_duration = render_len as f32 / self.audio_dur_div;
        self.stats
            .add_render_time(100.0 * render_duration / audio_duration);
        self.stats
            .set_voices(self.ports.iter().map(|p| p.voice_count()).sum());

        render_len
    }

    pub fn load_soundfonts(&mut self, list: &SoundFontList) -> Result<(), RendererError> {
        for old_port in &mut self.port_soundfonts {
            for handle in old_port.drain(..) {
                self.library.free_soundfont_handle(handle);
            }
        }
        let num_ports = self.ports.len();

        let load_handles =
            |filtered: &[&SoundFont]| -> Result<Vec<SoundFontHandle>, RendererError> {
                let mut vec = Vec::new();

                for sf in filtered {
                    match self.library.load_soundfont_handle(&self.config, sf) {
                        Ok(handle) => vec.push(handle),
                        Err(err) => {
                            for handle in vec.drain(..) {
                                self.library.free_soundfont_handle(handle);
                            }
                            return Err(err);
                        }
                    }
                }

                Ok(vec)
            };

        let mut apply_sfs_to_port =
            |idx: usize, handles: &[SoundFontHandle]| -> Result<(), RendererError> {
                let res = self.ports[idx].set_soundfonts(handles);

                if res.is_ok() {
                    self.port_soundfonts[idx].extend_from_slice(handles);
                }

                res
            };

        let mut loaded_lists: HashMap<&str, Vec<SoundFontHandle>> = HashMap::new();

        for i in 0..num_ports {
            let list_name = list.port_assignments.get(&i).or(list.default_list.as_ref());

            if let Some(name) = list_name {
                if !loaded_lists.contains_key(name.as_str()) {
                    let sfs = list.lists.get(name).ok_or_else(|| {
                        RendererError::SoundFont(format!("Sub-list '{}' not found", name))
                    })?;
                    let supported = self.library.supported_soundfont_types();

                    let filtered: Vec<_> = sfs
                        .iter()
                        .rev()
                        .filter(|sf| {
                            if !sf.enabled {
                                return false;
                            }
                            let ty = sf.font_type();
                            supported.contains(&ty)
                        })
                        .collect();
                    let handles = load_handles(&filtered)?;
                    loaded_lists.insert(name, handles);
                }

                if let Some(handles) = loaded_lists.get(name.as_str()) {
                    apply_sfs_to_port(i, handles)?;
                }
            }
        }

        Ok(())
    }

    pub fn reset(&mut self) {
        for port in &mut self.ports {
            port.reset();
        }

        if let Some(p) = &mut self.post_proc {
            p.reset();
        }
    }

    pub fn get_statistics(&self) -> Arc<MaestroRenderStatistics> {
        self.stats.clone()
    }

    pub(crate) fn get_port_senders(&self) -> Vec<Arc<PortEventSender>> {
        self.ports.iter().map(|p| p.get_buffer()).collect()
    }
}
