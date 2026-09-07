use crate::{
    audio_params::AudioParameters,
    realtime::config::RealtimeConfig,
    renderer::config::{EventProcessorConfig, PostProcessorConfig, RendererConfig},
};

#[derive(Debug, Clone)]
pub struct RealtimeEngineOptions {
    pub ports: Option<u8>,
    pub config: RealtimeConfig,
    pub renderer: RendererConfig,
    pub audio_params: AudioParameters,
    pub event_processor: Option<EventProcessorConfig>,
    pub post_processor: Option<PostProcessorConfig>,
}

impl Default for RealtimeEngineOptions {
    fn default() -> Self {
        Self {
            ports: None,
            config: Default::default(),
            renderer: Default::default(),
            audio_params: Default::default(),
            event_processor: None,
            post_processor: Some(Default::default()),
        }
    }
}
