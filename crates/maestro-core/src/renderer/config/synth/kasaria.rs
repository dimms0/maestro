#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct KasariaConfig {
    pub voice_limit: u32,
    pub fast_decay: bool,
    pub antialiasing: bool,
    pub pre_resample: bool,
}

impl Default for KasariaConfig {
    fn default() -> Self {
        Self {
            voice_limit: 1024,
            fast_decay: true,
            antialiasing: true,
            pre_resample: true,
        }
    }
}
