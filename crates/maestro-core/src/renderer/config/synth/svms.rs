#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct SVMSConfig {
    pub voice_limit: u32,
    // 0 lets SVMS pick (up to 16 threads)
    pub render_threads: u32,
    pub render_backend: SVMSRenderBackend,
    pub master_volume: f32,

    pub limiter_enabled: bool,
    pub limiter_algorithm: SVMSLimiterAlgorithm,
    pub limiter_threshold: f32,
    pub limiter_lookahead_ms: f32,
    pub limiter_attack_ms: f32,
    pub limiter_release_ms: f32,
}

impl Default for SVMSConfig {
    fn default() -> Self {
        Self {
            voice_limit: 4096,
            render_threads: 1,
            render_backend: Default::default(),
            master_volume: 1.0,
            limiter_enabled: true,
            limiter_algorithm: Default::default(),
            limiter_threshold: 0.95,
            limiter_lookahead_ms: 3.0,
            limiter_attack_ms: 0.5,
            limiter_release_ms: 100.0,
        }
    }
}

impl SVMSConfig {
    pub const MAX_VOICES: u32 = 524288;
    pub const MAX_RENDER_THREADS: u32 = 64;

    // Ranges the SVMS runtime accepts; anything outside makes it refuse to
    // create the session.
    pub fn clamped(&self) -> Self {
        Self {
            voice_limit: self.voice_limit.clamp(1, Self::MAX_VOICES),
            render_threads: self.render_threads.min(Self::MAX_RENDER_THREADS),
            render_backend: self.render_backend,
            master_volume: finite_or(self.master_volume, 1.0).clamp(0.0, 4.0),
            limiter_enabled: self.limiter_enabled,
            limiter_algorithm: self.limiter_algorithm,
            limiter_threshold: finite_or(self.limiter_threshold, 0.95).clamp(0.1, 1.0),
            limiter_lookahead_ms: finite_or(self.limiter_lookahead_ms, 3.0).clamp(0.0, 20.0),
            limiter_attack_ms: finite_or(self.limiter_attack_ms, 0.5).clamp(0.01, 100.0),
            limiter_release_ms: finite_or(self.limiter_release_ms, 100.0).clamp(1.0, 5000.0),
        }
    }
}

fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() { v } else { fallback }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[repr(i32)]
pub enum SVMSRenderBackend {
    #[default]
    Auto = 0,
    Scalar = 1,
    SSE2 = 2,
    AVX2 = 3,
}

impl TryFrom<i32> for SVMSRenderBackend {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Auto),
            1 => Ok(Self::Scalar),
            2 => Ok(Self::SSE2),
            3 => Ok(Self::AVX2),
            _ => Err(format!("Unsupported SVMS render backend: {value}")),
        }
    }
}

impl From<SVMSRenderBackend> for i32 {
    fn from(value: SVMSRenderBackend) -> Self {
        value as i32
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[repr(i32)]
pub enum SVMSLimiterAlgorithm {
    #[default]
    Classic = 0,
    Adaptive = 1,
}

impl TryFrom<i32> for SVMSLimiterAlgorithm {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Classic),
            1 => Ok(Self::Adaptive),
            _ => Err(format!("Unsupported SVMS limiter algorithm: {value}")),
        }
    }
}

impl From<SVMSLimiterAlgorithm> for i32 {
    fn from(value: SVMSLimiterAlgorithm) -> Self {
        value as i32
    }
}
