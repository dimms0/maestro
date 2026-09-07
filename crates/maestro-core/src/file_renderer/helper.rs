use crate::{audio_params::AudioParameters, error::FileRendererError, renderer::MaestroRenderer};

use super::encoder::AudioEncoder;

pub(super) struct RendererHelper {
    renderer: MaestroRenderer,
    encoder: AudioEncoder,

    audio_buffer: Vec<f32>,
    audio_params: AudioParameters,

    rendered: u64,
}

impl RendererHelper {
    pub fn new(
        audio_params: AudioParameters,
        renderer: MaestroRenderer,
        encoder: AudioEncoder,
    ) -> Self {
        Self {
            renderer,
            encoder,
            audio_buffer: Vec::new(),
            audio_params,
            rendered: 0,
        }
    }

    pub fn renderer_mut(&mut self) -> &mut MaestroRenderer {
        &mut self.renderer
    }

    pub fn render_batch(&mut self) -> Result<(), FileRendererError> {
        let target = self.renderer.clock_position();
        let sample_count = target.saturating_sub(self.rendered) as usize;
        if sample_count == 0 {
            return Ok(());
        }
        self.rendered = target;

        self.audio_buffer.resize(sample_count, 0.0);
        self.renderer.render(&mut self.audio_buffer);

        self.encoder.write_samples(&self.audio_buffer)?;
        self.audio_buffer.clear();

        Ok(())
    }

    pub fn finalize(mut self) -> Result<(), FileRendererError> {
        let sample_count =
            self.audio_params.sample_rate as usize * self.audio_params.channels as usize;
        self.audio_buffer.resize(sample_count, 0.0);
        let stats = self.renderer.get_statistics();

        let max_tail_iters = 60; // give up after ~60 s of tail
        for _ in 0..max_tail_iters {
            self.renderer.render(&mut self.audio_buffer);
            self.encoder.write_samples(&self.audio_buffer)?;
            if stats.read_voice_count() == 0 {
                break;
            }
        }

        self.encoder.finalize()?;

        Ok(())
    }
}
