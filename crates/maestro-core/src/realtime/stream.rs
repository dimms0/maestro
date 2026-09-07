use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{
    BufferSize, Device, DeviceId, ErrorKind, I24, SampleFormat, Stream, StreamConfig,
    SupportedBufferSize, SupportedStreamConfig, SupportedStreamConfigRange, U24,
};

use crate::audio_params::{AudioParameters, COMMON_SAMPLE_RATES, ChannelCount};
use crate::error::RealtimeEngineError;
use crate::realtime::config::RealtimeConfig;

#[cfg(any(feature = "jack", target_os = "linux"))]
const JACK_CLIENT_NAME: &str = "Maestro";

const PLAUSIBLE_SAMPLE_RATES: std::ops::RangeInclusive<u32> = 4000..=768000;
const PLAUSIBLE_BUFFER_SIZES: std::ops::RangeInclusive<u32> = 16..=16384;

type StreamErrFn = OnceLock<Box<dyn Fn(&str) + Send + Sync>>;
static STREAM_ERROR_HANDLER: StreamErrFn = OnceLock::new();

pub fn set_stream_error_handler(handler: impl Fn(&str) + Send + Sync + 'static) {
    let _ = STREAM_ERROR_HANDLER.set(Box::new(handler));
}

fn report_stream_error(msg: &str) {
    match STREAM_ERROR_HANDLER.get() {
        Some(handler) => handler(msg),
        None => eprintln!("{msg}"),
    }
}

pub(super) struct ResolvedOutput {
    device: Device,
    supported: SupportedStreamConfig,
    pub params: AudioParameters,
    pub device_name: String,
}

fn resolve_host(audio_host: Option<&str>) -> cpal::Host {
    let Some(name) = audio_host else {
        return cpal::default_host();
    };

    let host = cpal::HostId::from_str(name)
        .ok()
        .and_then(|id| cpal::host_from_id(id).ok());

    match host {
        Some(host) => host,
        None => {
            report_stream_error(&format!(
                "Audio host {name} is unavailable, using the default host instead"
            ));
            cpal::default_host()
        }
    }
}

#[cfg(any(feature = "jack", target_os = "linux"))]
fn jack_output_device() -> Option<Device> {
    let mut host = cpal::platform::JackHost::new().ok()?;
    host.set_connect_automatically(true);
    host.output_device_with_name(JACK_CLIENT_NAME)
        .map(Into::into)
}

#[cfg(not(any(feature = "jack", target_os = "linux")))]
fn jack_output_device() -> Option<Device> {
    None
}

fn open_device(cfg: &RealtimeConfig) -> Result<Device, RealtimeEngineError> {
    let host = resolve_host(cfg.audio_host.as_deref());

    if host.id().name().eq_ignore_ascii_case("jack")
        && let Some(device) = jack_output_device()
    {
        return Ok(device);
    }

    if let Some(id) = cfg.output_device.as_deref() {
        let device = DeviceId::from_str(id)
            .ok()
            .and_then(|id| host.device_by_id(&id));

        match device {
            Some(device) => return Ok(device),
            None => report_stream_error(&format!(
                "Output device {id} is unavailable, using the default device instead"
            )),
        }
    }

    host.default_output_device()
        .ok_or_else(|| RealtimeEngineError::Audio("No output device available".to_string()))
}

fn sample_format_rank(format: SampleFormat) -> u8 {
    match format {
        SampleFormat::F32 => 0,
        SampleFormat::F64 => 1,
        SampleFormat::I32 => 2,
        SampleFormat::I16 => 3,
        _ => 10,
    }
}

fn nearest_rate(range: &SupportedStreamConfigRange, wanted: u32) -> u32 {
    wanted.clamp(range.min_sample_rate(), range.max_sample_rate())
}

