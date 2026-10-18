use std::path::{Path, PathBuf};
use std::ptr::{self, NonNull};
use std::time::Duration;

use crate::app::{Callbacks, RawApp, c_string};
use crate::ffi::*;
use crate::{App, Error, ErrorKind, Maestro};

/// The output file's format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Format {
    #[default]
    Wav,
    Flac,
    Mp3,
    Ogg,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WavBitDepth {
    Int16,
    Int24,
    #[default]
    Float32,
}

/// How the file's tracks map onto synth ports. Either way, everything is
/// mixed into the one output file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PortMode {
    /// Each track plays on the port its MIDI Port meta event (or UMP group)
    /// assigns, for files with more than 16 channels; at most `max_ports`
    /// (1-16) of them.
    Multi { max_ports: Option<u8> },
    /// Every track plays on one port.
    Single,
}

impl Default for PortMode {
    fn default() -> Self {
        Self::Multi { max_ports: None }
    }
}

/// What to render, and how. SoundFonts, synth and effects settings come from
/// the user's settings for the [`App`].
pub struct FileRenderOptions {
    midi_path: PathBuf,
    output_path: PathBuf,
    format: Format,
    wav_bit_depth: WavBitDepth,
    bitrate_kbps: u32,
    sample_rate: u32,
    channels: u32,
    port_mode: PortMode,
}

impl FileRenderOptions {
    /// Renders `midi_path` to `output_path`, which is written only once the
    /// render completes, replacing any file already there.
    pub fn new(midi_path: impl Into<PathBuf>, output_path: impl Into<PathBuf>) -> Self {
        Self {
            midi_path: midi_path.into(),
            output_path: output_path.into(),
            format: Format::default(),
            wav_bit_depth: WavBitDepth::default(),
            bitrate_kbps: 0,
            sample_rate: 0,
            channels: 0,
            port_mode: PortMode::default(),
        }
    }

    pub fn format(mut self, format: Format) -> Self {
        self.format = format;
        self
    }

    pub fn wav_bit_depth(mut self, depth: WavBitDepth) -> Self {
        self.wav_bit_depth = depth;
        self
    }

    /// For MP3 and OGG: 64, 96, 128, 160, 192 (default), 224, 256 or 320.
    pub fn bitrate_kbps(mut self, kbps: u32) -> Self {
        self.bitrate_kbps = kbps;
        self
    }

    /// Defaults to the sample rate in the user's settings.
    pub fn sample_rate(mut self, hz: u32) -> Self {
        self.sample_rate = hz;
        self
    }

    /// 1 (mono) or 2 (stereo). Defaults to the channel count in the user's
    /// settings.
    pub fn channels(mut self, channels: u32) -> Self {
        self.channels = channels;
        self
    }

    pub fn port_mode(mut self, mode: PortMode) -> Self {
        self.port_mode = mode;
        self
    }
}

/// How far a render has come.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Progress {
    /// How much of the song has been rendered.
    pub position: Duration,
    /// The song's length, ending with its last event.
    pub length: Duration,
    /// Rendered seconds per wall-clock second.
    pub speed: f32,
    pub notes: u64,
    pub events: u64,
    pub voice_count: u64,
}

/// A job that renders one MIDI file to an audio file. Freed when dropped.
///
/// [`run`](Self::run) blocks; call [`cancel`](Self::cancel) and
/// [`progress`](Self::progress) from another thread meanwhile, e.g. with the
/// job in an `Arc` or a scoped thread.
pub struct FileRender {
    maestro: Maestro,
    api: &'static MaestroFileRenderV1,
    raw: NonNull<MaestroFileRender>,
    _callbacks: Box<Callbacks>,
}

// Everything but close is safe from any thread, and close needs the job by
// value, so it can't race run().
unsafe impl Send for FileRender {}
unsafe impl Sync for FileRender {}

