use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use maestro_core::{
    audio_params::ChannelCount,
    error::{FileRendererError, MaestroComponentError},
    file_renderer::{
        BITRATES_KBPS, DEFAULT_BITRATE_KBPS, MaestroFileRenderer, MaestroFileRendererStatistics,
        OutputFormat, OutputSettings, PortMode, RenderOutcome, WavBitDepth,
    },
    system_cfg::MaestroConfigManager,
};
use midi_parser::MidiFile;

use crate::abi::*;
use crate::app::App;
use crate::error::{Failure, err_guard};
use crate::sized;

pub static FILE_RENDER_V1: MaestroFileRenderV1 = MaestroFileRenderV1 {
    open,
    close,
    run,
    cancel,
    progress,
};

struct Job {
    renderer: Mutex<Option<MaestroFileRenderer>>,
    output: PathBuf,
    partial: PathBuf,
    cancel: Arc<AtomicBool>,
    stats: MaestroFileRendererStatistics,
    length: f64,
    // When run() started, and when it returned
    times: Mutex<(Option<Instant>, Option<Instant>)>,
}

impl Job {
    unsafe fn open(
        app: *const MaestroAppInfo,
        params: *const MaestroFileRenderParams,
    ) -> Result<Self, Failure> {
        let app = unsafe { App::read(app) }?;
        let params = unsafe { sized::read(params) };
        sized::reserved(params.reserved)?;
        let midi_path = unsafe { sized::string(params.midi_path) }
            .ok_or_else(|| Failure::invalid("midi_path is required"))?;
        let output = PathBuf::from(
            unsafe { sized::string(params.output_path) }
                .ok_or_else(|| Failure::invalid("output_path is required"))?,
        );

        let format = match params.format {
            MAESTRO_FORMAT_WAV => OutputFormat::Wav,
            MAESTRO_FORMAT_FLAC => OutputFormat::Flac,
            MAESTRO_FORMAT_MP3 => OutputFormat::Mp3,
            MAESTRO_FORMAT_OGG => OutputFormat::Ogg,
            n => return Err(Failure::unsupported(format!("format {n} is not known"))),
        };
        let wav_bit_depth = match params.wav_bit_depth {
            0 | 32 => WavBitDepth::Float32,
            24 => WavBitDepth::Int24,
            16 => WavBitDepth::Int16,
            n => {
                return Err(Failure::invalid(format!(
                    "wav_bit_depth must be 16, 24 or 32, not {n}"
                )));
            }
        };
        let bitrate_kbps = match params.bitrate_kbps {
            0 => DEFAULT_BITRATE_KBPS,
            n if BITRATES_KBPS.contains(&n) => n,
            n => {
                return Err(Failure::invalid(format!(
                    "bitrate_kbps must be one of {BITRATES_KBPS:?}, not {n}"
                )));
            }
        };
        let port_mode = match (params.port_mode, params.max_ports) {
            (MAESTRO_PORTS_MULTI, 0) => PortMode::Multi { max_ports: None },
            (MAESTRO_PORTS_MULTI, n @ 1..=16) => PortMode::Multi {
                max_ports: Some(n as u8),
            },
            (MAESTRO_PORTS_MULTI, n) => {
                return Err(Failure::invalid(format!("max_ports must be 1-16, not {n}")));
            }
            (MAESTRO_PORTS_SINGLE, _) => PortMode::Single,
            (n, _) => return Err(Failure::unsupported(format!("port_mode {n} is not known"))),
        };

        let length = MidiFile::open(&midi_path)
            .and_then(|midi| midi.scan())
            .map_err(|e| {
                let code = match e {
                    midi_parser::Error::Io(_) => MAESTRO_ERR_IO,
                    _ => MAESTRO_ERR_BAD_MIDI,
                };
                Failure::new(code, format!("{midi_path}: {e}"))
            })?
            .duration;

        let manager = MaestroConfigManager::default();
        let config = app.profile(&manager)?;
        let mut audio_params = config.audio_params;
        if params.sample_rate != 0 {
            audio_params.sample_rate = params.sample_rate;
        }
        audio_params.channels = match params.channels {
            0 => audio_params.channels,
            1 => ChannelCount::Mono,
            2 => ChannelCount::Stereo,
            n => {
                return Err(Failure::invalid(format!(
                    "channels must be 1 or 2, not {n}"
                )));
            }
        };

        let soundfonts = manager
            .get_soundfont_list(&config.sflist)
            .unwrap_or_else(|e| {
                app.callbacks.log(
                    MAESTRO_LOG_ERROR,
                    &format!("The SoundFonts could not be loaded: {e}"),
                );
                Default::default()
            });

        let mut partial = OsString::from(&output);
        partial.push(".part");
        let partial = PathBuf::from(partial);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut renderer =
            MaestroFileRenderer::new(config.renderer, audio_params, soundfonts, &midi_path, ".")
                .map_err(|e| Failure::new(MAESTRO_ERR_BAD_MIDI, e))?
                .with_output_file(&partial)
                .with_output_settings(OutputSettings {
                    format,
                    wav_bit_depth,
                    bitrate_kbps,
                })
                .with_port_mode(port_mode)
                .with_cancel_flag(cancel.clone());
        if let Some(evproc) = config.event_processor {
            renderer = renderer.with_event_processor(evproc);
        }
        if let Some(postproc) = config.post_processor {
            renderer = renderer.with_post_processor(postproc);
        }

        Ok(Self {
            stats: renderer.get_statistics(),
            renderer: Mutex::new(Some(renderer)),
            output,
            partial,
            cancel,
            length,
            times: Mutex::default(),
        })
    }

