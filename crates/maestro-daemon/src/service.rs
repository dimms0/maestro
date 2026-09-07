use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use maestro_core::soundfont::SoundFontList;
use maestro_core::system_cfg::{ConfigComponent, MaestroConfigManager};
use maestro_core::{notify, sysinfo};

use crate::{
    config::MaestroSystemConfig,
    engine::{EngineController, engine_ports},
    error::DaemonError,
    gate::GateMode,
    log_error, log_info,
    platform::{DeviceSpec, MidiBackend, create_backend},
    watcher::{ConfigWatcher, ServiceEvent},
};

const DEBOUNCE: Duration = Duration::from_millis(300);
const ACTIVITY_TICK: Duration = Duration::from_secs(5);
const IDLE_PARK: Duration = Duration::from_secs(60 * 60);
const IDLE_REMINDER: Duration = Duration::from_secs(30 * 60);

pub struct Service {
    cfg_manager: MaestroConfigManager,
    config_path: PathBuf,
    config: MaestroSystemConfig,
    engine: EngineController,
    backend: Option<Box<dyn MidiBackend>>,
    last_activity: Instant,
    reminded: bool,
    rx: Receiver<ServiceEvent>,
    tx: Sender<ServiceEvent>,
    _watcher: Option<ConfigWatcher>,
}

impl Service {
    pub fn new() -> Result<Self, DaemonError> {
        let cfg_manager = MaestroConfigManager::default();
        let config_path = cfg_manager.get_component_config_path(&ConfigComponent::System);
        let (tx, rx) = channel();

        let config: MaestroSystemConfig = match cfg_manager.load_or_default(&config_path) {
            Ok(c) => c,
            Err(err) => {
                log_error!(
                    "Failed to load {}: {err}. Using default configuration.",
                    config_path.display()
                );
                MaestroSystemConfig::default()
            }
        };

        Ok(Self {
            cfg_manager,
            config_path,
            config,
            engine: EngineController::new(tx.clone()),
            backend: None,
            last_activity: Instant::now(),
            reminded: false,
            rx,
            tx,
            _watcher: None,
        })
    }

    pub fn event_sender(&self) -> Sender<ServiceEvent> {
        self.tx.clone()
    }

    fn load_soundfonts(&self) -> Result<SoundFontList, DaemonError> {
        Ok(self.cfg_manager.get_soundfont_list(&self.config.sflist)?)
    }

    fn apply_current_config(&mut self) {
        self.recreate_backend();

        if self.idle_timeout().is_some() {
            self.last_activity = Instant::now();
            self.reminded = false;
            self.engine.stop(GateMode::Idle);
            log_info!(
                "Idle on startup enabled; the realtime engine will start on the first MIDI event"
            );
        } else {
            self.start_engine();
        }
    }

    fn recreate_backend(&mut self) {
        self.backend = None;

        match create_backend(&DeviceSpec::from_config(&self.config), self.engine.gate()) {
            Ok(backend) => {
                log_info!("{}", backend.describe());
                self.backend = Some(backend);
            }
            Err(err) => log_error!("Failed to create MIDI devices: {err}"),
        }
    }

    fn start_engine(&mut self) {
        self.last_activity = Instant::now();
        self.reminded = false;

        match self.load_soundfonts() {
            Ok(sf) => match self.engine.start(&self.config, sf) {
                Ok(()) => {
                    let txt = if self.config.custom.idle_timeout_minutes > 0 {
                        "Maestro will start when activity is detected."
                    } else {
                        "Maestro is loaded and running."
                    };

                    notify::send("Maestro is ready", txt);
                }
                Err(err) => {
                    log_error!("Failed to start realtime engine: {err}");
                }
            },
            Err(err) => {
                log_error!("Failed to load soundfont list: {err}");
                self.engine.stop(GateMode::Off);
            }
        }
    }

    fn idle_timeout(&self) -> Option<Duration> {
        let minutes = self.config.custom.idle_timeout_minutes;
        (minutes > 0).then(|| Duration::from_secs(u64::from(minutes) * 60))
    }

    fn next_wait(&self) -> Duration {
        if !self.engine.is_running() {
            return IDLE_PARK;
        }
        let deadline = self.idle_timeout().unwrap_or(IDLE_REMINDER);
        (self.last_activity + deadline)
            .saturating_duration_since(Instant::now())
            .min(ACTIVITY_TICK)
    }

