use crate::{
    audio_params::{AudioParameters, ChannelCount},
    error::RendererError,
    renderer::config::{AudioLimiterConfig, PostProcessorConfig},
};
use fundsp::prelude::*;

pub(super) struct AudioPostProcessor {
    volume: f32,
    limiter: Option<StereoAudioLimiter>,
}

impl AudioPostProcessor {
    pub fn new(
        config: PostProcessorConfig,
        audio_params: &AudioParameters,
    ) -> Result<Self, RendererError> {
        let limiter = config
            .limiter
            .map(|cfg| StereoAudioLimiter::new(&cfg, audio_params));

        Ok(Self {
            volume: config.volume.clamp(0.0, 5.0),
            limiter,
        })
    }

    pub fn process(&mut self, buffer: &mut [f32]) {
        if let Some(limiter) = &mut self.limiter {
            limiter.process(buffer);
        }

        for s in buffer.iter_mut() {
            *s *= self.volume;
        }
    }

    pub fn reset(&mut self) {
        if let Some(limiter) = &mut self.limiter {
            limiter.reset();
        }
    }
}

struct StereoAudioLimiter {
    limiter: An<Limiter<U2>>,
    channels: ChannelCount,
}

impl StereoAudioLimiter {
    fn new(config: &AudioLimiterConfig, audio_params: &AudioParameters) -> Self {
        let mut limiter = limiter_stereo(config.attack_ms / 1000.0, config.release_ms / 1000.0);
        limiter.set_sample_rate(audio_params.sample_rate as f64);

        Self {
            limiter,
            channels: audio_params.channels,
        }
    }

    fn process(&mut self, buffer: &mut [f32]) {
        match self.channels {
            ChannelCount::Mono => {
                for frame in buffer.iter_mut() {
                    // filter_mono() didn't work here so we will just use the left
                    // channel of the stereo limiter
                    let (f, _) = self.limiter.filter_stereo(*frame, 0.0);
                    *frame = f;
                }
            }
            ChannelCount::Stereo => {
                for frame in buffer.chunks_mut(2) {
                    let (l, r) = self.limiter.filter_stereo(frame[0], frame[1]);
                    frame[0] = l;
                    frame[1] = r;
                }
            }
        }
    }

    fn reset(&mut self) {
        self.limiter.reset();
    }
}
