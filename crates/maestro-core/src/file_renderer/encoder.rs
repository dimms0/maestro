use std::{fs::File, io::BufWriter, path::Path};

use hound::{SampleFormat, WavSpec, WavWriter};

use crate::{audio_params::AudioParameters, error::FileRendererError};

/// The container/codec the file renderer writes to disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
pub enum OutputFormat {
    #[default]
    Wav,
    Flac,
    Mp3,
    Ogg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
pub enum WavBitDepth {
    #[default]
    Float32,
    Int24,
    Int16,
}

pub const BITRATES_KBPS: [u32; 8] = [64, 96, 128, 160, 192, 224, 256, 320];
pub const DEFAULT_BITRATE_KBPS: u32 = 192;

impl OutputFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            OutputFormat::Wav => "wav",
            OutputFormat::Flac => "flac",
            OutputFormat::Mp3 => "mp3",
            OutputFormat::Ogg => "ogg",
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            OutputFormat::Wav => "wav",
            OutputFormat::Flac => "flac",
            OutputFormat::Mp3 => "mp3",
            OutputFormat::Ogg => "ogg",
        }
    }

    pub fn from_id(id: &str) -> Self {
        match id {
            "flac" => OutputFormat::Flac,
            "mp3" => OutputFormat::Mp3,
            "ogg" => OutputFormat::Ogg,
            _ => OutputFormat::Wav,
        }
    }

    pub fn has_bitrate(&self) -> bool {
        matches!(self, OutputFormat::Mp3 | OutputFormat::Ogg)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputSettings {
    pub format: OutputFormat,
    pub wav_bit_depth: WavBitDepth,
    pub bitrate_kbps: u32,
}

impl Default for OutputSettings {
    fn default() -> Self {
        Self {
            format: OutputFormat::default(),
            wav_bit_depth: WavBitDepth::default(),
            bitrate_kbps: DEFAULT_BITRATE_KBPS,
        }
    }
}

impl OutputSettings {
    pub fn extension(&self) -> &'static str {
        self.format.extension()
    }
}

#[cfg(feature = "encoders")]
fn lame_bitrate(kbps: u32) -> mp3lame_encoder::Bitrate {
    use mp3lame_encoder::Bitrate;

    const RATES: [(u32, Bitrate); 16] = [
        (8, Bitrate::Kbps8),
        (16, Bitrate::Kbps16),
        (24, Bitrate::Kbps24),
        (32, Bitrate::Kbps32),
        (40, Bitrate::Kbps40),
        (48, Bitrate::Kbps48),
        (64, Bitrate::Kbps64),
        (80, Bitrate::Kbps80),
        (96, Bitrate::Kbps96),
        (112, Bitrate::Kbps112),
        (128, Bitrate::Kbps128),
        (160, Bitrate::Kbps160),
        (192, Bitrate::Kbps192),
        (224, Bitrate::Kbps224),
        (256, Bitrate::Kbps256),
        (320, Bitrate::Kbps320),
    ];

    RATES
        .iter()
        .min_by_key(|(rate, _)| rate.abs_diff(kbps))
        .map(|(_, bitrate)| *bitrate)
        .unwrap_or(Bitrate::Kbps192)
}

#[inline]
fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

#[inline]
fn f32_to_i24(s: f32) -> i32 {
    (s.clamp(-1.0, 1.0) * 8_388_607.0) as i32
}

pub enum AudioEncoder {
    Wav {
        writer: WavWriter<BufWriter<File>>,
        bit_depth: WavBitDepth,
    },
    #[cfg(feature = "encoders")]
    Flac {
        samples: Vec<i32>,
        channels: usize,
        sample_rate: usize,
        path: std::path::PathBuf,
    },
    #[cfg(feature = "encoders")]
    Mp3 {
        encoder: mp3lame_encoder::Encoder,
        out: BufWriter<File>,
        scratch: Vec<u8>,
    },
    #[cfg(feature = "encoders")]
    Ogg {
        encoder: Box<vorbis_rs::VorbisEncoder<BufWriter<File>>>,
        channels: usize,
        planar: Vec<Vec<f32>>,
    },
}

