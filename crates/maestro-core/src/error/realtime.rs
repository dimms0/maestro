use thiserror::Error;

#[derive(Error, Debug)]
pub enum RealtimeEngineError {
    #[error("Error creating thread: {0:?}")]
    Thread(#[source] std::io::Error),

    #[error("Audio device error: {0}")]
    Audio(String),

    #[error("Error building audio stream: {0}")]
    BuildStream(#[from] cpal::Error),

    #[error("Error selecting output configuration: {0}")]
    DefaultStreamConfig(cpal::Error),
}