/// The device's rate is preferred over the configured one because resampling is
/// not an option here: a mismatch would detune the output. Channel counts above
/// the render count are accepted and handled by the output callback.
fn select_config(
    ranges: &[SupportedStreamConfigRange],
    requested: AudioParameters,
) -> Option<SupportedStreamConfig> {
    let channels = u16::from(requested.channels);

    // Exact channel match first, then the narrowest layout that can still carry
    // every rendered channel. A device offering only 8 channels is usable; one
    // offering only 1 when we render 2 is not.
    let mut candidates: Vec<&SupportedStreamConfigRange> =
        ranges.iter().filter(|r| r.channels() == channels).collect();

    if candidates.is_empty() {
        candidates = ranges.iter().filter(|r| r.channels() > channels).collect();
        candidates.sort_by_key(|r| r.channels());
    }

    // Prefer a range that covers the requested rate outright; among those, the
    // one whose sample format loses the least. Otherwise take whichever range
    // can get closest to the requested rate.
    let exact = candidates
        .iter()
        .filter(|r| {
            r.min_sample_rate() <= requested.sample_rate
                && requested.sample_rate <= r.max_sample_rate()
        })
        .min_by_key(|r| sample_format_rank(r.sample_format()));

    if let Some(range) = exact {
        return range.try_with_sample_rate(requested.sample_rate);
    }

    let closest = candidates.iter().min_by_key(|r| {
        let rate = nearest_rate(r, requested.sample_rate);
        (
            rate.abs_diff(requested.sample_rate),
            sample_format_rank(r.sample_format()),
        )
    })?;

    let rate = nearest_rate(closest, requested.sample_rate);
    closest.try_with_sample_rate(rate)
}

fn resolve_buffer_size(requested: Option<u32>, supported: &SupportedBufferSize) -> BufferSize {
    let Some(frames) = requested.filter(|f| *f > 0) else {
        return BufferSize::Default;
    };

    match supported {
        SupportedBufferSize::Range { min, max } => BufferSize::Fixed(frames.clamp(*min, *max)),
        // The device declines to say; pass the request through and let the
        // build fall back if it is rejected.
        _ => BufferSize::Fixed(frames),
    }
}

pub(super) fn resolve_output(
    requested: Option<AudioParameters>,
    cfg: &RealtimeConfig,
) -> Result<ResolvedOutput, RealtimeEngineError> {
    let device = open_device(cfg)?;
    let device_name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let requested = match requested {
        Some(params) => params,
        None => {
            let default = device
                .default_output_config()
                .map_err(RealtimeEngineError::DefaultStreamConfig)?;

            AudioParameters {
                channels: ChannelCount::try_from(default.channels())
                    .unwrap_or(ChannelCount::Stereo),
                sample_rate: default.sample_rate(),
            }
        }
    };

    let ranges = device
        .supported_output_configs()
        .map_err(|e| RealtimeEngineError::Audio(format!("Error while querying configs: {e:?}")))?
        .collect::<Vec<_>>();

    let supported = match select_config(&ranges, requested) {
        Some(config) => config,
        None => device
            .default_output_config()
            .map_err(RealtimeEngineError::DefaultStreamConfig)?,
    };

    if supported.sample_rate() != requested.sample_rate {
        report_stream_error(&format!(
            "{device_name} does not support {} Hz, using {} Hz instead",
            requested.sample_rate,
            supported.sample_rate()
        ));
    }

    let params = AudioParameters {
        channels: ChannelCount::try_from(supported.channels()).unwrap_or(requested.channels),
        sample_rate: supported.sample_rate(),
    };

    Ok(ResolvedOutput {
        device,
        supported,
        params,
        device_name,
    })
}

