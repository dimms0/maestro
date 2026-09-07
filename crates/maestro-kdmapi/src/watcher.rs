use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use maestro_core::system_cfg::watcher::{ConfigWatchEvent, ConfigWatcher};
use maestro_core::system_cfg::{ConfigComponent, MaestroConfigManager};

use crate::config::MaestroKdmapiConfig;
use crate::state::get_state;
use crate::{log_error, log_info, start_engine, stop_engine};

const DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchEvent {
    ConfigChanged,
    SoundfontsChanged,
    Shutdown,
}

pub struct WatcherHandle {
    tx: Sender<WatchEvent>,
    thread: Option<JoinHandle<()>>,
    _watcher: ConfigWatcher,
}

impl WatcherHandle {
    pub fn shutdown(mut self) {
        let _ = self.tx.send(WatchEvent::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn spawn(config: MaestroKdmapiConfig) -> Option<WatcherHandle> {
    let cfg_manager = MaestroConfigManager::default();
    let config_path = cfg_manager.get_component_config_path(&ConfigComponent::KDMAPI);
    let (tx, rx) = channel();

    let watcher_tx = tx.clone();
    let watcher = match ConfigWatcher::spawn(
        config_path.clone(),
        cfg_manager.sflist_dirs(),
        move |event| {
            let msg = match event {
                ConfigWatchEvent::ConfigChanged => WatchEvent::ConfigChanged,
                ConfigWatchEvent::SoundfontsChanged => WatchEvent::SoundfontsChanged,
            };
            let _ = watcher_tx.send(msg);
        },
        log_error,
    ) {
        Ok(w) => w,
        Err(err) => {
            log_error(err);
            return None;
        }
    };

    let thread = std::thread::Builder::new()
        .name("maestro-kdmapi-reload".to_string())
        .spawn(move || control_loop(rx, cfg_manager, config_path, config))
        .ok()?;

    Some(WatcherHandle {
        tx,
        thread: Some(thread),
        _watcher: watcher,
    })
}

fn control_loop(
    rx: Receiver<WatchEvent>,
    cfg_manager: MaestroConfigManager,
    config_path: PathBuf,
    mut config: MaestroKdmapiConfig,
) {
    loop {
        let event = match rx.recv() {
            Ok(e) => e,
            Err(_) => return,
        };

        match debounce(&rx, event) {
            WatchEvent::Shutdown => return,
            WatchEvent::ConfigChanged => reload_config(&cfg_manager, &config_path, &mut config),
            WatchEvent::SoundfontsChanged => reload_soundfonts(&cfg_manager, &config),
        }
    }
}

fn debounce(rx: &Receiver<WatchEvent>, first: WatchEvent) -> WatchEvent {
    if first == WatchEvent::Shutdown {
        return first;
    }

    let mut result = first;
    let deadline = Instant::now() + DEBOUNCE;
    loop {
        let now = Instant::now();
        let timeout = deadline.saturating_duration_since(now);
        match rx.recv_timeout(timeout) {
            Ok(WatchEvent::Shutdown) => return WatchEvent::Shutdown,
            Ok(WatchEvent::ConfigChanged) => result = WatchEvent::ConfigChanged,
            Ok(WatchEvent::SoundfontsChanged) => {}
            Err(RecvTimeoutError::Timeout) => return result,
            Err(RecvTimeoutError::Disconnected) => return result,
        }
        if timeout.is_zero() {
            return result;
        }
    }
}

fn reload_config(
    cfg_manager: &MaestroConfigManager,
    config_path: &PathBuf,
    config: &mut MaestroKdmapiConfig,
) {
    let new: MaestroKdmapiConfig = match cfg_manager.load_or_default(config_path) {
        Ok(c) => c,
        Err(err) => {
            log_error(format!(
                "Config file changed but failed to load ({err}); keeping previous settings"
            ));
            return;
        }
    };

    let engine_changed = config.audio_params != new.audio_params
        || config.realtime != new.realtime
        || config.renderer != new.renderer
        || config.event_processor != new.event_processor
        || config.post_processor != new.post_processor;
    let sflist_changed = config.sflist != new.sflist;
    let enabled_changed = config.enabled != new.enabled;
    *config = new;

    if !engine_changed && !sflist_changed && !enabled_changed {
        return;
    }

    if !config.enabled {
        log_info("KDMAPI disabled by config change; stopping engine");
        stop_engine();
        return;
    }

    let running = get_state().realtime.lock().unwrap().is_some();
    if engine_changed || !running {
        log_info("Config changed; rebuilding engine");
        stop_engine();
        if !start_engine(config) {
            log_error("Failed to restart engine after config change");
        }
    } else if sflist_changed {
        reload_soundfonts(cfg_manager, config);
    }
}

fn reload_soundfonts(cfg_manager: &MaestroConfigManager, config: &MaestroKdmapiConfig) {
    let state = get_state();
    let realtime = state.realtime.lock().unwrap();
    let Some(engine) = realtime.as_ref() else {
        return;
    };

    match cfg_manager.get_soundfont_list(&config.sflist) {
        Ok(sflist) => {
            log_info("Soundfont list changed; reloading");
            if let Err(err) = engine.set_soundfonts(sflist) {
                log_error(err);
            }
        }
        Err(err) => log_error(err),
    }
}
