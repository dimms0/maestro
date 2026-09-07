/// Everything that can go wrong reading or writing a MIDI file.
///
/// Byte offsets are relative to the start of the file, so a message points at
/// the exact spot a malformed file went wrong.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("not a MIDI file: expected \"MThd\" or \"SMF2CLIP\"")]
    BadMagic,

    #[error("SMF format {0} is not supported")]
    UnsupportedFormat(u16),

    #[error("data ends mid-event at byte {0}")]
    Truncated(usize),

    #[error("chunk at byte {0} declares a length that runs past the end of the file")]
    BadChunkLength(usize),

    #[error("data byte where a status byte was expected at byte {0}")]
    NoRunningStatus(usize),

    #[error("track starting at byte {0} has no End of Track event")]
    MissingEndOfTrack(usize),

    #[error("invalid clip file: {0}")]
    Clip(&'static str),

    #[error("event has no equivalent in the target format")]
    Unrepresentable,
}

pub type Result<T> = std::result::Result<T, Error>;
