use std::path::PathBuf;

use maestro_core::{
    audio_params::AudioParameters,
    error::ConfigError,
    realtime::{RealtimeEngineOptions, config::RealtimeConfig},
    renderer::config::{EventProcessorConfig, PostProcessorConfig, RendererConfig},
    soundfont::DEFAULT_SOUNDFONT_LIST_NAME,
    system_cfg::{ConfigComponent, MaestroConfigManager},
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct ApiConfig {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    pub sflist: String,
    pub audio_params: AudioParameters,
    pub realtime: RealtimeConfig,
    pub renderer: RendererConfig,
    pub event_processor: Option<EventProcessorConfig>,
    pub post_processor: Option<PostProcessorConfig>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            app_name: None,
            sflist: DEFAULT_SOUNDFONT_LIST_NAME.into(),
            audio_params: Default::default(),
            realtime: Default::default(),
            renderer: Default::default(),
            event_processor: None,
            post_processor: Some(Default::default()),
        }
    }
}

impl ApiConfig {
    pub fn engine_options(&self, ports: u8) -> RealtimeEngineOptions {
        RealtimeEngineOptions {
            ports: Some(ports),
            config: self.realtime.clone(),
            renderer: self.renderer,
            audio_params: self.audio_params,
            event_processor: self.event_processor.clone(),
            post_processor: self.post_processor,
        }
    }

    pub fn same_engine(&self, other: &Self) -> bool {
        self.audio_params == other.audio_params
            && self.realtime == other.realtime
            && self.renderer == other.renderer
            && self.event_processor == other.event_processor
            && self.post_processor == other.post_processor
    }
}

pub fn global_path(manager: &MaestroConfigManager) -> PathBuf {
    manager.get_component_config_path(&ConfigComponent::Api)
}

pub fn load_global(manager: &MaestroConfigManager) -> Result<ApiConfig, ConfigError> {
    manager.load_or_default(&global_path(manager))
}

pub fn load_app(manager: &MaestroConfigManager, app_id: &str) -> Result<ApiConfig, ConfigError> {
    manager.load_json(&manager.get_app_config_path(app_id))
}

pub fn ensure_app(
    manager: &MaestroConfigManager,
    app_id: &str,
    app_name: Option<&str>,
) -> Result<(), ConfigError> {
    let path = manager.get_app_config_path(app_id);
    if path.exists() {
        return Ok(());
    }

    let mut config = load_global(manager)?;
    config.enabled = true;
    config.app_name = Some(app_name.unwrap_or(app_id).to_string());
    manager.save_json(&path, &config)
}
