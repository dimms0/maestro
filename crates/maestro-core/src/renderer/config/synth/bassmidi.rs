#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct BASSMIDIConfig {
    pub voice_limit: u32,
    pub render_time_limit: f32,
    pub interpolation: BASSMIDIInterpolation,

    pub multithreading: Option<BASSMIDIThreading>,

    pub disable_effects: bool,
    pub fade_out_killing: bool,
    pub follow_overlaps: bool,

    pub sf_linear_attack_mod: bool,
    pub sf_linear_decay_vol: bool,
    pub sf_minfx: bool,
    pub sf_nofx: bool,
    pub sf_no_rampin: bool,
    pub sf_sb_limits: bool,
    pub sf_xg_drums: bool,
}

impl Default for BASSMIDIConfig {
    fn default() -> Self {
        Self {
            disable_effects: false,
            fade_out_killing: true,
            follow_overlaps: false,
            interpolation: Default::default(),
            multithreading: Default::default(),
            render_time_limit: 0.0,
            voice_limit: 1024,
            sf_linear_attack_mod: false,
            sf_linear_decay_vol: false,
            sf_minfx: false,
            sf_no_rampin: false,
            sf_sb_limits: false,
            sf_nofx: false,
            sf_xg_drums: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct BASSMIDIThreading {
    pub thread_count: Option<usize>,
    pub keyboard_divisions: u8,
}

impl Default for BASSMIDIThreading {
    fn default() -> Self {
        Self {
            thread_count: None,
            keyboard_divisions: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[repr(i32)]
pub enum BASSMIDIInterpolation {
    None = -1,
    #[default]
    Linear = 0,
    Sinc8 = 1,
    Sinc16 = 2,
}

impl TryFrom<i32> for BASSMIDIInterpolation {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            -1 => Ok(BASSMIDIInterpolation::None),
            0 => Ok(BASSMIDIInterpolation::Linear),
            1 => Ok(BASSMIDIInterpolation::Sinc8),
            2 => Ok(BASSMIDIInterpolation::Sinc16),
            _ => Err(format!("Unsupported interpolation type: {}", value)),
        }
    }
}

impl From<BASSMIDIInterpolation> for i32 {
    fn from(value: BASSMIDIInterpolation) -> Self {
        value as i32
    }
}
