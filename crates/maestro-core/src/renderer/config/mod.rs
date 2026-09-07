mod event_processor;
mod post_processor;
mod synth;

pub use event_processor::*;
pub use post_processor::*;
pub use synth::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct RendererConfig {
    pub synth: SynthConfig,
    pub port_threads: Option<usize>,
    pub render_fps: Option<f64>,
    pub render_fps_variation: f32,
}
