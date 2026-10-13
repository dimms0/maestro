use std::ffi::{CStr, CString, c_char, c_void};
use std::mem::offset_of;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use arc_swap::ArcSwapOption;

use maestro_core::{
    realtime::{MaestroRealtimeEngine, MaestroRealtimeStatistics, RealtimeEventSender, UMP_GROUPS},
    system_cfg::{
        MaestroConfigManager, is_valid_app_id,
        watcher::{ConfigWatchEvent, ConfigWatcher},
    },
};

use crate::abi::*;
use crate::error::Failure;
use crate::profile::{self, ApiConfig};
use crate::stats;

const DEBOUNCE: Duration = Duration::from_millis(300);

pub struct Live {
    pub engine: MaestroRealtimeEngine,
    pub stats: MaestroRealtimeStatistics,
}

// The engine owns a cpal stream, which is not Send/Sync on every platform; it
// is only reached through the engine lock, and mutated only under its write side.
unsafe impl Send for Live {}
unsafe impl Sync for Live {}

struct Callbacks {
    log: MaestroLogFn,
    notify: MaestroNotifyFn,
    user: *mut c_void,
}

// Thread-safety of the callbacks is the host's promise, per the header.
unsafe impl Send for Callbacks {}
unsafe impl Sync for Callbacks {}

impl Callbacks {
    fn log(&self, level: u32, message: &str) {
        if let Some(log) = self.log {
            let message = CString::new(message.replace('\0', "")).unwrap_or_default();
            unsafe { log(self.user, level, message.as_ptr()) };
        }
    }

    fn notify(&self, what: u32) {
        if let Some(notify) = self.notify {
            unsafe { notify(self.user, what) };
        }
    }
}

pub struct Shared {
    pub app_id: String,
    pub ports: u8,
    pub midi2: bool,
    pub engine: RwLock<Option<Live>>,
    pub sender: ArcSwapOption<RealtimeEventSender>,
    pub config: Mutex<ApiConfig>,
    callbacks: Callbacks,
}

#[derive(Clone, Copy, PartialEq)]
enum Event {
    Config,
    Soundfonts,
    Shutdown,
}

pub struct Instance {
    pub shared: Arc<Shared>,
    watchers: Vec<ConfigWatcher>,
    control: Option<(Sender<Event>, JoinHandle<()>)>,
}

/// Reads a field of a caller's struct only if its `struct_size` covers it.
unsafe fn field<T: Copy>(base: *const u8, size: u32, offset: usize) -> Option<T> {
    (offset + size_of::<T>() <= size as usize)
        .then(|| unsafe { base.add(offset).cast::<T>().read_unaligned() })
}

unsafe fn string(ptr: Option<*const c_char>) -> Option<String> {
    let ptr = ptr.filter(|p| !p.is_null())?;
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .ok()
        .map(str::to_string)
}

struct Hints {
    num_ports: Option<u32>,
    midi2: bool,
}

impl Hints {
    unsafe fn read(ptr: *const MaestroHints) -> Self {
        let size = if ptr.is_null() {
            0
        } else {
            unsafe { (*ptr).struct_size }
        };
        let get = |offset| unsafe { field::<u32>(ptr.cast(), size, offset) }.filter(|v| *v != 0);
        Self {
            num_ports: get(offset_of!(MaestroHints, num_ports)),
            midi2: get(offset_of!(MaestroHints, midi2)).is_some(),
        }
    }
}

