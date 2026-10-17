//! Rust bindings to the Maestro API, which drives [Maestro]'s realtime MIDI
//! synthesizer inside your process: timestamped MIDI 1.0 and UMP events,
//! several ports, live statistics and per-application settings the user
//! controls.
//!
//! The library is loaded at runtime from where the Maestro installer puts it,
//! so this crate builds anywhere and your program still starts on machines
//! without Maestro; [`Maestro::load`] tells you whether it's there.
//!
//! ```no_run
//! use std::time::Duration;
//! use maestro_api_rs::{Maestro, OpenOptions, TIME_NOW, short_message};
//!
//! let maestro = Maestro::load()?;
//! let inst = maestro.open(OpenOptions::new("org.example.app").app_name("Example"))?;
//!
//! inst.send_short(0, short_message(0x90, 60, 100), TIME_NOW)?;
//! inst.send_short(0, short_message(0x80, 60, 0), maestro.after(Duration::from_millis(500)))?;
//!
//! std::thread::sleep(Duration::from_secs(1));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! [Maestro]: https://dimms.gr/maestro

use std::ffi::OsStr;
use std::fmt;
use std::sync::OnceLock;
use std::time::Duration;

pub mod ffi;

mod error;
mod instance;

pub use error::{Error, ErrorKind, LoadError};
pub use instance::{Info, Instance, LogLevel, Notification, OpenOptions, Stats};

/// A timestamp on the platform's monotonic clock, in the units
/// [`Maestro::now`] returns. See `maestro.h` for what they are on each system.
pub type Time = ffi::MaestroTime;

/// Plays an event as soon as possible.
pub const TIME_NOW: Time = ffi::MAESTRO_TIME_NOW;

/// The name the library is installed under, which the system's loader finds.
pub const LIBRARY_NAME: &str = if cfg!(windows) {
    "maestro.dll"
} else if cfg!(target_os = "macos") {
    "libmaestro.1.dylib"
} else {
    "libmaestro.so.1"
};

/// Packs a MIDI 1.0 short message for [`Instance::send_short`].
pub const fn short_message(status: u8, data1: u8, data2: u8) -> u32 {
    status as u32 | (data1 as u32) << 8 | (data2 as u32) << 16
}

/// An API version, as `maestro_get_version()` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
}

impl Version {
    /// The version these bindings were written against.
    pub const REQUIRED: Self = Self::from_raw(ffi::MAESTRO_API_VERSION);

    pub const fn from_raw(raw: u32) -> Self {
        Self {
            major: (raw >> 16) as u16,
            minor: raw as u16,
        }
    }

    /// Whether a library of this version can serve these bindings.
    // Always true on the minor while REQUIRED is x.0; not once it moves on.
    #[allow(clippy::absurd_extreme_comparisons)]
    pub const fn is_compatible(self) -> bool {
        self.major == Self::REQUIRED.major && self.minor >= Self::REQUIRED.minor
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The core functions, resolved once.
pub(crate) struct Api {
    pub get_version: ffi::GetVersionFn,
    pub last_error: ffi::LastErrorFn,
    pub now: ffi::NowFn,
    pub ns_to_ticks: ffi::NsToTicksFn,
    pub open: ffi::OpenFn,
    pub close: ffi::CloseFn,
    pub get_extension: ffi::GetExtensionFn,
}

/// The loaded Maestro library. Cheap to copy; it stays loaded for the rest of
/// the process.
#[derive(Clone, Copy)]
pub struct Maestro {
    api: &'static Api,
}

impl fmt::Debug for Maestro {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Maestro")
            .field("version", &self.version())
            .finish()
    }
}

impl Maestro {
    /// Loads the installed library ([`LIBRARY_NAME`]), or returns the one
    /// already loaded.
    pub fn load() -> Result<Self, LoadError> {
        static LOADED: OnceLock<Maestro> = OnceLock::new();
        if let Some(maestro) = LOADED.get() {
            return Ok(*maestro);
        }
        // SAFETY: this is the name the Maestro installer links the library
        // under, and its initializers are safe to run.
        let maestro = unsafe { Self::load_from(LIBRARY_NAME) }?;
        Ok(*LOADED.get_or_init(|| maestro))
    }

    /// Loads the library from `path`, e.g. a development build. The library is
    /// never unloaded, so call this once per path.
    ///
    /// # Safety
    /// `path` must be a Maestro API library: loading runs its initializers, and
    /// its exports are trusted to match `maestro.h`.
    pub unsafe fn load_from(path: impl AsRef<OsStr>) -> Result<Self, LoadError> {
        let lib =
            unsafe { libloading::Library::new(path.as_ref()) }.map_err(LoadError::NotFound)?;
        macro_rules! resolve {
            ($name:literal) => {
                *unsafe { lib.get(concat!($name, "\0").as_bytes()) }
                    .map_err(|e| LoadError::MissingSymbol($name, e))?
            };
        }
        let api = Api {
            get_version: resolve!("maestro_get_version"),
            last_error: resolve!("maestro_last_error"),
            now: resolve!("maestro_now"),
            ns_to_ticks: resolve!("maestro_ns_to_ticks"),
            open: resolve!("maestro_open"),
            close: resolve!("maestro_close"),
            get_extension: resolve!("maestro_get_extension"),
        };
        // Never unloaded: the library runs threads and keeps thread-locals,
        // which don't survive being unmapped under them.
        std::mem::forget(lib);

        let maestro = Self {
            api: Box::leak(Box::new(api)),
        };
        let found = maestro.version();
        if !found.is_compatible() {
            return Err(LoadError::Incompatible { found });
        }
        Ok(maestro)
    }

    /// The installed library's version.
    pub fn version(&self) -> Version {
        Version::from_raw(unsafe { (self.api.get_version)() })
    }

    /// The current host time. Realtime-safe.
    pub fn now(&self) -> Time {
        unsafe { (self.api.now)() }
    }

    /// Converts a duration to host-time ticks, for scheduling ahead.
    /// Realtime-safe.
    pub fn ticks(&self, duration: Duration) -> Time {
        let ns = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        unsafe { (self.api.ns_to_ticks)(ns) }
    }

    /// The host time `duration` from now. Realtime-safe.
    pub fn after(&self, duration: Duration) -> Time {
        self.now().saturating_add(self.ticks(duration))
    }

    /// Opens an instance, with its own engine and audio output. A process can
    /// have several open at once.
    ///
    /// Fails with [`ErrorKind::Disabled`] when the user has turned the API off
    /// for this application; treat that as "carry on without Maestro".
    pub fn open(&self, options: OpenOptions) -> Result<Instance, Error> {
        Instance::open(*self, options)
    }

    pub(crate) fn api(&self) -> &'static Api {
        self.api
    }
}
