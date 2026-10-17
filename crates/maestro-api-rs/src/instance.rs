use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{self, AssertUnwindSafe};
use std::ptr::{self, NonNull};

use crate::ffi::*;
use crate::{Error, ErrorKind, Maestro, Time};

/// Severity of a message passed to [`OpenOptions::on_log`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Other(u32),
}

impl LogLevel {
    fn from_raw(level: u32) -> Self {
        match level {
            MAESTRO_LOG_ERROR => Self::Error,
            MAESTRO_LOG_WARN => Self::Warn,
            MAESTRO_LOG_INFO => Self::Info,
            MAESTRO_LOG_DEBUG => Self::Debug,
            other => Self::Other(other),
        }
    }
}

/// What [`OpenOptions::on_notify`] reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Notification {
    /// The user edited this application's settings; re-read [`Instance::info`].
    SettingsChanged,
    /// The settings were disabled, or the engine failed to restart.
    EngineStopped,
    EngineStarted,
    Other(u32),
}

impl Notification {
    fn from_raw(what: u32) -> Self {
        match what {
            MAESTRO_NOTIFY_SETTINGS_CHANGED => Self::SettingsChanged,
            MAESTRO_NOTIFY_ENGINE_STOPPED => Self::EngineStopped,
            MAESTRO_NOTIFY_ENGINE_STARTED => Self::EngineStarted,
            other => Self::Other(other),
        }
    }
}

type LogCallback = Box<dyn Fn(LogLevel, &str) + Send + Sync>;
type NotifyCallback = Box<dyn Fn(Notification) + Send + Sync>;

/// How to open an [`Instance`].
pub struct OpenOptions {
    app_id: String,
    app_name: Option<String>,
    num_ports: u32,
    midi2: bool,
    log: Option<LogCallback>,
    notify: Option<NotifyCallback>,
}

impl OpenOptions {
    /// `app_id` is a stable reverse-DNS ID, e.g. `"org.example.app"`, of
    /// 1-128 characters from `A-Z a-z 0-9 . - _`. It selects the user's
    /// settings for this application.
    pub fn new(app_id: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
            app_name: None,
            num_ports: 0,
            midi2: false,
            log: None,
            notify: None,
        }
    }

    /// The name shown in Maestro's settings.
    pub fn app_name(mut self, name: impl Into<String>) -> Self {
        self.app_name = Some(name.into());
        self
    }

    /// The MIDI 1.0 ports the application will address, 1-16 (default 1).
    /// A hint: the user's settings have the last word.
    pub fn num_ports(mut self, ports: u8) -> Self {
        self.num_ports = ports.into();
        self
    }

    /// Asks for all 16 UMP groups as ports. A hint, like `num_ports`.
    pub fn midi2(mut self, midi2: bool) -> Self {
        self.midi2 = midi2;
        self
    }

    /// Receives the library's log messages. May run on any thread; keep it
    /// short and don't call back into Maestro from it.
    pub fn on_log(mut self, f: impl Fn(LogLevel, &str) + Send + Sync + 'static) -> Self {
        self.log = Some(Box::new(f));
        self
    }

    /// Hears about settings changes and engine restarts. Same rules as
    /// [`on_log`](Self::on_log).
    pub fn on_notify(mut self, f: impl Fn(Notification) + Send + Sync + 'static) -> Self {
        self.notify = Some(Box::new(f));
        self
    }
}

struct Callbacks {
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

/// The settings that resulted from the hints combined with the user's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Info {
    pub sample_rate: u32,
    pub num_ports: u32,
    pub midi2: bool,
    pub render_block_us: u32,
}

/// Live statistics of the synth.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Stats {
    /// Current number of active voices.
    pub voice_count: u64,
    /// Render load percentage,
    pub render_time: f32,
    /// Notes per second processed.
    pub nps: u64,
    /// Events per second processed.
    pub eps: u64,
    /// The current notes-per-second limit, after the load limiter.
    pub nps_limit: u64,
}