impl AudioEncoder {
    pub fn new(
        path: &Path,
        audio_params: &AudioParameters,
        settings: OutputSettings,
    ) -> Result<Self, FileRendererError> {
        let channels: u16 = audio_params.channels.into();
        let sample_rate: u32 = audio_params.sample_rate;

        match settings.format {
            OutputFormat::Wav => {
                let bit_depth = settings.wav_bit_depth;
                let (bits, fmt) = match bit_depth {
                    WavBitDepth::Int16 => (16, SampleFormat::Int),
                    WavBitDepth::Int24 => (24, SampleFormat::Int),
                    WavBitDepth::Float32 => (32, SampleFormat::Float),
                };
                let spec = WavSpec {
                    channels,
                    sample_rate,
                    bits_per_sample: bits,
                    sample_format: fmt,
                };
                let writer = WavWriter::create(path, spec).map_err(FileRendererError::from)?;
                Ok(AudioEncoder::Wav { writer, bit_depth })
            }

            #[cfg(feature = "encoders")]
            OutputFormat::Flac => Ok(AudioEncoder::Flac {
                samples: Vec::new(),
                channels: channels as usize,
                sample_rate: sample_rate as usize,
                path: path.to_path_buf(),
            }),

            #[cfg(feature = "encoders")]
            OutputFormat::Mp3 => {
                use mp3lame_encoder::{Builder, Quality};
                let mut builder = Builder::new().ok_or_else(|| {
                    FileRendererError::Encoding("failed to create LAME builder".into())
                })?;
                builder
                    .set_num_channels(channels as u8)
                    .map_err(|e| FileRendererError::Encoding(format!("LAME channels: {e:?}")))?;
                builder
                    .set_sample_rate(sample_rate)
                    .map_err(|e| FileRendererError::Encoding(format!("LAME sample rate: {e:?}")))?;
                builder
                    .set_brate(lame_bitrate(settings.bitrate_kbps))
                    .map_err(|e| FileRendererError::Encoding(format!("LAME bitrate: {e:?}")))?;
                builder
                    .set_quality(Quality::Good)
                    .map_err(|e| FileRendererError::Encoding(format!("LAME quality: {e:?}")))?;
                let encoder = builder
                    .build()
                    .map_err(|e| FileRendererError::Encoding(format!("LAME build: {e:?}")))?;
                let out = BufWriter::new(File::create(path)?);
                Ok(AudioEncoder::Mp3 {
                    encoder,
                    out,
                    scratch: Vec::new(),
                })
            }

            #[cfg(feature = "encoders")]
            OutputFormat::Ogg => {
                use std::num::{NonZeroU8, NonZeroU32};
                use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoderBuilder};

                let sink = BufWriter::new(File::create(path)?);
                let sr = NonZeroU32::new(sample_rate)
                    .ok_or_else(|| FileRendererError::Encoding("invalid sample rate".into()))?;
                let ch = NonZeroU8::new(channels as u8)
                    .ok_or_else(|| FileRendererError::Encoding("invalid channel count".into()))?;
                let average_bitrate = NonZeroU32::new(settings.bitrate_kbps.max(1) * 1000)
                    .ok_or_else(|| FileRendererError::Encoding("invalid bitrate".into()))?;
                let encoder = VorbisEncoderBuilder::new(sr, ch, sink)
                    .map_err(|e| FileRendererError::Encoding(format!("Vorbis builder: {e}")))?
                    .bitrate_management_strategy(VorbisBitrateManagementStrategy::Abr {
                        average_bitrate,
                    })
                    .build()
                    .map_err(|e| FileRendererError::Encoding(format!("Vorbis build: {e}")))?;
                Ok(AudioEncoder::Ogg {
                    encoder: Box::new(encoder),
                    channels: channels as usize,
                    planar: vec![Vec::new(); channels as usize],
                })
            }

            #[cfg(not(feature = "encoders"))]
            OutputFormat::Flac => Err(FileRendererError::UnsupportedFormat("flac")),
            #[cfg(not(feature = "encoders"))]
            OutputFormat::Mp3 => Err(FileRendererError::UnsupportedFormat("mp3")),
            #[cfg(not(feature = "encoders"))]
            OutputFormat::Ogg => Err(FileRendererError::UnsupportedFormat("ogg")),
        }
    }

    pub fn write_samples(&mut self, interleaved: &[f32]) -> Result<(), FileRendererError> {
        match self {
            AudioEncoder::Wav { writer, bit_depth } => {
                match bit_depth {
                    WavBitDepth::Int16 => {
                        for &s in interleaved {
                            writer.write_sample(f32_to_i16(s))?;
                        }
                    }
                    WavBitDepth::Int24 => {
                        for &s in interleaved {
                            writer.write_sample(f32_to_i24(s))?;
                        }
                    }
                    WavBitDepth::Float32 => {
                        for &s in interleaved {
                            writer.write_sample(s)?;
                        }
                    }
                }
                Ok(())
            }

            #[cfg(feature = "encoders")]
            AudioEncoder::Flac { samples, .. } => {
                samples.extend(interleaved.iter().map(|&s| f32_to_i24(s)));
                Ok(())
            }

            #[cfg(feature = "encoders")]
            AudioEncoder::Mp3 {
                encoder,
                out,
                scratch,
            } => {
                use mp3lame_encoder::{InterleavedPcm, max_required_buffer_size};
                use std::io::Write;
                scratch.clear();
                // LAME writes into the Vec's spare capacity, so it must be
                // reserved up-front or the C encoder writes out of bounds.
                scratch.reserve(max_required_buffer_size(interleaved.len()));
                encoder
                    .encode_to_vec(InterleavedPcm(interleaved), scratch)
                    .map_err(|e| FileRendererError::Encoding(format!("MP3 encode: {e:?}")))?;
                out.write_all(scratch)?;
                Ok(())
            }

            #[cfg(feature = "encoders")]
            AudioEncoder::Ogg {
                encoder,
                channels,
                planar,
            } => {
                let ch = *channels;
                for plane in planar.iter_mut() {
                    plane.clear();
                }
                for frame in interleaved.chunks_exact(ch) {
                    for (c, &s) in frame.iter().enumerate() {
                        planar[c].push(s);
                    }
                }
                encoder
                    .encode_audio_block(&*planar)
                    .map_err(|e| FileRendererError::Encoding(format!("Vorbis encode: {e}")))?;
                Ok(())
            }
        }
    }

    pub fn finalize(self) -> Result<(), FileRendererError> {
        match self {
            AudioEncoder::Wav { writer, .. } => {
                writer.finalize()?;
                Ok(())
            }

            #[cfg(feature = "encoders")]
            AudioEncoder::Flac {
                samples,
                channels,
                sample_rate,
                path,
            } => {
                use flacenc::{
                    bitsink::ByteSink, component::BitRepr, error::Verify, source::MemSource,
                };
                use std::io::Write;

                let config = flacenc::config::Encoder::default()
                    .into_verified()
                    .map_err(|e| FileRendererError::Encoding(format!("FLAC config: {e:?}")))?;
                let source = MemSource::from_samples(&samples, channels, 24, sample_rate);
                let stream =
                    flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
                        .map_err(|e| FileRendererError::Encoding(format!("FLAC encode: {e:?}")))?;
                let mut sink = ByteSink::new();
                stream
                    .write(&mut sink)
                    .map_err(|e| FileRendererError::Encoding(format!("FLAC write: {e:?}")))?;
                let mut file = BufWriter::new(File::create(&path)?);
                file.write_all(sink.as_slice())?;
                file.flush()?;
                Ok(())
            }

            #[cfg(feature = "encoders")]
            AudioEncoder::Mp3 {
                mut encoder,
                mut out,
                mut scratch,
            } => {
                use mp3lame_encoder::FlushNoGap;
                use std::io::Write;
                scratch.clear();
                // The final flush needs at least ~7200 bytes of spare capacity.
                scratch.reserve(7200);
                encoder
                    .flush_to_vec::<FlushNoGap>(&mut scratch)
                    .map_err(|e| FileRendererError::Encoding(format!("MP3 flush: {e:?}")))?;
                out.write_all(&scratch)?;
                out.flush()?;
                Ok(())
            }

            #[cfg(feature = "encoders")]
            AudioEncoder::Ogg { encoder, .. } => {
                let mut sink = encoder
                    .finish()
                    .map_err(|e| FileRendererError::Encoding(format!("Vorbis finish: {e}")))?;
                use std::io::Write;
                sink.flush()?;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BITRATES_KBPS, DEFAULT_BITRATE_KBPS, OutputFormat, OutputSettings, WavBitDepth};

    const ALL: [OutputFormat; 4] = [
        OutputFormat::Wav,
        OutputFormat::Flac,
        OutputFormat::Mp3,
        OutputFormat::Ogg,
    ];

    #[test]
    fn id_from_id_roundtrip() {
        for fmt in ALL {
            assert_eq!(OutputFormat::from_id(fmt.id()), fmt);
        }
    }

    #[test]
    fn unknown_id_falls_back_to_default() {
        assert_eq!(OutputFormat::from_id("nonsense"), OutputFormat::default());
        assert_eq!(OutputFormat::default(), OutputFormat::Wav);
    }

    #[test]
    fn the_ids_wav_used_before_the_bit_depth_split_still_resolve() {
        for legacy in ["wav_f32", "wav_24", "wav_16"] {
            assert_eq!(OutputFormat::from_id(legacy), OutputFormat::Wav);
        }
    }

    #[test]
    fn ids_are_unique() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a.id(), b.id());
            }
        }
    }

    #[test]
    fn only_the_lossy_formats_carry_a_bitrate() {
        assert!(OutputFormat::Mp3.has_bitrate());
        assert!(OutputFormat::Ogg.has_bitrate());
        assert!(!OutputFormat::Wav.has_bitrate());
        assert!(!OutputFormat::Flac.has_bitrate());
    }

    #[test]
    fn defaults_are_lossless_wav_at_the_default_bitrate() {
        let s = OutputSettings::default();
        assert_eq!(s.format, OutputFormat::Wav);
        assert_eq!(s.wav_bit_depth, WavBitDepth::Float32);
        assert_eq!(s.bitrate_kbps, DEFAULT_BITRATE_KBPS);
        assert_eq!(s.extension(), "wav");
        assert!(BITRATES_KBPS.contains(&DEFAULT_BITRATE_KBPS));
    }

    #[cfg(feature = "encoders")]
    #[test]
    fn every_offered_bitrate_maps_to_itself_in_lame() {
        for kbps in BITRATES_KBPS {
            assert_eq!(super::lame_bitrate(kbps) as u32, kbps);
        }
    }

    #[cfg(feature = "encoders")]
    #[test]
    fn an_off_list_bitrate_lands_on_its_nearest_neighbour() {
        assert_eq!(super::lame_bitrate(0) as u32, 8);
        assert_eq!(super::lame_bitrate(130) as u32, 128);
        assert_eq!(super::lame_bitrate(100_000) as u32, 320);
    }
}
