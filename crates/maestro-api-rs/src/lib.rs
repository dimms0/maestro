//! Rust bindings to the Maestro API, which runs [Maestro]'s MIDI synthesizer
//! inside your process: realtime playback of timestamped MIDI 1.0 and UMP
//! events, and rendering MIDI files to audio files, both with the
//! per-application settings the user controls.
//!
//! The library is loaded at runtime from where the Maestro installer puts it,
//! so this crate builds anywhere and your program still starts on machines
//! without Maestro; [`Maestro::load`] tells you whether it's there.
//!
//! ```no_run
//! use std::time::Duration;
//! use maestro_api_rs::{App, Maestro, RealtimeOptions, TIME_NOW, short_message};
//!
//! let maestro = Maestro::load()?;
//! let app = App::new("org.example.app").name("Example");
//! let rt = maestro.open_realtime(&app, RealtimeOptions::new())?;
//!
//! rt.send_short(0, short_message(0x90, 60, 100), TIME_NOW)?;
//! rt.send_short(0, short_message(0x80, 60, 0), rt.after(Duration::from_millis(500)))?;
//!
//! std::thread::sleep(Duration::from_secs(1));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! [Maestro]: https://dimms.gr/maestro

use std::ffi::{CStr, OsStr};
use std::fmt;
use std::sync::OnceLock;

pub mod ffi;

mod app;
mod error;
mod file_render;
mod realtime;

pub use app::{App, LogLevel, Notification};
pub use error::{Error, ErrorKind, LoadError};
pub use file_render::{FileRender, FileRenderOptions, Format, PortMode, Progress, WavBitDepth};
pub use realtime::{Info, Realtime, RealtimeOptions, Stats};

/// A timestamp on the platform's monotonic clock, in the units
/// [`Realtime::now`] returns. See `maestro.h` for what they are on each system.
pub type Time = ffi::MaestroTime;

/// Plays an event as soon as possible.
pub const TIME_NOW: Time = ffi::MAESTRO_TIME_NOW;

/// The name the library is installed under, which the system's loader finds.
pub const LIBRARY_NAME: &str = if cfg!(windows) {
    "maestro.dll"
} else if cfg!(target_os = "macos") {
    "libmaestro.dylib"
} else {
    "libmaestro.so"
};

/// Packs a MIDI 1.0 short message for [`Realtime::send_short`].
pub const fn short_message(status: u8, data1: u8, data2: u8) -> u32 {
    status as u32 | (data1 as u32) << 8 | (data2 as u32) << 16
}

/// The version of the Maestro release the library comes from. For display
/// only: what the library can do is told by the interfaces it provides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The interface tables, looked up once.
pub(crate) struct Api {
    pub get_interface: ffi::MaestroGetInterfaceFn,
    pub core: &'static ffi::MaestroCoreV1,
    pub realtime: Option<&'static ffi::MaestroRealtimeV1>,
    pub file_render: Option<&'static ffi::MaestroFileRenderV1>,
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
        let get_interface: ffi::MaestroGetInterfaceFn =
            *unsafe { lib.get(b"maestro_get_interface\0") }.map_err(LoadError::NotMaestro)?;
        // Never unloaded: the library runs threads and keeps thread-locals,
        // which don't survive being unmapped under them.
        std::mem::forget(lib);

        // SAFETY: a table is constant while the library is loaded, which is
        // forever, and has the layout its ID names.
        fn table<T>(get_interface: ffi::MaestroGetInterfaceFn, id: &CStr) -> Option<&'static T> {
            unsafe { get_interface(id.as_ptr()).cast::<T>().as_ref() }
        }
        let api = Api {
            get_interface,
            core: table(get_interface, ffi::MAESTRO_CORE_V1)
                .ok_or(LoadError::MissingInterface(ffi::MAESTRO_CORE_V1))?,
            realtime: table(get_interface, ffi::MAESTRO_REALTIME_V1),
            file_render: table(get_interface, ffi::MAESTRO_FILE_RENDER_V1),
        };
        Ok(Self {
            api: Box::leak(Box::new(api)),
        })
    }

    /// The version of the Maestro release the library comes from.
    pub fn version(&self) -> Version {
        let raw = unsafe { (self.api.core.get_version)() };
        Version {
            major: (raw >> 16) as u16,
            minor: raw as u16,
        }
    }

    /// Opens a live engine, with its own audio output. A process can have
    /// several open at once. Needs Maestro 0.2 or later.
    ///
    /// Fails with [`ErrorKind::Disabled`] when the user has turned the API off
    /// for this application; treat that as "carry on without Maestro".
    pub fn open_realtime(&self, app: &App, options: RealtimeOptions) -> Result<Realtime, Error> {
        Realtime::open(*self, app, options)
    }

    /// Prepares a job that renders a MIDI file to an audio file. Nothing is
    /// rendered until [`FileRender::run`]. Needs Maestro 0.2 or later.
    pub fn open_file_render(
        &self,
        app: &App,
        options: FileRenderOptions,
    ) -> Result<FileRender, Error> {
        FileRender::open(*self, app, options)
    }

    /// The function table of interface `id`, or NULL if the library doesn't
    /// provide it, for calling interfaces these bindings don't wrap yet.
    pub fn raw_interface(&self, id: &CStr) -> *const std::ffi::c_void {
        unsafe { (self.api.get_interface)(id.as_ptr()) }
    }

    pub(crate) fn interface<T>(table: Option<&'static T>, id: &CStr) -> Result<&'static T, Error> {
        table.ok_or_else(|| {
            Error::new(
                ErrorKind::Unsupported,
                format!(
                    "this Maestro library does not provide {}; update Maestro",
                    id.to_string_lossy()
                ),
            )
        })
    }

    pub(crate) fn api(&self) -> &'static Api {
        self.api
    }

    pub(crate) fn check(&self, code: ffi::MaestroResult) -> Result<(), Error> {
        if code < ffi::MAESTRO_OK {
            Err(Error::last(code, self.api.core.last_error))
        } else {
            Ok(())
        }
    }
}
