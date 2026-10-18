use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{self, AssertUnwindSafe};
use std::ptr;
use std::sync::Arc;

use crate::ffi::*;
use crate::{Error, ErrorKind};

/// Severity of a message passed to [`App::on_log`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LogLevel {
    Error,
    Warn,
    Other(u32),
}

impl LogLevel {
    fn from_raw(level: MaestroLogLevel) -> Self {
        match level {
            MAESTRO_LOG_ERROR => Self::Error,
            MAESTRO_LOG_WARN => Self::Warn,
            other => Self::Other(other),
        }
    }
}

/// What [`RealtimeOptions::on_notify`](crate::RealtimeOptions::on_notify)
/// reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Notification {
    /// The user edited this application's settings; re-read
    /// [`Realtime::info`](crate::Realtime::info).
    SettingsChanged,
    /// The engine failed to restart.
    EngineStopped,
    EngineStarted,
    /// The user disabled the API, or this application's settings.
    EngineDisabled,
    Other(u32),
}

impl Notification {
    fn from_raw(what: MaestroNotify) -> Self {
        match what {
            MAESTRO_NOTIFY_SETTINGS_CHANGED => Self::SettingsChanged,
            MAESTRO_NOTIFY_ENGINE_STOPPED => Self::EngineStopped,
            MAESTRO_NOTIFY_ENGINE_STARTED => Self::EngineStarted,
            MAESTRO_NOTIFY_ENGINE_DISABLED => Self::EngineDisabled,
            other => Self::Other(other),
        }
    }
}

type LogCallback = Arc<dyn Fn(LogLevel, &str) + Send + Sync>;
pub(crate) type NotifyCallback = Box<dyn Fn(Notification) + Send + Sync>;

/// Who the application is. Selects the user's settings for it, which realtime
/// playback and file rendering share.
#[derive(Clone)]
pub struct App {
    id: String,
    name: Option<String>,
    log: Option<LogCallback>,
}

impl App {
    /// `id` is a stable reverse-DNS ID, e.g. `"org.example.app"`, of 1-128
    /// characters from `A-Z a-z 0-9 . - _`.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: None,
            log: None,
        }
    }

    /// The name shown in Maestro's settings.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Receives the library's log messages. May run on any thread; keep it
    /// short and don't call back into Maestro from it.
    pub fn on_log(mut self, f: impl Fn(LogLevel, &str) + Send + Sync + 'static) -> Self {
        self.log = Some(Arc::new(f));
        self
    }
}

pub(crate) struct Callbacks {
    log: Option<LogCallback>,
    notify: Option<NotifyCallback>,
}

unsafe extern "C" fn log_trampoline(user: *mut c_void, level: u32, message: *const c_char) {
    let callbacks = unsafe { &*user.cast::<Callbacks>() };
    if let (Some(log), false) = (&callbacks.log, message.is_null()) {
        let message = unsafe { CStr::from_ptr(message) }.to_string_lossy();
        // A panic must not unwind into the library.
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            log(LogLevel::from_raw(level), &message)
        }));
    }
}

unsafe extern "C" fn notify_trampoline(user: *mut c_void, what: u32) {
    let callbacks = unsafe { &*user.cast::<Callbacks>() };
    if let Some(notify) = &callbacks.notify {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| notify(Notification::from_raw(what))));
    }
}

pub(crate) fn c_string(s: impl Into<Vec<u8>>, what: &str) -> Result<CString, Error> {
    CString::new(s).map_err(|_| Error::new(ErrorKind::InvalidArg, format!("{what} contains NUL")))
}

/// A `MaestroAppInfo` and what it points at, kept alive together. The
/// callbacks must outlive the object opened with it.
pub(crate) struct RawApp {
    _id: CString,
    _name: Option<CString>,
    pub callbacks: Box<Callbacks>,
    pub info: MaestroAppInfo,
}

impl RawApp {
    pub fn new(app: &App, notify: Option<NotifyCallback>) -> Result<Self, Error> {
        let id = c_string(app.id.as_str(), "app_id")?;
        let name = app
            .name
            .as_deref()
            .map(|n| c_string(n, "app_name"))
            .transpose()?;
        let callbacks = Box::new(Callbacks {
            log: app.log.clone(),
            notify,
        });
        let info = MaestroAppInfo {
            struct_size: size_of::<MaestroAppInfo>() as u32,
            reserved: 0,
            app_id: id.as_ptr(),
            app_name: name.as_ref().map_or(ptr::null(), |n| n.as_ptr()),
            log: callbacks.log.as_ref().map(|_| log_trampoline as _),
            user: ptr::from_ref(&*callbacks).cast_mut().cast(),
        };
        Ok(Self {
            _id: id,
            _name: name,
            callbacks,
            info,
        })
    }

    pub fn notify_fn(&self) -> MaestroNotifyFn {
        self.callbacks
            .notify
            .as_ref()
            .map(|_| notify_trampoline as _)
    }
}