    fn service_idle_state(&mut self) {
        let devices_connected = self
            .backend
            .as_deref()
            .is_none_or(MidiBackend::has_devices_connected);

        if !devices_connected {
            if self.engine.is_running() {
                log_info!("No MIDI devices connected; unloading the realtime engine");
                self.engine.stop(GateMode::Idle);
                notify::send(
                    "Maestro has been paused",
                    "No MIDI devices are connected. Playing anything will load it again.",
                );
            }
            return;
        }

        if !self.engine.is_running() {
            if self.engine.gate().wants_start() {
                log_info!("MIDI arrived while paused; restarting the realtime engine");
                self.start_engine();
            }
            return;
        }

        match self.idle_timeout() {
            Some(timeout) if self.last_activity.elapsed() >= timeout => {
                log_info!(
                    "No MIDI for {} minute(s); stopping the realtime engine",
                    self.config.custom.idle_timeout_minutes
                );
                self.engine.stop(GateMode::Idle);
                notify::send(
                    "Maestro has been paused due to inactivity",
                    "The soundfonts have been unloaded. Playing anything will load them again.",
                );
            }
            None if !self.reminded && self.last_activity.elapsed() >= IDLE_REMINDER => {
                self.reminded = true;
                let using = sysinfo::rss_bytes()
                    .map(|rss| format!(" and is using {}", notify::format_bytes(rss)))
                    .unwrap_or_default();
                notify::send(
                    "Maestro is still running",
                    &format!(
                        "It has been idle for {} minutes{using}.",
                        IDLE_REMINDER.as_secs() / 60
                    ),
                );
            }
            _ => {}
        }
    }

    pub fn run(&mut self) -> Result<(), DaemonError> {
        log_info!(
            "Maestro daemon starting (config: {})",
            self.config_path.display()
        );

        self._watcher = match ConfigWatcher::spawn(
            self.config_path.clone(),
            self.cfg_manager.sflist_dirs(),
            self.tx.clone(),
        ) {
            Ok(w) => Some(w),
            Err(err) => {
                log_error!("Failed to start file watcher, live changes are disabled: {err}");
                None
            }
        };

        let shutdown_tx = self.tx.clone();
        if let Err(err) = maestro_core::ipc::publish(
            maestro_core::ipc::DAEMON_LABEL,
            self.engine.shared_stats(),
            &ConfigComponent::System,
            maestro_core::ipc::PublishOptions {
                state: Some(self.engine.gate().state_cell()),
                on_shutdown: Some(Box::new(move || {
                    let _ = shutdown_tx.send(ServiceEvent::Shutdown);
                })),
            },
        ) {
            log_error!("Failed to publish statistics: {err}");
        }

        self.apply_current_config();

        loop {
            let event = match self.rx.recv_timeout(self.next_wait()) {
                Ok(event) => Some(event),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };

            if self.engine.gate().take_activity() {
                self.last_activity = Instant::now();
                self.reminded = false;
            }

            match event {
                Some(ServiceEvent::Shutdown) => break,
                Some(ServiceEvent::WakeEngine) => {}
                Some(event @ (ServiceEvent::ConfigChanged | ServiceEvent::SoundfontsChanged)) => {
                    match self.debounce(event) {
                        ServiceEvent::Shutdown => break,
                        ServiceEvent::ConfigChanged => self.reload_config(),
                        ServiceEvent::SoundfontsChanged => self.reload_soundfonts(),
                        _ => {}
                    }
                }
                None => {}
            }

            self.service_idle_state();
        }

        log_info!("Shutting down");
        // Backend first so no producer thread is mid-dispatch when the engine
        // goes away
        self.backend = None;
        self.engine.stop(GateMode::Off);
        Ok(())
    }

    fn debounce(&self, first: ServiceEvent) -> ServiceEvent {
        if first == ServiceEvent::Shutdown {
            return first;
        }

        let mut result = first;
        let deadline = Instant::now() + DEBOUNCE;
        loop {
            let now = Instant::now();
            let timeout = deadline.saturating_duration_since(now);
            match self.rx.recv_timeout(timeout) {
                Ok(ServiceEvent::Shutdown) => return ServiceEvent::Shutdown,
                Ok(ServiceEvent::ConfigChanged) => result = ServiceEvent::ConfigChanged,
                Ok(ServiceEvent::SoundfontsChanged) => {}
                // Safe to swallow: the control loop re-derives the start
                // decision from the gate on every iteration.
                Ok(ServiceEvent::WakeEngine) => {}
                Err(RecvTimeoutError::Timeout) => return result,
                Err(RecvTimeoutError::Disconnected) => return result,
            }
            if timeout.is_zero() {
                return result;
            }
        }
    }

