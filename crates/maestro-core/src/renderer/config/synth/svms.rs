#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct SVMSConfig {
    pub voice_limit: u32,
    // 0 lets SVMS pick (up to 16 threads)
    pub render_threads: u32,
    pub render_backend: SVMSRenderBackend,
    // 1 = single engine core; N > 1 splits notes by channel+key across N cores
    pub shards: u32,
}

impl Default for SVMSConfig {
    fn default() -> Self {
        Self {
            voice_limit: 4096,
            render_threads: 1,
            render_backend: Default::default(),
            shards: 1,
        }
    }
}

impl SVMSConfig {
    pub const MAX_VOICES: u32 = 524288;
    pub const MAX_RENDER_THREADS: u32 = 64;
    pub const MAX_SHARDS: u32 = 64;

    pub fn clamped(&self) -> Self {
        Self {
            voice_limit: self.voice_limit.clamp(1, Self::MAX_VOICES),
            render_threads: self.render_threads.min(Self::MAX_RENDER_THREADS),
            render_backend: self.render_backend,
            shards: self.shards.clamp(1, Self::MAX_SHARDS),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[repr(i32)]
pub enum SVMSRenderBackend {
    #[default]
    Auto = 0,
    Scalar = 1,
    SSE2 = 2,
    AVX2 = 3,
    // Not an SVMS backend value: CPU stays Auto and the GPU render stage is requested
    GPU = 4,
}

impl TryFrom<i32> for SVMSRenderBackend {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Auto),
            1 => Ok(Self::Scalar),
            2 => Ok(Self::SSE2),
            3 => Ok(Self::AVX2),
            4 => Ok(Self::GPU),
            _ => Err(format!("Unsupported SVMS render backend: {value}")),
        }
    }
}

impl From<SVMSRenderBackend> for i32 {
    fn from(value: SVMSRenderBackend) -> Self {
        value as i32
    }
}