// `lost` is raised from the audio thread when the
// device goes away, so the host can rebuild the engine
pub(super) fn build_stream<F>(
    out: &ResolvedOutput,
    cfg: &RealtimeConfig,
    func: F,
) -> Result<Stream, RealtimeEngineError>
where
    F: 'static + FnMut(&mut [f32]) + Send,
{
    let mut config: StreamConfig = out.supported.into();
    config.buffer_size = resolve_buffer_size(cfg.buffer_size, out.supported.buffer_size());

    let render_channels = u16::from(out.params.channels) as usize;
    let device_channels = out.supported.channels() as usize;

    let func = Arc::new(Mutex::new(func));
    let trampoline = || {
        let func = func.clone();
        move |buffer: &mut [f32]| (func.lock().unwrap())(buffer)
    };

    let err = match build_with_config(
        &out.device,
        config,
        out.supported.sample_format(),
        device_channels,
        render_channels,
        trampoline(),
    ) {
        Ok(stream) => return Ok(stream),
        Err(err) => err,
    };

    // A device can advertise a buffer range and still refuse a size inside it.
    // Losing the latency setting beats producing no audio at all.
    if matches!(config.buffer_size, BufferSize::Default) {
        return Err(err);
    }

    report_stream_error(&format!(
        "{} rejected a buffer of {:?} frames ({err}), using the device default instead",
        out.device_name, cfg.buffer_size
    ));

    config.buffer_size = BufferSize::Default;
    build_with_config(
        &out.device,
        config,
        out.supported.sample_format(),
        device_channels,
        render_channels,
        trampoline(),
    )
}

#[allow(clippy::too_many_arguments)]
fn build_with_config<F>(
    device: &Device,
    config: StreamConfig,
    sample_format: SampleFormat,
    device_channels: usize,
    render_channels: usize,
    func: F,
) -> Result<Stream, RealtimeEngineError>
where
    F: 'static + FnMut(&mut [f32]) + Send,
{
    macro_rules! build {
        ($t:ty) => {
            build_output_stream_for::<$t, _>(device, config, device_channels, render_channels, func)
        };
    }

    match sample_format {
        SampleFormat::I8 => build!(i8),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I24 => build!(I24),
        SampleFormat::I32 => build!(i32),
        SampleFormat::I64 => build!(i64),

        SampleFormat::U8 => build!(u8),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U24 => build!(U24),
        SampleFormat::U32 => build!(u32),
        SampleFormat::U64 => build!(u64),
        SampleFormat::F32 => build!(f32),
        SampleFormat::F64 => build!(f64),

        _ => Err(RealtimeEngineError::Audio(format!(
            "Unsupported sample format : {}",
            sample_format
        ))),
    }
}

fn write_frames<T>(data: &mut [T], rendered: &[f32], device_channels: usize, render_channels: usize)
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    if device_channels == render_channels {
        for (out, &sample) in data.iter_mut().zip(rendered.iter()) {
            *out = T::from_sample(sample);
        }
        return;
    }

    for (frame, out) in data.chunks_mut(device_channels).enumerate() {
        let base = frame * render_channels;

        for (channel, slot) in out.iter_mut().enumerate() {
            let sample = if channel < render_channels {
                rendered.get(base + channel).copied().unwrap_or(0.0)
            } else if render_channels == 1 {
                rendered.get(base).copied().unwrap_or(0.0)
            } else {
                0.0
            };

            *slot = T::from_sample(sample);
        }
    }
}

fn build_output_stream_for<T, F>(
    device: &Device,
    config: StreamConfig,
    device_channels: usize,
    render_channels: usize,
    mut func: F,
) -> Result<Stream, RealtimeEngineError>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
    F: 'static + FnMut(&mut [f32]) + Send,
{
    let mut render_buffer = Vec::new();

    Ok(device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            let frames = data.len() / device_channels.max(1);
            render_buffer.resize(frames * render_channels, 0.0);
            func(&mut render_buffer);

            write_frames(data, &render_buffer, device_channels, render_channels);
        },
        move |err| match err.kind() {
            ErrorKind::DeviceNotAvailable | ErrorKind::StreamInvalidated => {
                // TODO: auto fix
                report_stream_error(
                    "The audio device has been removed. Please restart the engine.",
                );
            }
            ErrorKind::Xrun => {}
            other => report_stream_error(&format!("Audio stream error: {other}")),
        },
        None,
    )?)
}