impl Instance {
    /// # Safety
    /// `params` must be NULL or point at least `struct_size` readable bytes.
    pub unsafe fn open(
        api_version: u32,
        params: *const MaestroOpenParams,
    ) -> Result<Self, Failure> {
        if api_version >> 16 != MAESTRO_VERSION_MAJOR
            || api_version & 0xFFFF > MAESTRO_VERSION_MINOR
        {
            return Err(Failure::new(
                MAESTRO_ERR_UNSUPPORTED,
                format!(
                    "API {}.{} was requested, but this library provides {MAESTRO_VERSION_MAJOR}.{MAESTRO_VERSION_MINOR}",
                    api_version >> 16,
                    api_version & 0xFFFF
                ),
            ));
        }
        if params.is_null() {
            return Err(Failure::invalid("params is NULL"));
        }

        let base = params.cast::<u8>();
        let size = unsafe { (*params).struct_size };
        let app_id = unsafe { string(field(base, size, offset_of!(MaestroOpenParams, app_id))) }
            .filter(|id| is_valid_app_id(id))
            .ok_or_else(|| {
                Failure::invalid(
                    "app_id is required: 1-128 characters of A-Z, a-z, 0-9, '.', '-' and '_'",
                )
            })?;
        let app_name =
            unsafe { string(field(base, size, offset_of!(MaestroOpenParams, app_name))) };
        let hints = unsafe {
            Hints::read(
                field(base, size, offset_of!(MaestroOpenParams, hints)).unwrap_or(std::ptr::null()),
            )
        };
        let callbacks = unsafe {
            Callbacks {
                log: field(base, size, offset_of!(MaestroOpenParams, log)).flatten(),
                notify: field(base, size, offset_of!(MaestroOpenParams, notify)).flatten(),
                user: field(base, size, offset_of!(MaestroOpenParams, user))
                    .unwrap_or(std::ptr::null_mut()),
            }
        };

        let ports = match hints.num_ports {
            _ if hints.midi2 => UMP_GROUPS,
            None => 1,
            Some(n @ 1..=16) => n as u8,
            Some(n) => return Err(Failure::invalid(format!("num_ports must be 1-16, not {n}"))),
        };

        let manager = MaestroConfigManager::default();
        let global = profile::load_global(&manager).unwrap_or_else(|e| {
            callbacks.log(
                MAESTRO_LOG_WARN,
                &format!("The Maestro API settings could not be read ({e}); using the defaults"),
            );
            ApiConfig::default()
        });
        if !global.enabled {
            return Err(Failure::new(
                MAESTRO_ERR_DISABLED,
                "The Maestro API is turned off in Maestro's settings",
            ));
        }

        if let Err(e) = profile::ensure_app(&manager, &app_id, app_name.as_deref()) {
            callbacks.log(
                MAESTRO_LOG_WARN,
                &format!("This application's settings could not be created ({e})"),
            );
        }
        let config = profile::load_app(&manager, &app_id).unwrap_or_else(|e| {
            callbacks.log(
                MAESTRO_LOG_WARN,
                &format!("This application's settings could not be read ({e}); using the defaults"),
            );
            ApiConfig {
                enabled: true,
                ..global
            }
        });
        if !config.enabled {
            return Err(Failure::new(
                MAESTRO_ERR_DISABLED,
                format!("Maestro is set not to serve this application ({app_id})"),
            ));
        }

        let shared = Arc::new(Shared {
            app_id,
            ports,
            midi2: hints.midi2,
            engine: RwLock::new(None),
            sender: ArcSwapOption::empty(),
            config: Mutex::new(config.clone()),
            callbacks,
        });
        shared.start(&manager, &config)?;

        let (tx, rx) = channel();
        let watchers = [
            (
                manager.get_app_config_path(&shared.app_id),
                manager.sflist_dirs(),
            ),
            (profile::global_path(&manager), Vec::new()),
        ]
        .into_iter()
        .filter_map(|(path, sflist_dirs)| {
            let tx = tx.clone();
            let warn = shared.clone();
            ConfigWatcher::spawn(
                path,
                sflist_dirs,
                move |event| {
                    let _ = tx.send(match event {
                        ConfigWatchEvent::ConfigChanged => Event::Config,
                        ConfigWatchEvent::SoundfontsChanged => Event::Soundfonts,
                    });
                },
                move |message| warn.callbacks.log(MAESTRO_LOG_WARN, &message),
            )
            .map_err(|e| {
                shared.callbacks.log(
                    MAESTRO_LOG_WARN,
                    &format!("Settings changes will not apply until reopened ({e})"),
                )
            })
            .ok()
        })
        .collect();

        let control_shared = shared.clone();
        let control = std::thread::Builder::new()
            .name("maestro-api".into())
            .spawn(move || control_loop(control_shared, rx))
            .ok()
            .map(|thread| (tx, thread));

        Ok(Self {
            shared,
            watchers,
            control,
        })
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.watchers.clear();
        if let Some((tx, thread)) = self.control.take() {
            let _ = tx.send(Event::Shutdown);
            let _ = thread.join();
        }
        self.shared.stop();
    }
}

impl Shared {
    fn id(&self) -> usize {
        self as *const Self as usize
    }

    fn start(&self, manager: &MaestroConfigManager, config: &ApiConfig) -> Result<(), Failure> {
        let engine =
            MaestroRealtimeEngine::new(config.engine_options(self.ports)).map_err(|e| {
                Failure::new(
                    MAESTRO_ERR_UNAVAILABLE,
                    format!("The engine could not start: {e}"),
                )
            })?;
        self.load_soundfonts(manager, &engine, &config.sflist);

        let sender = Arc::new(engine.get_event_sender());
        let live = Live {
            stats: engine.get_statistics(),
            engine,
        };
        stats::set(self.id(), Some(live.stats.clone()));
        *self.engine.write().unwrap() = Some(live);
        self.sender.store(Some(sender));
        Ok(())
    }

