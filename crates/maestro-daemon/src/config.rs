use maestro_core::{
    audio_params::AudioParameters,
    realtime::config::RealtimeConfig,
    renderer::config::{EventProcessorConfig, PostProcessorConfig, RendererConfig},
    soundfont::DEFAULT_SOUNDFONT_LIST_NAME,
};
use serde::{Deserialize, Serialize};

pub use maestro_core::system_cfg::system::SystemCustomSettings;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct MaestroSystemConfig {
    pub sflist: String,
    pub audio_params: AudioParameters,
    pub realtime: RealtimeConfig,
    pub renderer: RendererConfig,
    pub event_processor: Option<EventProcessorConfig>,
    pub post_processor: Option<PostProcessorConfig>,
    pub custom: SystemCustomSettings,
}

impl Default for MaestroSystemConfig {
    fn default() -> Self {
        Self {
            sflist: DEFAULT_SOUNDFONT_LIST_NAME.into(),
            audio_params: Default::default(),
            realtime: Default::default(),
            renderer: Default::default(),
            event_processor: None,
            post_processor: Some(Default::default()),
            custom: Default::default(),
        }
    }
}
