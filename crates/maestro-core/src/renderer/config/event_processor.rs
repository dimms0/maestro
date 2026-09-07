#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct EventProcessorConfig {
    pub bypass_ports: Box<[u8]>,
    pub ignore_ports: Box<[u8]>,

    pub bypass_channels: Box<[u8]>,
    pub ignore_channels: Box<[u8]>,

    pub transpose: i8,

    pub key_range_low: u8,
    pub key_range_high: u8,

    pub fixed_velocity: u8,
    pub velocity_multiplier: f32,
    pub velocity_threshold: u8,
    pub velocity_curve: f32,

    pub ignore_program_change: bool,
    pub ignore_sysex: bool,
}

impl Default for EventProcessorConfig {
    fn default() -> Self {
        Self {
            bypass_ports: vec![].into_boxed_slice(),
            ignore_ports: vec![].into_boxed_slice(),
            bypass_channels: vec![9].into_boxed_slice(),
            ignore_channels: vec![].into_boxed_slice(),
            fixed_velocity: 0,
            transpose: 0,
            key_range_low: 0,
            key_range_high: 127,
            velocity_multiplier: 1.0,
            velocity_threshold: 0,
            velocity_curve: 1.0,
            ignore_program_change: false,
            ignore_sysex: false,
        }
    }
}