    fn run(&self) -> Result<(), Failure> {
        let renderer = self
            .renderer
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| Failure::new(MAESTRO_ERR_BUSY, "This job has already run"))?;
        if self.cancel.load(Ordering::Relaxed) {
            return Err(cancelled());
        }

        self.times.lock().unwrap().0 = Some(Instant::now());
        let outcome = renderer.render();
        self.times.lock().unwrap().1 = Some(Instant::now());

        let result = match outcome {
            Ok(RenderOutcome::Completed) => {
                std::fs::rename(&self.partial, &self.output).map_err(|e| {
                    Failure::new(MAESTRO_ERR_IO, format!("{}: {e}", self.output.display()))
                })
            }
            Ok(RenderOutcome::Cancelled) => Err(cancelled()),
            Err(e) => Err(failure(e)),
        };
        if result.is_err() {
            let _ = std::fs::remove_file(&self.partial);
        }
        result
    }

    fn progress(&self) -> (f64, f32) {
        let position = self.stats.get_time();
        let (start, end) = *self.times.lock().unwrap();
        let elapsed = start.map_or(0.0, |start| {
            end.unwrap_or_else(Instant::now)
                .duration_since(start)
                .as_secs_f64()
        });
        let speed = if elapsed > 0.0 {
            position / elapsed
        } else {
            0.0
        };
        (position, speed as f32)
    }
}

fn cancelled() -> Failure {
    Failure::new(MAESTRO_ERR_CANCELLED, "The render was cancelled")
}

fn failure(e: MaestroComponentError) -> Failure {
    let code = match &e {
        MaestroComponentError::FileRenderer(FileRendererError::FileSystem(_)) => MAESTRO_ERR_IO,
        MaestroComponentError::FileRenderer(FileRendererError::InvalidFile(_)) => {
            MAESTRO_ERR_BAD_MIDI
        }
        MaestroComponentError::FileRenderer(FileRendererError::UnsupportedFormat(_)) => {
            MAESTRO_ERR_UNSUPPORTED
        }
        MaestroComponentError::FileRenderer(_) => MAESTRO_ERR_ENCODER,
        _ => MAESTRO_ERR_UNAVAILABLE,
    };
    Failure::new(code, e)
}

unsafe fn job<'a>(job: *mut MaestroFileRender) -> Result<&'a Job, Failure> {
    unsafe { job.cast::<Job>().as_ref() }.ok_or_else(|| Failure::invalid("job is NULL"))
}

unsafe extern "C" fn open(
    app: *const MaestroAppInfo,
    params: *const MaestroFileRenderParams,
    out: *mut *mut MaestroFileRender,
) -> MaestroResult {
    err_guard(|| {
        if out.is_null() {
            return Err(Failure::invalid("out is NULL"));
        }
        let job = unsafe { Job::open(app, params) }?;
        unsafe { *out = Box::into_raw(Box::new(job)).cast() };
        Ok(())
    })
}

unsafe extern "C" fn close(job: *mut MaestroFileRender) {
    if !job.is_null() {
        let _ = err_guard(|| {
            drop(unsafe { Box::from_raw(job.cast::<Job>()) });
            Ok(())
        });
    }
}

unsafe extern "C" fn run(job_ptr: *mut MaestroFileRender) -> MaestroResult {
    err_guard(|| unsafe { job(job_ptr) }?.run())
}

unsafe extern "C" fn cancel(job_ptr: *mut MaestroFileRender) {
    if let Ok(job) = unsafe { job(job_ptr) } {
        job.cancel.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn progress(
    job_ptr: *mut MaestroFileRender,
    out: *mut MaestroFileRenderProgress,
) -> MaestroResult {
    err_guard(|| {
        let job = unsafe { job(job_ptr) }?;
        let (position_s, speed) = job.progress();
        unsafe {
            sized::write(out, |struct_size| MaestroFileRenderProgress {
                struct_size,
                speed,
                position_s,
                length_s: job.length,
                notes: job.stats.get_notes(),
                events: job.stats.get_events(),
                voice_count: job.stats.get_renderer().read_voice_count(),
            })
        }
    })
}
