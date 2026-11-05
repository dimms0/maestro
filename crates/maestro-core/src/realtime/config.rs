#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct RealtimeConfig {
    /// Determines the render thread's block size in milliseconds. This is
    /// different from the device buffer size.
    pub render_buffer_ms: f32,

    /// Stamp events sent without a timestamp with their arrival time, instead
    /// of playing them at the start of the next render block.
    pub auto_timestamp: bool,

    /// Drop notes while the renderer is behind realtime.
    pub skip_behind: bool,

    /// Limit on notes per second. `None` lets every note through.
    pub nps_limit: Option<usize>,

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

    /// Run the render threads at realtime priority.
    pub realtime_priority: bool,
}

impl Default for RealtimeConfig {
    fn default() -> Self {
        Self {
            device_audio_params: true,
            render_buffer_ms: 20.0,
            auto_timestamp: true,
            skip_behind: true,
            nps_limit: Some(400_000),
            audio_host: None,
            output_device: None,
            buffer_size: None,
            realtime_priority: false,
        }
    }
}
