use crate::error::AudioConfigurationError;

pub const DEFAULT_SAMPLE_RATE: u32 = 48000;
pub const COMMON_SAMPLE_RATES: [u32; 12] = [
    8000, 11025, 16000, 22050, 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000,
];

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct AudioParameters {
    pub channels: ChannelCount,
    pub sample_rate: u32,
}

impl Default for AudioParameters {
    fn default() -> Self {
        Self {
            channels: ChannelCount::default(),
            sample_rate: DEFAULT_SAMPLE_RATE,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[repr(u16)]
pub enum ChannelCount {
    Mono = 1,
    #[default]
    Stereo = 2,
}

impl TryFrom<u16> for ChannelCount {
    type Error = AudioConfigurationError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(ChannelCount::Mono),
            2 => Ok(ChannelCount::Stereo),
            _ => Err(AudioConfigurationError::ChannelCount(value)),
        }
    }
}

impl From<ChannelCount> for u16 {
    fn from(count: ChannelCount) -> Self {
        count as u16
    }
}
