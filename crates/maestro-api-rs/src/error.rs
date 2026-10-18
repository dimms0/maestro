use std::ffi::CStr;
use std::fmt;

use crate::ffi::*;

/// What went wrong, from the `MaestroResult` of a failed call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    InvalidArg,
    /// The requested API version, or an extension, isn't provided by the
    /// installed library.
    Unsupported,
    /// The user turned the API off, or turned it off for this application.
    /// That's their choice, not a fault: carry on without Maestro.
    Disabled,
    /// The audio device could not be opened.
    Unavailable,
    /// The engine is stopped or restarting.
    NotRunning,
    /// Includes a panic caught inside the library.
    Internal,
    /// A code this version of the crate doesn't know.
    Other(i32),
}

impl ErrorKind {
    fn from_code(code: MaestroResult) -> Self {
        match code {
            MAESTRO_ERR_INVALID_ARG => Self::InvalidArg,
            MAESTRO_ERR_UNSUPPORTED => Self::Unsupported,
            MAESTRO_ERR_DISABLED => Self::Disabled,
            MAESTRO_ERR_UNAVAILABLE => Self::Unavailable,
            MAESTRO_ERR_NOT_RUNNING => Self::NotRunning,
            MAESTRO_ERR_INTERNAL => Self::Internal,
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

    /// Reads `maestro_last_error()`, which must be called right after the
    /// failure and on the same thread.
    pub(crate) fn last(code: MaestroResult, last_error: LastErrorFn) -> Self {
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
    /// The library lacks one of the core functions.
    MissingSymbol(&'static str, libloading::Error),
    /// The installed library is a different major version, or an older minor.
    Incompatible { found: crate::Version },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(e) => write!(f, "Maestro is not installed ({e})"),
            Self::MissingSymbol(name, e) => write!(f, "the Maestro library lacks {name} ({e})"),
            Self::Incompatible { found } => write!(
                f,
                "Maestro API {found} is installed, but this program needs {}",
                crate::Version::REQUIRED
            ),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotFound(e) | Self::MissingSymbol(_, e) => Some(e),
            Self::Incompatible { .. } => None,
        }
    }
}
