use std::{collections::HashMap, sync::Arc, time::Instant};

use crate::{
    audio_params::AudioParameters,
    error::RendererError,
    event::{MaestroTimedEvent, sysex},
    helpers::{prepapre_cache_vec, sum_buffer},
    renderer::{
        config::{EventProcessorConfig, PostProcessorConfig, RendererConfig, SynthConfig},
        event_processor::PortEventProcessor,
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

pub mod config;
mod event_processor;
mod port;
mod post_processor;
pub mod sender;
mod synth;

pub use synth::probe::{LibraryStatus, probe_libraries};

use rayon::iter::{IndexedParallelIterator, IntoParallelRefMutIterator, ParallelIterator};

pub(crate) struct MaestroRenderer {
    library: Arc<dyn SynthLibrary>,
    ports: Box<[PortRenderer]>,
    render_buffers: Box<[Vec<f32>]>,
    port_soundfonts: Box<[Vec<SoundFontHandle>]>,

    config: RendererConfig,
    evproc: Option<EventProcessorConfig>,
    postproc: Option<PostProcessorConfig>,
    audio_params: AudioParameters,
    post_proc: Option<AudioPostProcessor>,

    stats: Arc<MaestroRenderStatistics>,
    audio_dur_div: f32,
    audio_channels: usize,

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
                    let port = PortSynthManager::<BASSMIDISynth>::new(
                        i,
                        newsynth,
                        evproc.clone(),
                        audio_params,
                    );
                    ports.push(PortRenderer::BASSMIDI(port));
                }

                library
            }
            SynthConfig::FluidSynth(cfg) => {
                let library = FluidSynthLib::load()?;

                for i in 0..port_count {
                    let newsynth = FluidSynthSynth::new(library.clone(), &cfg, audio_params)?;
                    let port = PortSynthManager::<FluidSynthSynth>::new(
                        i,
                        newsynth,
                        evproc.clone(),
                        audio_params,
                    );
                    ports.push(PortRenderer::FluidSynth(port));
                }

                library
            }
        };

        let audio_dur_div =
            audio_params.sample_rate as f32 * u16::from(audio_params.channels) as f32;

        let threadpool = build_threadpool(config.port_threads, port_count as usize)?;
        let post_proc = postproc
            .map(|c| AudioPostProcessor::new(c, audio_params))
            .transpose()?;

        Ok(Self {
            library,
            ports: ports.into_boxed_slice(),
            render_buffers: vec![Vec::new(); port_count as usize].into_boxed_slice(),
            port_soundfonts: vec![Vec::new(); port_count as usize].into_boxed_slice(),
            config,
            evproc,
            postproc,
            audio_params: *audio_params,
            post_proc,
            stats: Arc::new(MaestroRenderStatistics::new()),
            audio_dur_div,
            audio_channels: audio_params.channels as usize,
            threadpool,
        })
    }

    pub fn process_timed_events<I>(&mut self, port: u8, events: I) -> Result<(), RendererError>
    where
        I: Iterator<Item = MaestroTimedEvent>,
    {
        let Some(portmgr) = self.ports.get_mut(port as usize) else {
            for event in events {
                sysex::release(&event.event);
            }

            return Err(RendererError::InvalidPort(port));
        };

        for event in events {
            portmgr.process_event(event);
        }

        Ok(())
    }

    pub fn render(&mut self, buffer: &mut [f32]) -> usize {
        let start = Instant::now();

        buffer.fill(0.0);
        let render_len = buffer.len();
        if render_len < self.audio_channels || !render_len.is_multiple_of(self.audio_channels) {
            return 0;
        }

        if let Some(pool) = &self.threadpool {
            let ports = &mut self.ports;
            let buffers = &mut self.render_buffers;

            pool.install(|| {
                ports
                    .par_iter_mut()
                    .zip(buffers.par_iter_mut())
                    .for_each(|(port, buf)| {
                        prepapre_cache_vec(buf, render_len, 0.0);
                        port.read_audio(buf);
                    });

                for buf in buffers {
                    sum_buffer(buf, buffer);
                }
            });
        } else {
            for (port, port_buffer) in self.ports.iter_mut().zip(self.render_buffers.iter_mut()) {
                prepapre_cache_vec(port_buffer, render_len, 0.0);
                port.read_audio(port_buffer);
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
                    match self.library.load_soundfont_handle(&self.config.synth, sf) {
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

    pub(crate) fn reload_config(
        &mut self,
        config: RendererConfig,
        evproc: Option<EventProcessorConfig>,
        postproc: Option<PostProcessorConfig>,
    ) -> Result<bool, RendererError> {
        if config.synth != self.config.synth {
            // every port runs the same synth so the first one speaks for all
            let Some((first, rest)) = self.ports.split_first_mut() else {
                return Ok(false);
            };
            if !first.reload_config(&config.synth) {
                return Ok(false);
            }
            for port in rest {
                port.reload_config(&config.synth);
            }
        }

        if config.port_threads != self.config.port_threads {
            self.threadpool = build_threadpool(config.port_threads, self.ports.len())?;
        }

        if postproc != self.postproc {
            self.post_proc = postproc
                .map(|c| AudioPostProcessor::new(c, &self.audio_params))
                .transpose()?;
        }

        if evproc != self.evproc {
            for (i, port) in self.ports.iter_mut().enumerate() {
                let processor = evproc
                    .clone()
                    .map(|c| PortEventProcessor::new(i as u8, c, &self.audio_params));
                port.get_buffer().set_event_processor(processor);
                port.reset();
            }
        }

        self.config = config;
        self.evproc = evproc;
        self.postproc = postproc;
        Ok(true)
    }

    pub fn get_statistics(&self) -> Arc<MaestroRenderStatistics> {
        self.stats.clone()
    }

    pub(crate) fn set_statistics(&mut self, stats: Arc<MaestroRenderStatistics>) {
        self.stats = stats;
    }

    pub(crate) fn get_port_senders(&self) -> Vec<Arc<PortEventSender>> {
        self.ports.iter().map(|p| p.get_buffer()).collect()
    }
}

fn build_threadpool(
    threads: Option<usize>,
    port_count: usize,
) -> Result<Option<rayon::ThreadPool>, RendererError> {
    match threads {
        Some(threads) if threads > 1 => Ok(Some(
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads.min(port_count))
                .build()?,
        )),
        _ => Ok(None),
    }
}