    fn reload_config(&mut self) {
        let new: MaestroSystemConfig = match self.cfg_manager.load_or_default(&self.config_path) {
            Ok(c) => c,
            Err(err) => {
                log_error!(
                    "Config file changed but failed to load ({err}); keeping previous settings"
                );
                return;
            }
        };

        let old = std::mem::replace(&mut self.config, new);
        let diff = diff_configs(&old, &self.config);

        if !diff.any() {
            return;
        }
        log_info!(
            "Config changed (engine: {}, devices: {}, soundfonts: {}, idle: {})",
            diff.engine,
            diff.devices,
            diff.soundfonts,
            diff.idle
        );

        if diff.devices {
            self.recreate_backend();
        }

        if diff.engine {
            self.start_engine();
        } else if diff.soundfonts {
            self.reload_soundfonts();
        } else if diff.idle {
            // Only the timeout moved. Re-arm from now rather than restarting.
            self.last_activity = Instant::now();
            self.reminded = false;
        }
    }

    fn reload_soundfonts(&mut self) {
        if !self.engine.is_running() {
            return;
        }
        match self.load_soundfonts() {
            Ok(sf) => {
                if let Err(err) = self.engine.set_soundfonts(sf) {
                    log_error!("Failed to reload soundfonts: {err}");
                }
            }
            Err(err) => log_error!("Failed to load soundfont list: {err}"),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ConfigDiff {
    pub engine: bool,
    pub devices: bool,
    pub soundfonts: bool,
    pub idle: bool,
}

impl ConfigDiff {
    pub fn any(&self) -> bool {
        self.engine || self.devices || self.soundfonts || self.idle
    }
}

pub fn diff_configs(old: &MaestroSystemConfig, new: &MaestroSystemConfig) -> ConfigDiff {
    let engine = engine_ports(old) != engine_ports(new)
        || old.audio_params != new.audio_params
        || old.realtime != new.realtime
        || old.renderer != new.renderer
        || old.event_processor != new.event_processor
        || old.post_processor != new.post_processor;

    let devices = old.custom.device_name != new.custom.device_name
        || old.custom.midi2_enabled != new.custom.midi2_enabled
        || (!new.custom.midi2_enabled && old.custom.num_ports != new.custom.num_ports);

    let soundfonts = old.sflist != new.sflist;

    // Deliberately not part of `engine`: changing the timeout must re-arm the
    // clock, never rebuild the engine and interrupt playback.
    let idle = old.custom.idle_timeout_minutes != new.custom.idle_timeout_minutes;

    ConfigDiff {
        engine,
        devices,
        soundfonts,
        idle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_configs_produce_no_diff() {
        let a = MaestroSystemConfig::default();
        let b = MaestroSystemConfig::default();
        assert!(!diff_configs(&a, &b).any());
    }

    #[test]
    fn port_count_change_rebuilds_engine_and_devices() {
        let a = MaestroSystemConfig::default();
        let mut b = MaestroSystemConfig::default();
        b.custom.num_ports = 8;
        let diff = diff_configs(&a, &b);
        assert!(diff.engine);
        assert!(diff.devices);
        assert!(!diff.soundfonts);
    }

    #[test]
    fn port_count_is_ignored_in_midi2_mode() {
        let mut a = MaestroSystemConfig::default();
        a.custom.midi2_enabled = true;
        let mut b = a.clone();
        b.custom.num_ports = 12;
        assert!(!diff_configs(&a, &b).any());
    }

    #[test]
    fn midi2_toggle_changes_engine_and_devices() {
        let a = MaestroSystemConfig::default();
        let mut b = MaestroSystemConfig::default();
        b.custom.midi2_enabled = true;
        let diff = diff_configs(&a, &b);
        assert!(diff.engine); // port count changes to 16 groups
        assert!(diff.devices);
    }

    #[test]
    fn sflist_change_only_reloads_soundfonts() {
        let a = MaestroSystemConfig::default();
        let mut b = MaestroSystemConfig::default();
        b.sflist = "other".to_string();
        let diff = diff_configs(&a, &b);
        assert!(!diff.engine);
        assert!(!diff.devices);
        assert!(diff.soundfonts);
    }

    #[test]
    fn idle_timeout_change_is_seen_and_restarts_nothing() {
        let a = MaestroSystemConfig::default();
        let mut b = MaestroSystemConfig::default();
        b.custom.idle_timeout_minutes = 45;
        let diff = diff_configs(&a, &b);
        assert!(diff.any());
        assert!(diff.idle);
        assert!(!diff.engine);
        assert!(!diff.devices);
        assert!(!diff.soundfonts);
    }
}
