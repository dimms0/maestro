use crate::error::DaemonError;
use crate::gate::SharedGate;

#[cfg(target_os = "linux")]
mod alsa;
#[cfg(target_os = "macos")]
mod coremidi;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSpec {
    pub device_name: String,
    pub num_ports: u8,
    pub midi2: bool,
}

impl DeviceSpec {
    pub fn from_config(config: &crate::config::MaestroSystemConfig) -> Self {
        Self {
            device_name: config.custom.device_name.clone(),
            num_ports: config.custom.num_ports.max(1),
            midi2: config.custom.midi2_enabled,
        }
    }
}

pub trait MidiBackend: Send {
    fn describe(&self) -> String;

    fn has_devices_connected(&self) -> bool {
        true
    }
}

#[cfg(target_os = "linux")]
pub fn create_backend(
    spec: &DeviceSpec,
    gate: SharedGate,
) -> Result<Box<dyn MidiBackend>, DaemonError> {
    Ok(Box::new(alsa::AlsaBackend::new(spec, gate)?))
}

#[cfg(target_os = "macos")]
pub fn create_backend(
    spec: &DeviceSpec,
    gate: SharedGate,
) -> Result<Box<dyn MidiBackend>, DaemonError> {
    Ok(Box::new(coremidi::CoreMidiBackend::new(spec, gate)?))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn create_backend(
    _spec: &DeviceSpec,
    _gate: SharedGate,
) -> Result<Box<dyn MidiBackend>, DaemonError> {
    Err(DaemonError::Backend(
        "The daemon service backend is only available on Linux and macOS; \
         on Windows the driver DLL is loaded by the system instead"
            .to_string(),
    ))
}