pub fn output_hosts() -> Vec<(String, String)> {
    cpal::available_hosts()
        .into_iter()
        .map(|id| (id.to_string(), id.name().to_string()))
        .collect()
}

/// Deliberately cheap: it never queries supported configurations, because doing
/// so opens the device, and on ALSA a failed open can leak file descriptors and
/// poison the backend for the rest of the process's life. Use [`device_caps`]
/// for the one device the user actually selected.
pub fn output_devices(host: Option<&str>) -> Vec<(String, String)> {
    let host = resolve_host(host);

    // JACK has no devices to choose between
    if host.id().name().eq_ignore_ascii_case("jack") {
        return Vec::new();
    }

    let Ok(devices) = host.output_devices() else {
        return Vec::new();
    };

    devices
        .filter_map(|device| {
            let id = device.id().ok()?.to_string();
            let name = device
                .description()
                .map(|d| d.name().to_string())
                .unwrap_or_else(|_| id.clone());

            Some((id, name))
        })
        .collect()
}

pub type DeviceCaps = (Vec<u32>, Option<(u32, u32)>);

pub fn device_caps(host: Option<&str>, device: Option<&str>) -> Option<DeviceCaps> {
    let cfg = RealtimeConfig {
        audio_host: host.map(str::to_string),
        output_device: device.map(str::to_string),
        ..Default::default()
    };

    let device = open_device(&cfg).ok()?;
    let ranges = device.supported_output_configs().ok()?.collect::<Vec<_>>();

    let mut rates: Vec<u32> = COMMON_SAMPLE_RATES
        .iter()
        .copied()
        .filter(|rate| {
            ranges
                .iter()
                .any(|r| r.min_sample_rate() <= *rate && *rate <= r.max_sample_rate())
        })
        .collect();

    for range in &ranges {
        for rate in [range.min_sample_rate(), range.max_sample_rate()] {
            if PLAUSIBLE_SAMPLE_RATES.contains(&rate) && !rates.contains(&rate) {
                rates.push(rate);
            }
        }
    }

    rates.sort_unstable();
    rates.dedup();

    // Narrowed to sizes a person would actually pick. ALSA advertises 1 frame
    // to 4 M frames, which is true but useless as a hint, and a single-frame
    // period is not something to offer.
    let buffer_range = ranges
        .iter()
        .find_map(|r| match r.buffer_size() {
            SupportedBufferSize::Range { min, max } => Some((*min, *max)),
            _ => None,
        })
        .map(|(min, max)| {
            (
                min.max(*PLAUSIBLE_BUFFER_SIZES.start()),
                max.min(*PLAUSIBLE_BUFFER_SIZES.end()),
            )
        })
        .filter(|(min, max)| min <= max);

    Some((rates, buffer_range))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(
        channels: u16,
        min: u32,
        max: u32,
        format: SampleFormat,
    ) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(
            channels,
            min,
            max,
            SupportedBufferSize::Range { min: 64, max: 4096 },
            format,
        )
    }

    fn stereo(rate: u32) -> AudioParameters {
        AudioParameters {
            channels: ChannelCount::Stereo,
            sample_rate: rate,
        }
    }

    #[test]
    fn picks_the_requested_rate_when_supported() {
        let ranges = [range(2, 44100, 96000, SampleFormat::F32)];
        let config = select_config(&ranges, stereo(48000)).unwrap();

        assert_eq!(config.sample_rate(), 48000);
        assert_eq!(config.channels(), 2);
    }

    #[test]
    fn prefers_f32_among_configs_covering_the_rate() {
        let ranges = [
            range(2, 48000, 48000, SampleFormat::I16),
            range(2, 48000, 48000, SampleFormat::F32),
        ];

        let config = select_config(&ranges, stereo(48000)).unwrap();
        assert_eq!(config.sample_format(), SampleFormat::F32);
    }

    /// The reported bug: a device that only offers 44100 used to be rejected
    /// outright with "No supported config found".
    #[test]
    fn falls_back_to_the_nearest_rate() {
        let ranges = [range(2, 44100, 44100, SampleFormat::F32)];
        let config = select_config(&ranges, stereo(48000)).unwrap();

        assert_eq!(config.sample_rate(), 44100);
    }

    #[test]
    fn falls_back_upwards_when_the_device_is_higher_rate_only() {
        let ranges = [range(2, 96000, 192000, SampleFormat::F32)];
        let config = select_config(&ranges, stereo(48000)).unwrap();

        assert_eq!(config.sample_rate(), 96000);
    }

    #[test]
    fn picks_the_closest_of_several_rates() {
        let ranges = [
            range(2, 8000, 8000, SampleFormat::F32),
            range(2, 44100, 44100, SampleFormat::F32),
            range(2, 192000, 192000, SampleFormat::F32),
        ];

        let config = select_config(&ranges, stereo(48000)).unwrap();
        assert_eq!(config.sample_rate(), 44100);
    }

    #[test]
    fn accepts_a_wider_channel_layout() {
        let ranges = [
            range(8, 48000, 48000, SampleFormat::F32),
            range(6, 48000, 48000, SampleFormat::F32),
        ];

        let config = select_config(&ranges, stereo(48000)).unwrap();
        assert_eq!(config.channels(), 6, "should take the narrowest that fits");
    }

    #[test]
    fn rejects_a_device_that_cannot_carry_every_channel() {
        let ranges = [range(1, 48000, 48000, SampleFormat::F32)];
        assert!(select_config(&ranges, stereo(48000)).is_none());
    }

    #[test]
    fn returns_none_without_any_ranges() {
        assert!(select_config(&[], stereo(48000)).is_none());
    }

    #[test]
    fn clamps_the_buffer_size_to_what_the_device_offers() {
        let supported = SupportedBufferSize::Range { min: 64, max: 512 };

        assert!(matches!(
            resolve_buffer_size(Some(32), &supported),
            BufferSize::Fixed(64)
        ));
        assert!(matches!(
            resolve_buffer_size(Some(2048), &supported),
            BufferSize::Fixed(512)
        ));
        assert!(matches!(
            resolve_buffer_size(Some(256), &supported),
            BufferSize::Fixed(256)
        ));
    }

    #[test]
    fn unset_or_zero_buffer_size_uses_the_device_default() {
        let supported = SupportedBufferSize::Range { min: 64, max: 512 };

        assert!(matches!(
            resolve_buffer_size(None, &supported),
            BufferSize::Default
        ));
        assert!(matches!(
            resolve_buffer_size(Some(0), &supported),
            BufferSize::Default
        ));
    }

    #[test]
    fn copies_straight_through_when_layouts_match() {
        let mut data = [0.0f32; 4];
        write_frames(&mut data, &[0.1, 0.2, 0.3, 0.4], 2, 2);

        assert_eq!(data, [0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn silences_channels_beyond_the_rendered_layout() {
        // Two stereo frames rendered into an 8-channel device buffer.
        let mut data = [1.0f32; 16];
        write_frames(&mut data, &[0.1, 0.2, 0.3, 0.4], 8, 2);

        assert_eq!(data[0..2], [0.1, 0.2]);
        assert!(data[2..8].iter().all(|s| *s == 0.0));
        assert_eq!(data[8..10], [0.3, 0.4]);
        assert!(data[10..16].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn spreads_mono_across_every_device_channel() {
        let mut data = [0.0f32; 4];
        write_frames(&mut data, &[0.5, 0.25], 2, 1);

        assert_eq!(data, [0.5, 0.5, 0.25, 0.25]);
    }
}
