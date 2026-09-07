use thiserror::Error;

#[derive(Error, Debug)]
pub enum AudioConfigurationError {
    #[error("Invalid audio channel count: {0}")]
    ChannelCount(u16),
}
