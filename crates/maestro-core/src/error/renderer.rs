use thiserror::Error;

#[derive(Error, Debug)]
pub enum RendererError {
    #[error("Audio configuration error: {0}")]
    AudioConfiguration(#[from] super::AudioConfigurationError),

    #[error("Error initializing synthesizer: {0}")]
    SynthInit(String),

    #[error("The selected port does not exist: {0}")]
    InvalidPort(u8),

    #[error("MIDI event error: {0}")]
    Event(#[from] super::MIDIEventError),

    #[error("Error loading soundfont(s): {0}")]
    SoundFont(String),

    #[error("Loaded configuration does not match the active synthesizer")]
    ConfigMismatch,

    #[error("Error creating threadpool: {0}")]
    ThreadPool(#[from] rayon::ThreadPoolBuildError),

    #[error("Error loading library: {0}")]
    Library(#[from] libloading::Error),
}