    fn stop(&self) {
        self.sender.store(None);
        let live = self.engine.write().unwrap().take();
        if live.is_some() {
            drop(live);
            stats::set(self.id(), None);
        }
    }

    fn load_soundfonts(
        &self,
        manager: &MaestroConfigManager,
        engine: &MaestroRealtimeEngine,
        sflist: &str,
    ) {
        let result = manager
            .get_soundfont_list(sflist)
            .map_err(|e| e.to_string())
            .and_then(|list| engine.set_soundfonts(list).map_err(|e| e.to_string()));
        if let Err(e) = result {
            self.callbacks.log(
                MAESTRO_LOG_ERROR,
                &format!("The SoundFonts could not be loaded: {e}"),
            );
        }
    }

    fn reload_soundfonts(&self, manager: &MaestroConfigManager) {
        let sflist = self.config.lock().unwrap().sflist.clone();
        if let Some(live) = self.engine.read().unwrap().as_ref() {
            self.load_soundfonts(manager, &live.engine, &sflist);
        }
    }

    fn reload(&self, manager: &MaestroConfigManager) {
        let global_enabled = profile::load_global(manager).is_ok_and(|g| g.enabled);
        let new = match profile::load_app(manager, &self.app_id) {
            Ok(config) => config,
            Err(e) => {
                self.callbacks.log(
                    MAESTRO_LOG_WARN,
                    &format!(
                        "Settings changed but could not be read ({e}); keeping the previous ones"
                    ),
                );
                return;
            }
        };
        let old = std::mem::replace(&mut *self.config.lock().unwrap(), new.clone());
        let running = self.engine.read().unwrap().is_some();

        if !global_enabled || !new.enabled {
            if running {
                self.stop();
                self.callbacks.notify(MAESTRO_NOTIFY_ENGINE_STOPPED);
            }
            return;
        }

        if !running {
            match self.start(manager, &new) {
                Ok(()) => self.callbacks.notify(MAESTRO_NOTIFY_ENGINE_STARTED),
                Err(e) => self.callbacks.log(MAESTRO_LOG_ERROR, &e.to_string()),
            }
            return;
        }

        let mut rebuilt = false;
        if !old.same_engine(&new) && !self.reload_in_place(&new) {
            self.stop();
            if let Err(e) = self.start(manager, &new) {
                self.callbacks.log(MAESTRO_LOG_ERROR, &e.to_string());
                self.callbacks.notify(MAESTRO_NOTIFY_ENGINE_STOPPED);
                return;
            }
            rebuilt = true;
        }
        if !rebuilt && old.sflist != new.sflist {
            self.reload_soundfonts(manager);
        }
        if old != new {
            self.callbacks.notify(MAESTRO_NOTIFY_SETTINGS_CHANGED);
        }
    }

    fn reload_in_place(&self, config: &ApiConfig) -> bool {
        let mut engine = self.engine.write().unwrap();
        let Some(live) = engine.as_mut() else {
            return false;
        };
        match live.engine.reload_config(config.engine_options(self.ports)) {
            Ok(applied) => applied,
            Err(e) => {
                self.callbacks.log(
                    MAESTRO_LOG_WARN,
                    &format!("The engine could not be reconfigured in place ({e}); restarting it"),
                );
                false
            }
        }
    }

    pub fn with_sender(
        &self,
        port: Option<u8>,
        send: impl FnOnce(&RealtimeEventSender),
    ) -> Result<(), Failure> {
        if let Some(port) = port
            && port >= self.ports
        {
            return Err(Failure::invalid(format!(
                "port {port} is out of range; this instance has {} ports",
                self.ports
            )));
        }
        let sender = self.sender.load();
        send(sender.as_deref().ok_or_else(Failure::not_running)?);
        Ok(())
    }
}

fn control_loop(shared: Arc<Shared>, rx: Receiver<Event>) {
    let manager = MaestroConfigManager::default();
    while let Ok(event) = rx.recv() {
        match debounce(&rx, event) {
            Event::Shutdown => return,
            Event::Config => shared.reload(&manager),
            Event::Soundfonts => shared.reload_soundfonts(&manager),
        }
    }
}

fn debounce(rx: &Receiver<Event>, first: Event) -> Event {
    if first == Event::Shutdown {
        return first;
    }

    let mut result = first;
    let deadline = Instant::now() + DEBOUNCE;
    loop {
        let timeout = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(timeout) {
            Ok(Event::Shutdown) => return Event::Shutdown,
            Ok(Event::Config) => result = Event::Config,
            Ok(Event::Soundfonts) => {}
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return result,
        }
    }
}
