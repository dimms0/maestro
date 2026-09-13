#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct FluidSynthConfig {
    pub voice_limit: u32,
    pub interpolation: FluidSynthInterpolation,
    pub cpu_cores: i32,
    pub midi_bank_select: FluidSynthBankSelect,
    pub portamento_time: FluidSynthPortamentoTime,

    pub minimum_note_length: u32,
    pub system_id: i32,
    pub note_cut: i32,

    pub chorus_active: bool,
    pub chorus_depth: f32,
    pub chorus_level: f32,
    pub chorus_speed: f32,

    pub reverb_active: bool,
    pub reverb_damp: f32,
    pub reverb_engine: FluidSynthReverbEngine,
    pub reverb_level: f32,
    pub reverb_roomsize: f32,
    pub reverb_width: f32,

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
            cpu_cores: 1,
            midi_bank_select: Default::default(),
            portamento_time: Default::default(),
            minimum_note_length: 10,
            system_id: 0,
            note_cut: 0,
            chorus_active: true,
            chorus_depth: 4.25,
            chorus_level: 0.6,
            chorus_speed: 0.2,
            reverb_active: true,
            reverb_damp: 0.2,
            reverb_engine: Default::default(),
            reverb_level: 0.7,
            reverb_roomsize: 0.5,
            reverb_width: 0.8,
            overflow_age: 1000.0,
            overflow_percussion: 4000.0,
            overflow_released: -2000.0,
            overflow_sustained: -1000.0,
            overflow_volume: 500.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum FluidSynthBankSelect {
    #[default]
    GS,
    GM,
    GM2,
    XG,
    MMA,
}

impl Into<&str> for FluidSynthBankSelect {
    fn into(self) -> &'static str {
        match self {
            FluidSynthBankSelect::GS => "gs",
            FluidSynthBankSelect::GM => "gm",
            FluidSynthBankSelect::GM2 => "gm2",
            FluidSynthBankSelect::XG => "xg",
            FluidSynthBankSelect::MMA => "mma",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum FluidSynthPortamentoTime {
    #[default]
    Auto,
    Linear,
    XgGs,
}

impl Into<&str> for FluidSynthPortamentoTime {
    fn into(self) -> &'static str {
        match self {
            FluidSynthPortamentoTime::Auto => "auto",
            FluidSynthPortamentoTime::Linear => "linear",
            FluidSynthPortamentoTime::XgGs => "XgGs",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum FluidSynthReverbEngine {
    Freeverb,
    FDN,
    Lexverb,
    #[default]
    Dattorro,
    Signalsmith,
}

impl Into<&str> for FluidSynthReverbEngine {
    fn into(self) -> &'static str {
        match self {
            FluidSynthReverbEngine::Freeverb => "free",
            FluidSynthReverbEngine::FDN => "fdn",
            FluidSynthReverbEngine::Lexverb => "lex",
            FluidSynthReverbEngine::Dattorro => "dat",
            FluidSynthReverbEngine::Signalsmith => "signal",
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
