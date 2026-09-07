use std::sync::mpsc::Sender;

use maestro_core::{
    ipc::SharedStats,
    realtime::{MaestroRealtimeEngine, RealtimeEngineOptions, UMP_GROUPS},
    soundfont::SoundFontList,
};

use crate::{
    config::MaestroSystemConfig,
    error::DaemonError,
    gate::{GateMode, MidiGate, SharedGate},
    log_info,
    watcher::ServiceEvent,
};

pub fn engine_ports(config: &MaestroSystemConfig) -> u8 {
    if config.custom.midi2_enabled {
        UMP_GROUPS
    } else {
        config.custom.num_ports.max(1)
    }
}

pub struct EngineController {
    engine: Option<MaestroRealtimeEngine>,
    gate: SharedGate,
    stats: SharedStats,
}

impl EngineController {
    pub fn new(control: Sender<ServiceEvent>) -> Self {
        Self {
            engine: None,
            gate: MidiGate::new(control),
            stats: SharedStats::default(),
        }
    }

    pub fn gate(&self) -> SharedGate {
        self.gate.clone()
    }

    pub fn shared_stats(&self) -> SharedStats {
        self.stats.clone()
    }

    pub fn is_running(&self) -> bool {
        self.engine.is_some()
    }

    pub fn start(
        &mut self,
        config: &MaestroSystemConfig,
        soundfonts: SoundFontList,
    ) -> Result<(), DaemonError> {
        self.gate.demote(GateMode::Starting);
        self.drop_engine();

        let ports = engine_ports(config);
        let options = RealtimeEngineOptions {
            ports: Some(ports),
            config: config.realtime.clone(),
            renderer: config.renderer,
            audio_params: config.audio_params,
            event_processor: config.event_processor.clone(),
            post_processor: config.post_processor,
        };

        let built = (|| -> Result<MaestroRealtimeEngine, DaemonError> {
            let engine = MaestroRealtimeEngine::new(options)?;
            engine.set_soundfonts(soundfonts)?;
            Ok(engine)
        })();

        let engine = match built {
            Ok(engine) => engine,
            Err(err) => {
                self.gate.demote(GateMode::Off);
                return Err(err);
            }
        };

        self.gate.promote(engine.get_event_sender());
        *self.stats.lock().unwrap() = Some(engine.get_statistics());
        self.engine = Some(engine);
        log_info!("Realtime engine started with {ports} port(s)");
        Ok(())
    }

    pub fn stop(&mut self, mode: GateMode) {
        let was_running = self.engine.is_some();
        self.gate.demote(mode);
        self.drop_engine();
        if was_running {
            log_info!("Realtime engine stopped ({mode:?})");
        }
    }

    fn drop_engine(&mut self) {
        if self.engine.take().is_some() {
            *self.stats.lock().unwrap() = None;
        }
    }

    pub fn set_soundfonts(&mut self, soundfonts: SoundFontList) -> Result<(), DaemonError> {
        if let Some(engine) = &self.engine {
            engine.set_soundfonts(soundfonts)?;
            log_info!("Soundfonts reloaded");
        }
        Ok(())
    }
}
