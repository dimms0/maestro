use thiserror::Error;

#[derive(Error, Debug)]
pub enum DaemonError {
    #[error("Configuration error: {0}")]
    Config(#[from] maestro_core::error::ConfigError),

    #[error("Engine error: {0}")]
    Engine(#[from] maestro_core::error::MaestroComponentError),

    #[error("Renderer error: {0}")]
    Renderer(#[from] maestro_core::error::RendererError),

    #[error("File watcher error: {0}")]
    Watcher(#[from] maestro_core::system_cfg::watcher::notify::Error),

    #[error("MIDI backend error: {0}")]
    Backend(String),
}
