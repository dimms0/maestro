use maestro_core::file_renderer::{
    DEFAULT_BITRATE_KBPS, OutputFormat, OutputSettings, WavBitDepth,
};
pub use maestro_core::system_cfg::ConfigComponent;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug)]
pub struct ComponentProfile {
    pub kind: ConfigComponent,
    pub has_enabled: bool,
    pub has_sflist: bool,
    pub has_audio_params: bool,
    pub has_realtime: bool,
    pub has_renderer: bool,
    pub has_event_processor: bool,
    pub has_post_processor: bool,
    pub has_port_threads: bool,
}

impl ComponentProfile {
    pub fn for_kind(kind: ConfigComponent) -> Self {
        let base = ComponentProfile {
            kind,
            has_enabled: true,
            has_sflist: true,
            has_audio_params: false,
            has_realtime: false,
            has_renderer: true,
            has_event_processor: true,
            has_post_processor: true,
            has_port_threads: true,
        };
        match kind {
            ConfigComponent::Converter => ComponentProfile {
                has_enabled: false,
                has_sflist: false,
                has_audio_params: true,
                ..base
            },
            ConfigComponent::System => ComponentProfile {
                has_enabled: false,
                has_audio_params: true,
                has_realtime: true,
                ..base
            },
            ConfigComponent::KDMAPI => ComponentProfile {
                has_audio_params: true,
                has_realtime: true,
                has_port_threads: false,
                ..base
            },
            // ComponentKind::Generic => ComponentProfile {
            //     has_audio_params: true,
            //     has_realtime: true,
            //     ..base
            // },
        }
    }

    pub fn for_name(name: &str) -> Option<Self> {
        ConfigComponent::from_name(name).map(Self::for_kind)
    }

    pub fn has_converter_options(&self) -> bool {
        self.kind == ConfigComponent::Converter
    }

    pub fn has_device_options(&self) -> bool {
        self.kind == ConfigComponent::System
    }
}

#[derive(Serialize, Debug, Clone)]
pub struct ConverterCustom {
    pub multithreaded_export: bool,
    pub export_threads: usize,
    pub output_format: OutputFormat,
    pub wav_bit_depth: WavBitDepth,
    pub bitrate_kbps: u32,
    pub output_path: String,
}

impl Default for ConverterCustom {
    fn default() -> Self {
        Self {
            multithreaded_export: false,
            export_threads: 0,
            output_format: OutputFormat::default(),
            wav_bit_depth: WavBitDepth::default(),
            bitrate_kbps: DEFAULT_BITRATE_KBPS,
            output_path: String::new(),
        }
    }
}

impl ConverterCustom {
    pub fn output_settings(&self) -> OutputSettings {
        OutputSettings {
            format: self.output_format,
            wav_bit_depth: self.wav_bit_depth,
            bitrate_kbps: self.bitrate_kbps,
        }
    }
}

#[derive(Deserialize, Default, Clone, Copy)]
enum StoredFormat {
    #[default]
    Wav,
    Flac,
    Mp3,
    Ogg,
    WavF32,
    Wav24,
    Wav16,
}

impl StoredFormat {
    fn split(self) -> (OutputFormat, Option<WavBitDepth>) {
        match self {
            Self::Wav => (OutputFormat::Wav, None),
            Self::Flac => (OutputFormat::Flac, None),
            Self::Mp3 => (OutputFormat::Mp3, None),
            Self::Ogg => (OutputFormat::Ogg, None),
            Self::WavF32 => (OutputFormat::Wav, Some(WavBitDepth::Float32)),
            Self::Wav24 => (OutputFormat::Wav, Some(WavBitDepth::Int24)),
            Self::Wav16 => (OutputFormat::Wav, Some(WavBitDepth::Int16)),
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct StoredConverterCustom {
    multithreaded_export: bool,
    export_threads: usize,
    output_format: StoredFormat,
    wav_bit_depth: Option<WavBitDepth>,
    bitrate_kbps: Option<u32>,
    output_path: String,
}

impl Default for StoredConverterCustom {
    fn default() -> Self {
        let d = ConverterCustom::default();
        Self {
            multithreaded_export: d.multithreaded_export,
            export_threads: d.export_threads,
            output_format: StoredFormat::default(),
            wav_bit_depth: None,
            bitrate_kbps: None,
            output_path: d.output_path,
        }
    }
}

impl<'de> Deserialize<'de> for ConverterCustom {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let stored = StoredConverterCustom::deserialize(deserializer)?;
        let (output_format, legacy_depth) = stored.output_format.split();
        let defaults = ConverterCustom::default();

        Ok(ConverterCustom {
            multithreaded_export: stored.multithreaded_export,
            export_threads: stored.export_threads,
            output_format,
            wav_bit_depth: stored
                .wav_bit_depth
                .or(legacy_depth)
                .unwrap_or(defaults.wav_bit_depth),
            bitrate_kbps: stored.bitrate_kbps.unwrap_or(defaults.bitrate_kbps),
            output_path: stored.output_path,
        })
    }
}

pub use maestro_core::system_cfg::system::SystemCustomSettings;

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> ConverterCustom {
        serde_json::from_str(json).expect("converter payload should parse")
    }

    #[test]
    fn an_empty_payload_falls_back_to_the_defaults() {
        let c = parse("{}");
        assert_eq!(c.output_format, OutputFormat::Wav);
        assert_eq!(c.wav_bit_depth, WavBitDepth::Float32);
        assert_eq!(c.bitrate_kbps, DEFAULT_BITRATE_KBPS);
        assert_eq!(c.export_threads, 0);
    }

    #[test]
    fn a_legacy_wav_format_keeps_the_bit_depth_it_named() {
        for (stored, expected) in [
            ("WavF32", WavBitDepth::Float32),
            ("Wav24", WavBitDepth::Int24),
            ("Wav16", WavBitDepth::Int16),
        ] {
            let c = parse(&format!(r#"{{"output_format":"{stored}"}}"#));
            assert_eq!(c.output_format, OutputFormat::Wav);
            assert_eq!(c.wav_bit_depth, expected, "migrating {stored}");
        }
    }

    #[test]
    fn an_explicit_bit_depth_wins_over_a_legacy_format_name() {
        let c = parse(r#"{"output_format":"Wav16","wav_bit_depth":"Int24"}"#);
        assert_eq!(c.wav_bit_depth, WavBitDepth::Int24);
    }

    #[test]
    fn the_other_formats_are_unchanged_by_the_migration() {
        for (stored, expected) in [
            ("Flac", OutputFormat::Flac),
            ("Mp3", OutputFormat::Mp3),
            ("Ogg", OutputFormat::Ogg),
        ] {
            assert_eq!(
                parse(&format!(r#"{{"output_format":"{stored}"}}"#)).output_format,
                expected
            );
        }
    }

    #[test]
    fn a_saved_payload_round_trips() {
        let c = ConverterCustom {
            multithreaded_export: true,
            export_threads: 6,
            output_format: OutputFormat::Mp3,
            wav_bit_depth: WavBitDepth::Int24,
            bitrate_kbps: 320,
            output_path: "/tmp/out".to_string(),
        };
        let back = parse(&serde_json::to_string(&c).unwrap());
        assert_eq!(back.multithreaded_export, c.multithreaded_export);
        assert_eq!(back.export_threads, c.export_threads);
        assert_eq!(back.output_format, c.output_format);
        assert_eq!(back.wav_bit_depth, c.wav_bit_depth);
        assert_eq!(back.bitrate_kbps, c.bitrate_kbps);
        assert_eq!(back.output_path, c.output_path);
    }
}
