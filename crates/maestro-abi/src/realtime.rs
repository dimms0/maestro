use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use maestro_core::{
    realtime::{MaestroRealtimeEngine, MaestroRealtimeStatistics, RealtimeEventSender, UMP_GROUPS},
    system_cfg::{
        MaestroConfigManager,
        watcher::{ConfigWatchEvent, ConfigWatcher},
    },
};

use crate::abi::*;
use crate::app::{App, Callbacks};
use crate::error::Failure;
use crate::profile::{self, ApiConfig};
use crate::{sized, stats};

const DEBOUNCE: Duration = Duration::from_millis(300);

pub struct Live {
    pub engine: MaestroRealtimeEngine,
    pub stats: MaestroRealtimeStatistics,
}

// The engine owns a cpal stream, which is not Send/Sync on every platform; it
// is only reached through the engine lock, and mutated only under its write side.
unsafe impl Send for Live {}
unsafe impl Sync for Live {}

pub struct Shared {
    pub app_id: String,
    pub ports: u8,
    pub midi2: bool,
    pub engine: RwLock<Option<Live>>,
    sender: AtomicPtr<RealtimeEventSender>,
    // The previous one is kept, so a send racing a restart never reaches a freed sender
    senders: Mutex<[Option<Box<RealtimeEventSender>>; 2]>,
    pub config: Mutex<ApiConfig>,
    callbacks: Callbacks,
}

#[derive(Clone, Copy, PartialEq)]
enum Event {
    Config,
    Soundfonts,
    Shutdown,
}

pub struct Realtime {
    pub shared: Arc<Shared>,
    watchers: Vec<ConfigWatcher>,
    control: Option<(Sender<Event>, JoinHandle<()>)>,
}

impl Realtime {
    /// # Safety
    /// `app` and `params` must be NULL or point at least their `struct_size`
    /// readable bytes.
    pub unsafe fn open(
        app: *const MaestroAppInfo,
        params: *const MaestroRealtimeParams,
    ) -> Result<Self, Failure> {
        let mut app = unsafe { App::read(app) }?;
        let params = unsafe { sized::read(params) };
        sized::reserved(params.reserved)?;
        app.callbacks.notify = params.notify;
        let midi2 = params.midi2 != 0;

        let ports = match params.num_ports {
            _ if midi2 => UMP_GROUPS,
            0 => 1,
            n @ 1..=16 => n as u8,
            n => return Err(Failure::invalid(format!("num_ports must be 1-16, not {n}"))),
        };

        let manager = MaestroConfigManager::default();
        let config = app.profile(&manager)?;

        let shared = Arc::new(Shared {
            app_id: app.id,
            ports,
            midi2,
            engine: RwLock::new(None),
            sender: AtomicPtr::default(),
            senders: Mutex::default(),
            config: Mutex::new(config.clone()),
            callbacks: app.callbacks,
        });
        shared.start(&manager, &config)?;

        let (tx, rx) = channel();
        let watchers = [(
            manager.get_app_config_path(&shared.app_id),
            manager.sflist_dirs(),
        )]
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

impl Drop for Realtime {
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

        let sender = Box::new(engine.get_event_sender());
        let live = Live {
            stats: engine.get_statistics(),
            engine,
        };
        stats::set(self.id(), Some(live.stats.clone()));
        *self.engine.write().unwrap() = Some(live);
        self.sender
            .store(std::ptr::from_ref(&*sender).cast_mut(), Ordering::Release);
        let mut senders = self.senders.lock().unwrap();
        senders[0] = senders[1].replace(sender);
        Ok(())
    }

    fn stop(&self) {
        self.sender.store(std::ptr::null_mut(), Ordering::Release);
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

        if !new.enabled {
            if running {
                self.stop();
                self.callbacks.notify(MAESTRO_NOTIFY_ENGINE_DISABLED);
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

    pub fn sender(&self) -> Option<&RealtimeEventSender> {
        unsafe { self.sender.load(Ordering::Acquire).as_ref() }
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
        send(self.sender().ok_or_else(Failure::not_running)?);
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
