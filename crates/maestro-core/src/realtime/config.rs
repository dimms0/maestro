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

    pub max_nps: Option<usize>,

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
            max_nps: Some(100_000),
            audio_host: None,
            output_device: None,
            buffer_size: None,
        }
    }
}
