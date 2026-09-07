use thiserror::Error;

mod audio;
mod config;
mod event;
mod file_renderer;
mod realtime;
mod renderer;

pub use audio::*;
pub use config::*;
pub use event::*;
pub use file_renderer::*;
pub use realtime::*;
pub use renderer::*;

#[derive(Error, Debug)]
pub enum MaestroComponentError {
    #[error("Renderer error: {0}")]
    Renderer(#[from] RendererError),

    #[error("File renderer error: {0}")]
    FileRenderer(#[from] FileRendererError),

    #[error("Realtime engine error: {0}")]
    RealtimeEngine(#[from] RealtimeEngineError),
}
