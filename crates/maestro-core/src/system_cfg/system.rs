use serde::{Deserialize, Serialize};

// This needs to be here so it can ba shared to both daemon and the GUI...
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct SystemCustomSettings {
    pub device_name: String,
    pub num_ports: u8,

    pub midi2_enabled: bool,

    pub idle_timeout_minutes: u32,
}

impl Default for SystemCustomSettings {
    fn default() -> Self {
        Self {
            device_name: "Maestro".to_string(),
            num_ports: 4,
            midi2_enabled: false,
            idle_timeout_minutes: 10,
        }
    }
}
