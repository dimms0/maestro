use maestro_core::{
    audio_params::AudioParameters,
    realtime::config::RealtimeConfig,
    renderer::config::{EventProcessorConfig, PostProcessorConfig, RendererConfig},
    soundfont::DEFAULT_SOUNDFONT_LIST_NAME,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct MaestroKdmapiConfig {
    pub enabled: bool,
    pub sflist: String,
    pub audio_params: AudioParameters,
    pub realtime: RealtimeConfig,
    pub renderer: RendererConfig,
    pub event_processor: Option<EventProcessorConfig>,
    pub post_processor: Option<PostProcessorConfig>,
}

impl Default for MaestroKdmapiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sflist: DEFAULT_SOUNDFONT_LIST_NAME.into(),
            audio_params: Default::default(),
            realtime: Default::default(),
            renderer: Default::default(),
            event_processor: None,
            post_processor: Some(Default::default()),
        }
    }
}
