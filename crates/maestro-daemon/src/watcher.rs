use std::path::PathBuf;
use std::sync::mpsc::Sender;

use maestro_core::system_cfg::watcher::{ConfigWatchEvent, ConfigWatcher as CoreConfigWatcher};

use crate::{error::DaemonError, log_warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceEvent {
    ConfigChanged,
    SoundfontsChanged,

    WakeEngine,

    Shutdown,
}

pub struct ConfigWatcher {
    _watcher: CoreConfigWatcher,
}

impl ConfigWatcher {
    pub fn spawn(
        config_path: PathBuf,
        sflist_dirs: Vec<PathBuf>,
        tx: Sender<ServiceEvent>,
    ) -> Result<Self, DaemonError> {
        let watcher = CoreConfigWatcher::spawn(
            config_path,
            sflist_dirs,
            move |event| {
                let msg = match event {
                    ConfigWatchEvent::ConfigChanged => ServiceEvent::ConfigChanged,
                    ConfigWatchEvent::SoundfontsChanged => ServiceEvent::SoundfontsChanged,
                };
                let _ = tx.send(msg);
            },
            |warning| log_warn!("{warning}"),
        )?;

        Ok(Self { _watcher: watcher })
    }
}
