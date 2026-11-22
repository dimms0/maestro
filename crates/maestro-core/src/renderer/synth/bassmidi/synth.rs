use std::sync::Arc;

use rayon::iter::{IndexedParallelIterator, IntoParallelRefMutIterator, ParallelIterator};

use crate::{
    audio_params::AudioParameters,
    error::RendererError,
    event::{MaestroEvent, MaestroTimedEvent, sysex},
    helpers::{prepapre_cache_vec, sum_buffer},
    realtime::priority::Priority,
    renderer::{
        SoundFontHandle,
        config::bassmidi::BASSMIDIConfig,
        synth::{
            SynthModule,
            bassmidi::{BASSMIDILib, BASSMIDIStream},
        },
    },
};

pub(crate) struct BASSMIDISynth {
    streams: Box<[BASSMIDIStream]>,
    buffers: Box<[Vec<f32>]>,

    divisor: usize,
    kbdiv: usize,
    parallel: bool,
    config: BASSMIDIConfig,
}

impl BASSMIDISynth {
    pub fn new(
        lib: Arc<BASSMIDILib>,
        config: &BASSMIDIConfig,
        audio_params: &AudioParameters,
    ) -> Result<Self, RendererError> {
        let mut streams = Vec::new();

        let threads = parallel_threads(config);
        let kbdiv = match config.multithreading {
            Some(mtcfg) if mtcfg.divide_channels => threads / 16,
            _ => 0,
        };
        let instances = if kbdiv > 0 {
            16 * kbdiv
        } else {
            threads.max(1)
        };

        for i in 0..instances {
            let stream_config = split_voices(config, i, instances);
            let mut stream = BASSMIDIStream::initialize(lib.clone(), &stream_config, audio_params)?;

            if kbdiv > 0 && i / kbdiv == 9 {
                stream.set_drum_channel(9, true);
            }

            streams.push(stream);
        }

        Ok(Self {
            streams: streams.into_boxed_slice(),
            buffers: vec![Vec::new(); instances].into_boxed_slice(),
            divisor: if kbdiv == 0 { instances } else { kbdiv },
            kbdiv,
            parallel: threads > 1,
            config: *config,
        })
    }
}

pub(crate) fn parallel_threads(config: &BASSMIDIConfig) -> usize {
    match config.multithreading.map(|m| m.thread_count) {
        None | Some(1) => 0,
        Some(0) => std::thread::available_parallelism()
            .map(|v| v.get())
            .unwrap_or(4),
        Some(count) => count.min(128) as usize,
    }
}

fn split_voices(config: &BASSMIDIConfig, index: usize, instances: usize) -> BASSMIDIConfig {
    let instances = instances as u32;
    let index = index as u32;
    let base = config.voice_limit / instances;
    let extra = u32::from(index < config.voice_limit % instances);

    BASSMIDIConfig {
        voice_limit: (base + extra).max(1),
        ..*config
    }
}

impl SynthModule for BASSMIDISynth {
    type Config = BASSMIDIConfig;

    fn reload_config(&mut self, config: &BASSMIDIConfig) -> bool {
        // Threading decides the stream layout and the `sf_*` flags were baked
        // into the loaded soundfonts, everything else is a stream setting
        let fixed = |c: &BASSMIDIConfig| BASSMIDIConfig {
            voice_limit: 0,
            render_time_limit: 0.0,
            interpolation: Default::default(),
            disable_effects: false,
            fade_out_killing: false,
            note_off1: false,
            exclusive_keys: 0,
            ..*c
        };
        if fixed(&self.config) != fixed(config) {
            return false;
        }

        let instances = self.streams.len();
        for (i, stream) in self.streams.iter_mut().enumerate() {
            stream.reload_config(&split_voices(config, i, instances));
        }
        self.config = *config;
        true
    }

    fn set_soundfonts(&mut self, handles: &[SoundFontHandle]) -> Result<(), RendererError> {
        for stream in &mut self.streams {
            stream.set_soundfonts(handles)?;
        }

        Ok(())
    }

    fn reset(&mut self) {
        for synth in &mut self.streams {
            synth.reset();
        }
    }

    fn process_event(&mut self, event: MaestroTimedEvent) {
        match event.event {
            MaestroEvent::NoteOn { channel, key, .. } | MaestroEvent::NoteOff { channel, key } => {
                let idx = channel as usize * self.kbdiv + key as usize % self.divisor;
                self.streams[idx].process_event(event);
            }

            MaestroEvent::ControlChange { channel, .. }
            | MaestroEvent::PitchBendChange { channel, .. }
            | MaestroEvent::ProgramChange { channel, .. }
            | MaestroEvent::ChannelAftertouch { channel, .. }
            | MaestroEvent::PolyphonicAftertouch { channel, .. } => {
                for i in 0..self.divisor {
                    let idx = channel as usize * self.kbdiv + i;
                    self.streams[idx].process_event(event);
                }
            }
            _ => {
                if let MaestroEvent::SystemExclusive { id } = event.event {
                    sysex::retain(id, self.streams.len() - 1);
                }

                for stream in &mut self.streams {
                    stream.process_event(event);
                }
            }
        }
    }

    fn read_audio(&mut self, buffer: &mut [f32]) {
        let render_len = buffer.len();

        if self.parallel {
            let priority = Priority::current();
            self.streams
                .par_iter_mut()
                .zip(self.buffers.par_iter_mut())
                .for_each(|(s, b)| {
                    if let Some(priority) = priority {
                        priority.promote();
                    }
                    prepapre_cache_vec(b, render_len, 0.0);
                    s.read_audio(b);
                });

            for buf in self.buffers.iter_mut() {
                sum_buffer(buf, buffer);
            }
        } else {
            self.streams[0].read_audio(buffer);
        }
    }

    fn voice_count(&self) -> u64 {
        self.streams.iter().map(|s| s.voice_count()).sum()
    }
}