/// An open instance: its own engine and audio output. Closed when dropped.
///
/// Every method takes `&self` and may be called from any thread; the ones
/// marked realtime-safe neither allocate nor block.
pub struct Instance {
    maestro: Maestro,
    raw: NonNull<MaestroInstance>,
    midi1: Option<NonNull<MaestroExtMidi1>>,
    ump: Option<NonNull<MaestroExtUmp>>,
    control: Option<NonNull<MaestroExtControl>>,
    info: Option<NonNull<MaestroExtInfo>>,
    stats: Option<NonNull<MaestroExtStats>>,
    // Read by the trampolines until maestro_close returns.
    _callbacks: Box<Callbacks>,
}

// Everything but maestro_close is safe from any thread, and close needs the
// instance by value.
unsafe impl Send for Instance {}
unsafe impl Sync for Instance {}

impl Instance {
    pub(crate) fn open(maestro: Maestro, options: OpenOptions) -> Result<Self, Error> {
        let api = maestro.api();
        let nul_free = |s: String, what| {
            CString::new(s)
                .map_err(|_| Error::new(ErrorKind::InvalidArg, format!("{what} contains NUL")))
        };
        let app_id = nul_free(options.app_id, "app_id")?;
        let app_name = options
            .app_name
            .map(|n| nul_free(n, "app_name"))
            .transpose()?;

        let hints = MaestroHints {
            struct_size: size_of::<MaestroHints>() as u32,
            num_ports: options.num_ports,
            midi2: options.midi2.into(),
        };
        let callbacks = Box::new(Callbacks {
            log: options.log,
            notify: options.notify,
        });
        let params = MaestroOpenParams {
            struct_size: size_of::<MaestroOpenParams>() as u32,
            app_id: app_id.as_ptr(),
            app_name: app_name.as_ref().map_or(ptr::null(), |n| n.as_ptr()),
            hints: &hints,
            log: callbacks.log.as_ref().map(|_| log_trampoline as _),
            notify: callbacks.notify.as_ref().map(|_| notify_trampoline as _),
            user: ptr::from_ref(&*callbacks).cast_mut().cast(),
        };

        let mut raw = ptr::null_mut();
        let code = unsafe { (api.open)(MAESTRO_API_VERSION, &params, &mut raw) };
        if code < MAESTRO_OK {
            return Err(Error::last(code, api.last_error));
        }
        let raw = NonNull::new(raw)
            .ok_or_else(|| Error::new(ErrorKind::Internal, "maestro_open returned no instance"))?;

        let ext = |id: &CStr| {
            NonNull::new(unsafe { (api.get_extension)(raw.as_ptr(), id.as_ptr()) }.cast_mut())
        };
        Ok(Self {
            maestro,
            raw,
            midi1: ext(MAESTRO_EXT_MIDI1).map(NonNull::cast),
            ump: ext(MAESTRO_EXT_UMP).map(NonNull::cast),
            control: ext(MAESTRO_EXT_CONTROL).map(NonNull::cast),
            info: ext(MAESTRO_EXT_INFO).map(NonNull::cast),
            stats: ext(MAESTRO_EXT_STATS).map(NonNull::cast),
            _callbacks: callbacks,
        })
    }

    /// The library this instance came from.
    pub fn maestro(&self) -> Maestro {
        self.maestro
    }

    /// Sends a MIDI 1.0 short message, packed by
    /// [`short_message`](crate::short_message). Realtime-safe.
    pub fn send_short(&self, port: u8, msg: u32, at: Time) -> Result<(), Error> {
        let midi1 = self.extension(self.midi1, MAESTRO_EXT_MIDI1)?;
        self.check(unsafe { (midi1.send_short)(self.raw.as_ptr(), port, msg, at) })
    }

