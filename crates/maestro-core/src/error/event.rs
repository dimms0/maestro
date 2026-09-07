use thiserror::Error;

#[derive(Error, Debug)]
pub enum MIDIEventError {
    #[error("Invalid short event received: {0:x}")]
    Short(u32),

    #[error("Unknown SysEx message received: {0:?}")]
    SysEx(Box<[u8]>),
}
