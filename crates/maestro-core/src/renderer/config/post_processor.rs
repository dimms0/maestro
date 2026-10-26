#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct PostProcessorConfig {
    pub volume: f32,
    pub limiter: Option<AudioLimiterConfig>,
    pub dc_blocker: bool,
}

impl Default for PostProcessorConfig {
    fn default() -> Self {
        Self {
            volume: 1.0,
            limiter: Some(Default::default()),
            dc_blocker: true,
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