    /// Sends SysEx or any long message; `data` is copied before this returns.
    /// Realtime-safe.
    pub fn send_long(&self, port: u8, data: &[u8], at: Time) -> Result<(), Error> {
        let midi1 = self.extension(self.midi1, MAESTRO_EXT_MIDI1)?;
        self.check(unsafe {
            (midi1.send_long)(self.raw.as_ptr(), port, data.as_ptr(), data.len(), at)
        })
    }

    /// Sends one or more complete Universal MIDI Packets, in host byte order.
    /// Realtime-safe.
    pub fn send_ump(&self, words: &[u32], at: Time) -> Result<(), Error> {
        let ump = self.extension(self.ump, MAESTRO_EXT_UMP)?;
        self.check(unsafe { (ump.send)(self.raw.as_ptr(), words.as_ptr(), words.len(), at) })
    }

    /// All notes off, controllers reset, and every queued event dropped.
    pub fn reset(&self) -> Result<(), Error> {
        let control = self.extension(self.control, MAESTRO_EXT_CONTROL)?;
        self.check(unsafe { (control.reset)(self.raw.as_ptr()) })
    }

    /// Opens Maestro's settings for this application so the user can tune it.
    pub fn show_settings(&self) -> Result<(), Error> {
        let control = self.extension(self.control, MAESTRO_EXT_CONTROL)?;
        self.check(unsafe { (control.show_settings)(self.raw.as_ptr()) })
    }

    /// The settings in effect.
    pub fn info(&self) -> Result<Info, Error> {
        let ext = self.extension(self.info, MAESTRO_EXT_INFO)?;
        let mut out = MaestroInfo {
            struct_size: size_of::<MaestroInfo>() as u32,
            ..Default::default()
        };
        self.check(unsafe { (ext.get)(self.raw.as_ptr(), &mut out) })?;
        Ok(Info {
            sample_rate: out.sample_rate,
            num_ports: out.num_ports,
            midi2: out.midi2 != 0,
            render_block_us: out.render_block_us,
        })
    }

    /// Live statistics. Realtime-safe.
    pub fn stats(&self) -> Result<Stats, Error> {
        let ext = self.extension(self.stats, MAESTRO_EXT_STATS)?;
        let mut out = MaestroStats {
            struct_size: size_of::<MaestroStats>() as u32,
            ..Default::default()
        };
        self.check(unsafe { (ext.get)(self.raw.as_ptr(), &mut out) })?;
        Ok(Stats {
            voice_count: out.voice_count,
            render_time: out.render_time,
            nps: out.nps,
            eps: out.eps,
            nps_limit: out.nps_limit,
        })
    }

    /// The raw handle, for calling extensions these bindings don't wrap yet.
    /// It stays valid until this `Instance` is dropped.
    pub fn as_raw(&self) -> *mut MaestroInstance {
        self.raw.as_ptr()
    }

    /// The function table of extension `id`, or NULL if the library doesn't
    /// provide it. Valid until this `Instance` is dropped.
    pub fn raw_extension(&self, id: &CStr) -> *const c_void {
        unsafe { (self.maestro.api().get_extension)(self.raw.as_ptr(), id.as_ptr()) }
    }

    fn extension<T>(&self, table: Option<NonNull<T>>, id: &CStr) -> Result<&T, Error> {
        match table {
            // SAFETY: extension tables live as long as the instance.
            Some(table) => Ok(unsafe { table.as_ref() }),
            None => Err(Error::new(
                ErrorKind::Unsupported,
                format!(
                    "this Maestro library does not provide {}",
                    id.to_string_lossy()
                ),
            )),
        }
    }

    fn check(&self, code: MaestroResult) -> Result<(), Error> {
        if code < MAESTRO_OK {
            Err(Error::last(code, self.maestro.api().last_error))
        } else {
            Ok(())
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // No callback runs after this returns, so `_callbacks` can go next.
        unsafe { (self.maestro.api().close)(self.raw.as_ptr()) };
    }
}
