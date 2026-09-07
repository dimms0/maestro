use thiserror::Error;

#[derive(Error, Debug)]
pub enum FileRendererError {
    #[error("Error opening file: {0}")]
    FileSystem(#[from] std::io::Error),

    #[error("Error writing audio file: {0}")]
    AudioWriter(#[from] hound::Error),

    #[error("Invalid file: {0}")]
    InvalidFile(String),

    #[error("Audio encoding error: {0}")]
    Encoding(String),

    #[error("Output format '{0}' is not available")]
    UnsupportedFormat(&'static str),
}