fn path_string(path: &Path, what: &str) -> Result<std::ffi::CString, Error> {
    let path = path
        .to_str()
        .ok_or_else(|| Error::new(ErrorKind::InvalidArg, format!("{what} is not valid UTF-8")))?;
    c_string(path, what)
}

impl FileRender {
    pub(crate) fn open(
        maestro: Maestro,
        app: &App,
        options: FileRenderOptions,
    ) -> Result<Self, Error> {
        let api = Maestro::interface(maestro.api().file_render, MAESTRO_FILE_RENDER_V1)?;
        let app = RawApp::new(app, None)?;
        let midi_path = path_string(&options.midi_path, "midi_path")?;
        let output_path = path_string(&options.output_path, "output_path")?;
        let (port_mode, max_ports) = match options.port_mode {
            PortMode::Multi { max_ports } => (MAESTRO_PORTS_MULTI, max_ports.unwrap_or(0).into()),
            PortMode::Single => (MAESTRO_PORTS_SINGLE, 0),
        };
        let params = MaestroFileRenderParams {
            struct_size: size_of::<MaestroFileRenderParams>() as u32,
            reserved: 0,
            midi_path: midi_path.as_ptr(),
            output_path: output_path.as_ptr(),
            format: match options.format {
                Format::Wav => MAESTRO_FORMAT_WAV,
                Format::Flac => MAESTRO_FORMAT_FLAC,
                Format::Mp3 => MAESTRO_FORMAT_MP3,
                Format::Ogg => MAESTRO_FORMAT_OGG,
            },
            wav_bit_depth: match options.wav_bit_depth {
                WavBitDepth::Int16 => 16,
                WavBitDepth::Int24 => 24,
                WavBitDepth::Float32 => 32,
            },
            bitrate_kbps: options.bitrate_kbps,
            sample_rate: options.sample_rate,
            channels: options.channels,
            port_mode,
            max_ports,
        };

        let mut raw = ptr::null_mut();
        maestro.check(unsafe { (api.open)(&app.info, &params, &mut raw) })?;
        let raw = NonNull::new(raw)
            .ok_or_else(|| Error::new(ErrorKind::Internal, "open returned no job"))?;
        Ok(Self {
            maestro,
            api,
            raw,
            _callbacks: app.callbacks,
        })
    }

    /// Renders the whole file, blocking until it is done. Fails with
    /// [`ErrorKind::Cancelled`] if [`cancel`](Self::cancel) stopped it, and
    /// with [`ErrorKind::Busy`] if the job has already run. A cancelled or
    /// failed render leaves no output behind.
    pub fn run(&self) -> Result<(), Error> {
        self.maestro
            .check(unsafe { (self.api.run)(self.raw.as_ptr()) })
    }

    /// Asks a running job to stop; [`run`](Self::run) returns soon after.
    pub fn cancel(&self) {
        unsafe { (self.api.cancel)(self.raw.as_ptr()) }
    }

    pub fn progress(&self) -> Result<Progress, Error> {
        let mut out = MaestroFileRenderProgress {
            struct_size: size_of::<MaestroFileRenderProgress>() as u32,
            ..Default::default()
        };
        self.maestro
            .check(unsafe { (self.api.progress)(self.raw.as_ptr(), &mut out) })?;
        Ok(Progress {
            position: Duration::from_secs_f64(out.position_s.max(0.0)),
            length: Duration::from_secs_f64(out.length_s.max(0.0)),
            speed: out.speed,
            notes: out.notes,
            events: out.events,
            voice_count: out.voice_count,
        })
    }

    /// The raw handle, for calling interfaces these bindings don't wrap yet.
    /// It stays valid until this `FileRender` is dropped.
    pub fn as_raw(&self) -> *mut MaestroFileRender {
        self.raw.as_ptr()
    }
}

impl Drop for FileRender {
    fn drop(&mut self) {
        unsafe { (self.api.close)(self.raw.as_ptr()) };
    }
}
