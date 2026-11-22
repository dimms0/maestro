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
    pub render_threads: RenderThreads,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderThreads {
    #[default]
    Auto,
    Fixed(usize),
}
