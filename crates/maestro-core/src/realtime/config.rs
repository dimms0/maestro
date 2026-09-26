#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct RealtimeConfig {
    /// Determines the render thread's block size in milliseconds. This is
    /// different from the device buffer size.
    pub render_buffer_ms: f32,

    /// Use the high precision realtine clock for timestamping events.
    /// The timestamps are determined by the exact time the event is
    /// received by the engine.
    pub precision_playback: bool,

    /// Limit on notes per second. `None` lets every note through.
    pub max_nps: Option<NpsLimit>,

    /// Collapse bursts of CC/pitch-bend/program-change/aftertouch on the same
    /// channel+parameter down to the latest value, flushed to the renderer
    /// at most once per this many milliseconds. `None` disables coalescing.
    pub coalesce_window_ms: Option<u32>,

    /// Audio host (backend) to open the output on, as a `cpal::HostId` string
    /// such as `"ALSA"`, `"JACK"`, `"WASAPI"` or `"ASIO"`. `None` uses the
    /// platform default host. An unavailable host falls back to the default.
    pub audio_host: Option<String>,

    /// Output device to open, as a `cpal::DeviceId` string. `None` follows the
    /// system default output device. A device that is no longer present falls
    /// back to the default.
    pub output_device: Option<String>,

    /// Use the audio device's default parameters (channels, sample rate)
    pub device_audio_params: bool,

    /// Device buffer size in frames, which is what actually determines output
    /// latency. `None` leaves the host at its default period.
    pub buffer_size: Option<u32>,
}

impl Default for RealtimeConfig {
    fn default() -> Self {
        Self {
            device_audio_params: true,
            render_buffer_ms: 20.0,
            precision_playback: true,
            max_nps: Some(NpsLimit::default()),
            coalesce_window_ms: Some(10),
            audio_host: None,
            output_device: None,
            buffer_size: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct NpsLimit {
    pub max: usize,

    /// Lower the limit while the renderer can't keep up with realtime, scaled
    /// by how far over budget it is, and raise it back up to `max` as the load
    /// drops.
    pub load_limiter: bool,
}

impl Default for NpsLimit {
    fn default() -> Self {
        Self {
            max: 400_000,
            load_limiter: true,
        }
    }
}
