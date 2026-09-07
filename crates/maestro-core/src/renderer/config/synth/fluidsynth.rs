#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct FluidSynthConfig {
    pub voice_limit: u32,
    pub interpolation: FluidSynthInterpolation,

    pub minimum_note_length: u32,
    pub system_id: i32,

    pub chorus_active: bool,
    pub reverb_active: bool,

    pub overflow_age: f64,
    pub overflow_percussion: f64,
    pub overflow_released: f64,
    pub overflow_sustained: f64,
    pub overflow_volume: f64,
}

impl Default for FluidSynthConfig {
    fn default() -> Self {
        Self {
            voice_limit: 256,
            interpolation: Default::default(),
            minimum_note_length: 10,
            system_id: 0,
            chorus_active: true,
            reverb_active: true,
            overflow_age: 1000.0,
            overflow_percussion: 4000.0,
            overflow_released: -2000.0,
            overflow_sustained: -1000.0,
            overflow_volume: 500.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[repr(i32)]
pub enum FluidSynthInterpolation {
    None = 0,
    #[default]
    Linear = 1,
    Sinc4th = 2,
    Sinc7th = 3,
}

impl TryFrom<i32> for FluidSynthInterpolation {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(FluidSynthInterpolation::None),
            1 => Ok(FluidSynthInterpolation::Linear),
            2 => Ok(FluidSynthInterpolation::Sinc4th),
            3 => Ok(FluidSynthInterpolation::Sinc7th),
            _ => Err(format!("Unsupported interpolation type: {}", value)),
        }
    }
}

impl From<FluidSynthInterpolation> for i32 {
    fn from(value: FluidSynthInterpolation) -> Self {
        value as i32
    }
}
