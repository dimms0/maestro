use crate::{
    audio_params::{AudioParameters, ChannelCount},
    error::RendererError,
    renderer::config::PostProcessorConfig,
};
use fundsp::prelude::*;

pub(super) struct AudioPostProcessor {
    chain: Net,
    channels: ChannelCount,
}

impl AudioPostProcessor {
    pub fn new(
        config: PostProcessorConfig,
        audio_params: &AudioParameters,
    ) -> Result<Self, RendererError> {
        let stereo = audio_params.channels == ChannelCount::Stereo;
        let per_channel = |unit: Net| if stereo { unit.clone() | unit } else { unit };

        let mut chain = per_channel(Net::wrap(Box::new(mul(config.volume.clamp(0.0, 5.0)))));

        if config.dc_blocker {
            chain = per_channel(Net::wrap(Box::new(dcblock::<f32>()))) >> chain;
        }

        if let Some(cfg) = config.limiter {
            let (attack, release) = (cfg.attack_ms / 1000.0, cfg.release_ms / 1000.0);
            chain = if stereo {
                limiter_stereo(attack, release) >> chain
            } else {
                limiter(attack, release) >> chain
            };
        }

        chain.set_sample_rate(audio_params.sample_rate as f64);
        chain.allocate();

        Ok(Self {
            chain,
            channels: audio_params.channels,
        })
    }

    pub fn process(&mut self, buffer: &mut [f32]) {
        match self.channels {
            ChannelCount::Mono => {
                for s in buffer.iter_mut() {
                    *s = self.chain.filter_mono(*s);
                }
            }
            ChannelCount::Stereo => {
                for frame in buffer.chunks_mut(2) {
                    (frame[0], frame[1]) = self.chain.filter_stereo(frame[0], frame[1]);
                }
            }
        }
    }

    pub fn reset(&mut self) {
        self.chain.reset();
    }
}
