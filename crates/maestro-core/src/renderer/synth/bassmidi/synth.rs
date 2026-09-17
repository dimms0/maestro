use std::sync::Arc;

use rayon::iter::{IndexedParallelIterator, IntoParallelRefMutIterator, ParallelIterator};

use crate::{
    audio_params::AudioParameters,
    error::RendererError,
    event::{MaestroEvent, MaestroTimedEvent, sysex},
    helpers::{prepapre_cache_vec, sum_buffer},
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
    threadpool: Option<rayon::ThreadPool>,
}

impl BASSMIDISynth {
    pub fn new(
        lib: Arc<BASSMIDILib>,
        config: &BASSMIDIConfig,
        audio_params: &AudioParameters,
    ) -> Result<Self, RendererError> {
        let mut streams = Vec::new();

        let kbdiv;
        let instances;
        let threadpool;

        if let Some(mtcfg) = config.multithreading
            && (mtcfg.thread_count > 1 || mtcfg.thread_count == 0)
        {
            let threads = if mtcfg.thread_count == 0 {
                std::thread::available_parallelism()
                    .map(|v| v.get())
                    .unwrap_or(4)
            } else {
                mtcfg.thread_count.min(128) as usize
            };

            kbdiv = if mtcfg.divide_channels {
                threads / 16
            } else {
                0
            };
            instances = if kbdiv > 0 { 16 * kbdiv } else { threads };

            threadpool = Some(
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()?,
            );
        } else {
            kbdiv = 0;
            instances = 1;
            threadpool = None;
        }

        for i in 0..instances {
            let mut stream = BASSMIDIStream::initialize(lib.clone(), config, audio_params)?;

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
            threadpool,
        })
    }
}

impl SynthModule for BASSMIDISynth {
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

        if let Some(pool) = &self.threadpool {
            let streams = &mut self.streams;
            let buffers = &mut self.buffers;

            pool.install(|| {
                streams
                    .par_iter_mut()
                    .zip(buffers.par_iter_mut())
                    .for_each(|(s, b)| {
                        prepapre_cache_vec(b, render_len, 0.0);
                        s.read_audio(b);
                    });

                for buf in buffers.iter_mut() {
                    sum_buffer(buf, buffer);
                }
            });
        } else {
            self.streams[0].read_audio(buffer);
        }
    }

    fn voice_count(&self) -> u64 {
        self.streams.iter().map(|s| s.voice_count()).sum()
    }
}
