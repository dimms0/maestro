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

        let mut chain = per_channel(Net::wrap(Box::new(pass())));

        if config.dc_blocker {
            chain = chain >> per_channel(Net::wrap(Box::new(dcblock::<f32>())));
        }

        if let Some(eq) = config.equalizer {
            let bands = lowshelf_hz(120.0, 0.7, db_amp(eq.low_db.clamp(-24.0, 24.0)))
                >> bell_hz(1000.0, 0.7, db_amp(eq.mid_db.clamp(-24.0, 24.0)))
                >> highshelf_hz(6000.0, 0.7, db_amp(eq.high_db.clamp(-24.0, 24.0)));
            chain = chain >> per_channel(Net::wrap(Box::new(bands)));
        }

        chain = chain >> per_channel(Net::wrap(Box::new(mul(config.volume.clamp(0.0, 5.0)))));

        if let Some(cfg) = config.reverb {
            let mix = cfg.mix.clamp(0.0, 1.0);
            let reverb = reverb_stereo(
                cfg.room_size_m.clamp(10.0, 30.0) as f64,
                cfg.time_s.clamp(0.1, 20.0) as f64,
                cfg.damping.clamp(0.0, 1.0) as f64,
            );
            let wet = ((1.0 - mix) * multipass::<U2>()) & (mix * reverb);
            chain = chain
                >> if stereo {
                    Net::wrap(Box::new(wet))
                } else {
                    Net::wrap(Box::new(split::<U2>() >> wet >> join::<U2>()))
                };
        }

        if let Some(cfg) = config.limiter {
            let (attack, release) = (cfg.attack_ms / 1000.0, cfg.release_ms / 1000.0);
            chain = if stereo {
                chain >> limiter_stereo(attack, release)
            } else {
                chain >> limiter(attack, release)
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
