use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use arc_swap::ArcSwapOption;
use maestro_core::{
    ipc::{SharedStats, process_name, publish},
    realtime::{MaestroRealtimeEngine, RealtimeEngineOptions, RealtimeEventSender},
    system_cfg::{ConfigComponent, MaestroConfigManager},
};

use super::ffi::*;
use super::{log_driver_error, log_driver_warn};
use crate::{
    config::MaestroSystemConfig,
    service::diff_configs,
    watcher::{ConfigWatcher, ServiceEvent},
};

// Boxed and handed to winmm through `dwUser`,
// so every open/close pair owns exactly one allocation
pub struct OutClient {
    pub port: u8,
    pub callback: usize,
    pub instance: usize,
    pub hmidi: usize,
    pub flags: u32,
}

pub struct DriverState {
    config: Mutex<MaestroSystemConfig>,
    engine: Mutex<Option<MaestroRealtimeEngine>>,
    pub sender: ArcSwapOption<RealtimeEventSender>,
    stats: SharedStats,
    engine_users: Mutex<usize>,
    watcher_started: Mutex<bool>,
}

unsafe impl Send for DriverState {}
unsafe impl Sync for DriverState {}

pub(super) fn get_state() -> &'static DriverState {
    static STATE: OnceLock<DriverState> = OnceLock::new();
    STATE.get_or_init(|| {
        let config = load_config().unwrap_or_else(|err| {
            log_driver_error("failed to load config, using defaults", err);
            MaestroSystemConfig::default()
        });

        let stats = SharedStats::default();

        if let Err(err) = publish(
            &format!("WinMM — {}", process_name()),
            stats.clone(),
            &ConfigComponent::System,
            Default::default(),
        ) {
            log_driver_warn("statistics are unavailable", err);
        }

        DriverState {
            config: Mutex::new(config),
            engine: Mutex::new(None),
            sender: ArcSwapOption::empty(),
            stats,
            engine_users: Mutex::new(0),
            watcher_started: Mutex::new(false),
        }
    })
}

fn load_config() -> Result<MaestroSystemConfig, maestro_core::error::ConfigError> {
    let manager = MaestroConfigManager::default();
    let path = manager.get_component_config_path(&ConfigComponent::System);
    manager.load_or_default(&path)
}

fn utf16_name(name: &str) -> [u16; 32] {
    let mut buf = [0u16; 32];
    for (i, unit) in name.encode_utf16().take(31).enumerate() {
        buf[i] = unit;
    }
    buf
}

impl DriverState {
    fn config(&self) -> MaestroSystemConfig {
        self.config.lock().unwrap().clone()
    }

    // Device tables

    pub fn num_output_devices(&self) -> u32 {
        self.config().custom.num_ports.max(1) as u32
    }

    pub fn output_caps(&self, device: u32) -> Option<MIDIOUTCAPSW> {
        if device >= self.num_output_devices() {
            return None;
        }
        let config = self.config();

        let name = format!("{} - Port {}", config.custom.device_name, device + 1);

        Some(MIDIOUTCAPSW {
            wMid: 0,
            wPid: 0,
            vDriverVersion: 0x0100,
            szPname: utf16_name(&name),
            wTechnology: MOD_SWSYNTH as u16,
            wVoices: u16::MAX,
            wNotes: u16::MAX,
            wChannelMask: 0xFFFF,
            dwSupport: 0,
        })
    }

    // Output devices

    pub fn open_output(
        &self,
        device: u32,
        desc: &MIDIOPENDESC,
        flags: u32,
    ) -> Result<Box<OutClient>, u32> {
        if device >= self.num_output_devices() {
            return Err(MMSYSERR_BADDEVICEID);
        }

        if let Err(code) = self.retain_engine() {
            return Err(code);
        }
        self.ensure_watcher();

        Ok(Box::new(OutClient {
            port: device as u8,
            callback: desc.dwCallback,
            instance: desc.dwInstance,
            hmidi: desc.hMidi,
            flags,
        }))
    }

    pub fn close_output(&self) {
        self.release_engine();
    }

    // Engine lifecycle

    fn retain_engine(&self) -> Result<(), u32> {
        let mut users = self.engine_users.lock().unwrap();
        if *users == 0 {
            let config = self.config();
            if let Err(code) = self.start_engine(&config) {
                return Err(code);
            }
        }
        *users += 1;
        Ok(())
    }

