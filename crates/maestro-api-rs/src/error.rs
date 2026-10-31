use std::ffi::{CStr, c_char};
use std::fmt;

use crate::ffi::*;

/// What went wrong, from the `MaestroResult` of a failed call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    InvalidArg,
    /// The installed library doesn't provide an interface, or doesn't
    /// recognize a value: it is older than these bindings.
    Unsupported,
    /// The user turned Maestro off for this application.
    /// That's their choice, not a fault: carry on without Maestro.
    Disabled,
    /// Includes a panic caught inside the library.
    Internal,
    /// The synthesizer or the audio device could not be started.
    Unavailable,
    /// The realtime engine is stopped or restarting.
    NotRunning,
    /// Reading the MIDI file or writing the output failed.
    Io,
    /// The input is not a MIDI file Maestro reads.
    BadMidi,
    /// The output format could not be encoded.
    Encoder,
    /// The render was cancelled.
    Cancelled,
    /// The render job has already run.
    Busy,
    /// A code this version of the crate doesn't know.
    Other(i32),
}

impl ErrorKind {
    fn from_code(code: MaestroResult) -> Self {
        match code {
            MAESTRO_ERR_INVALID_ARG => Self::InvalidArg,
            MAESTRO_ERR_UNSUPPORTED => Self::Unsupported,
            MAESTRO_ERR_DISABLED => Self::Disabled,
            MAESTRO_ERR_INTERNAL => Self::Internal,
            MAESTRO_ERR_UNAVAILABLE => Self::Unavailable,
            MAESTRO_ERR_NOT_RUNNING => Self::NotRunning,
            MAESTRO_ERR_IO => Self::Io,
            MAESTRO_ERR_BAD_MIDI => Self::BadMidi,
            MAESTRO_ERR_ENCODER => Self::Encoder,
            MAESTRO_ERR_CANCELLED => Self::Cancelled,
            MAESTRO_ERR_BUSY => Self::Busy,
            other => Self::Other(other),
        }
    }
}

/// A failed Maestro call, with the library's description of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Reads `last_error()`, which must be called right after the failure and
    /// on the same thread.
    pub(crate) fn last(
        code: MaestroResult,
        last_error: unsafe extern "C" fn() -> *const c_char,
    ) -> Self {
        let message = unsafe { CStr::from_ptr(last_error()) }
            .to_string_lossy()
            .into_owned();
        Self::new(ErrorKind::from_code(code), message)
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for Error {}

/// Why [`Maestro::load`](crate::Maestro::load) found no usable library.
#[derive(Debug)]
#[non_exhaustive]
pub enum LoadError {
    /// The library isn't installed, or couldn't be loaded.
    NotFound(libloading::Error),
    /// The library doesn't export `maestro_get_interface`: it isn't the
    /// Maestro API.
    NotMaestro(libloading::Error),
    /// The library lacks an interface these bindings can't do without.
    MissingInterface(&'static CStr),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(e) => write!(f, "Maestro is not installed ({e})"),
            Self::NotMaestro(e) => write!(f, "the library is not the Maestro API ({e})"),
            Self::MissingInterface(id) => write!(
                f,
                "the Maestro library lacks {}; update Maestro",
                id.to_string_lossy()
            ),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotFound(e) | Self::NotMaestro(e) => Some(e),
            Self::MissingInterface(_) => None,
        }
    }
}
