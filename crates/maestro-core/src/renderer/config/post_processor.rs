#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct PostProcessorConfig {
    pub volume: f32,
    pub limiter: Option<AudioLimiterConfig>,
    pub dc_blocker: bool,
    pub equalizer: Option<EqualizerConfig>,
    pub reverb: Option<ReverbConfig>,
}

impl Default for PostProcessorConfig {
    fn default() -> Self {
        Self {
            volume: 1.0,
            limiter: Some(Default::default()),
            dc_blocker: true,
            equalizer: None,
            reverb: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct AudioLimiterConfig {
    pub attack_ms: f32,
    pub release_ms: f32,
}

impl Default for AudioLimiterConfig {
    fn default() -> Self {
        Self {
            attack_ms: 20.0,
            release_ms: 400.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct EqualizerConfig {
    pub low_db: f32,
    pub mid_db: f32,
    pub high_db: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct ReverbConfig {
    pub mix: f32,
    pub room_size_m: f32,
    pub time_s: f32,
    pub damping: f32,
}

impl Default for ReverbConfig {
    fn default() -> Self {
        Self {
            mix: 0.15,
            room_size_m: 20.0,
            time_s: 2.0,
            damping: 0.5,
        }
    }
}