    fn release_engine(&self) {
        let mut users = self.engine_users.lock().unwrap();
        *users = users.saturating_sub(1);
        if *users == 0 {
            self.sender.store(None);
            *self.stats.lock().unwrap() = None;
            *self.engine.lock().unwrap() = None;
        }
    }

    fn start_engine(&self, config: &MaestroSystemConfig) -> Result<(), u32> {
        let manager = MaestroConfigManager::default();
        let soundfonts = match manager.get_soundfont_list(&config.sflist) {
            Ok(s) => s,
            Err(err) => {
                log_driver_error("failed to load soundfont list", err);
                return Err(MMSYSERR_ERROR_CODE);
            }
        };

        let options = RealtimeEngineOptions {
            ports: Some(config.custom.num_ports.max(1)),
            config: config.realtime.clone(),
            renderer: config.renderer.clone(),
            audio_params: config.audio_params,
            event_processor: config.event_processor.clone(),
            post_processor: config.post_processor.clone(),
        };

        let engine = match MaestroRealtimeEngine::new(options) {
            Ok(e) => e,
            Err(err) => {
                log_driver_error("failed to start engine", err);
                return Err(MMSYSERR_NOMEM);
            }
        };
        if let Err(err) = engine.set_soundfonts(soundfonts) {
            log_driver_error("failed to load soundfonts", err);
        }

        self.sender.store(Some(Arc::new(engine.get_event_sender())));
        *self.stats.lock().unwrap() = Some(engine.get_statistics());
        *self.engine.lock().unwrap() = Some(engine);
        Ok(())
    }

    fn restart_engine_if_running(&self, config: &MaestroSystemConfig) {
        let users = self.engine_users.lock().unwrap();
        if *users == 0 {
            return;
        }

        self.sender.store(None);
        *self.stats.lock().unwrap() = None;
        *self.engine.lock().unwrap() = None;
        let _ = self.start_engine(config);
    }

    // Config watching

    fn ensure_watcher(&self) {
        let mut started = self.watcher_started.lock().unwrap();
        if *started {
            return;
        }
        *started = true;

        std::thread::Builder::new()
            .name("maestro-drv-watch".to_string())
            .spawn(move || watcher_loop(get_state()))
            .ok();
    }

    fn apply_config_change(&self) {
        let new = match load_config() {
            Ok(c) => c,
            Err(err) => {
                log_driver_warn("config changed but failed to load", err);
                return;
            }
        };
        let old = std::mem::replace(&mut *self.config.lock().unwrap(), new.clone());
        let diff = diff_configs(&old, &new);

        if diff.engine {
            self.restart_engine_if_running(&new);
        } else if diff.soundfonts {
            self.reload_soundfonts(&new);
        }
    }

    fn reload_soundfonts(&self, config: &MaestroSystemConfig) {
        let engine = self.engine.lock().unwrap();
        if let Some(engine) = engine.as_ref() {
            let manager = MaestroConfigManager::default();
            match manager.get_soundfont_list(&config.sflist) {
                Ok(soundfonts) => {
                    if let Err(err) = engine.set_soundfonts(soundfonts) {
                        log_driver_error("failed to reload soundfonts", err);
                    }
                }
                Err(err) => log_driver_error("failed to load soundfont list", err),
            }
        }
    }
}

const MMSYSERR_ERROR_CODE: u32 = 1; // MMSYSERR_ERROR

fn watcher_loop(state: &'static DriverState) {
    let manager = MaestroConfigManager::default();
    let config_path = manager.get_component_config_path(&ConfigComponent::System);

    let sflist_dirs = manager.sflist_dirs();

    let (tx, rx) = channel();
    let _watcher = match ConfigWatcher::spawn(config_path, sflist_dirs, tx) {
        Ok(w) => w,
        Err(err) => {
            log_driver_warn("config watcher unavailable", err);
            return;
        }
    };

    loop {
        match rx.recv() {
            Ok(ServiceEvent::ConfigChanged) => {
                while rx.recv_timeout(Duration::from_millis(300)).is_ok() {}
                state.apply_config_change();
            }
            Ok(ServiceEvent::SoundfontsChanged) => {
                while rx.recv_timeout(Duration::from_millis(300)).is_ok() {}
                let config = state.config();
                state.reload_soundfonts(&config);
            }
            Ok(ServiceEvent::Shutdown) | Err(_) => return,
            _ => {}
        }
    }
}
