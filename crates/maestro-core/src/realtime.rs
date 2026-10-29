use crate::{
    audio_params::AudioParameters,
    error::{MaestroComponentError, RealtimeEngineError, RendererError},
    realtime::{buffered_renderer::BufferedRenderer, clock::RealtimeClock, load::LoadLimiter},
    renderer::MaestroRenderer,
    soundfont::SoundFontList,
};
use cpal::{Stream, traits::StreamTrait};
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod buffered_renderer;
mod clock;
pub mod config;
mod event_sender;
pub mod host_time;
pub use clock::MAX_SCHEDULE_SECS;
pub use event_sender::{RealtimeEventSender, UMP_GROUPS};
mod load;
mod options;
pub(crate) mod priority;
mod statistics;
pub use statistics::MaestroRealtimeStatistics;
mod stream;
pub use options::*;
pub use stream::{device_caps, output_devices, output_hosts, set_stream_error_handler};

pub struct MaestroRealtimeEngine {
    renderer: Arc<Mutex<MaestroRenderer>>,
    clock: Arc<RealtimeClock>,
    stream: Option<Stream>,
    sender: RealtimeEventSender,
    stats: MaestroRealtimeStatistics,
    options: RealtimeEngineOptions,
    stream_params: AudioParameters,
}

fn requested_params(options: &RealtimeEngineOptions) -> Option<AudioParameters> {
    (!options.config.device_audio_params).then_some(options.audio_params)
}

// run the audio device stuff on a different thread than the host app
// to avoid RPC_E_CHANGED_MODE errors on WASAPI in case it is already
// initialized ft. microslop
#[cfg(windows)]
fn on_device_thread<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .name("maestro-device".to_string())
            .spawn_scoped(s, f)
            .expect("failed to spawn audio device thread")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

#[cfg(not(windows))]
fn on_device_thread<T>(f: impl FnOnce() -> T) -> T {
    f()
}

fn start_stream(
    renderer: &Arc<Mutex<MaestroRenderer>>,
    clock: &Arc<RealtimeClock>,
    output: &stream::ResolvedOutput,
    config: &config::RealtimeConfig,
    limiter: Arc<LoadLimiter>,
    stats: &MaestroRealtimeStatistics,
) -> Result<Stream, RealtimeEngineError> {
    let renderer = renderer.clone();

    let func = {
        let clock = clock.clone();
        move |buffer: &mut [f32], queued: u64| {
            clock.begin_block(buffer.len() as u64, queued);
            renderer.lock().unwrap().render(buffer);
        }
    };

    let mut buffered = BufferedRenderer::new(func, output.params, config, limiter, stats.clone())?;

    let func = {
        let clock = clock.clone();
        move |buffer: &mut [f32], output_delay: Duration| {
            clock.set_output_delay(output_delay);
            buffered.read(buffer);
        }
    };

    let stream = stream::build_stream(output, config, func)?;
    stream
        .play()
        .map_err(|e| RealtimeEngineError::Audio(format!("Failed to start audio stream: {e:?}")))?;
    Ok(stream)
}

impl MaestroRealtimeEngine {
    pub fn new(options: RealtimeEngineOptions) -> Result<Self, MaestroComponentError> {
        let output = on_device_thread(|| {
            stream::resolve_output(requested_params(&options), &options.config)
        })?;
        let stream_params = output.params;

        let ports = options.ports.unwrap_or(1).clamp(1, 16);
        let mut renderer = MaestroRenderer::new(
            ports,
            options.renderer,
            &stream_params,
            options.event_processor.clone(),
            options.post_processor,
        )?;
        let stats = MaestroRealtimeStatistics::new();
        renderer.set_statistics(stats.renderer_handle());

        let clock = Arc::new(RealtimeClock::new(
            &stream_params,
            options.config.auto_timestamp,
        ));
        let sender = RealtimeEventSender::new(
            &options,
            clock.clone(),
            renderer.get_port_senders().into_boxed_slice(),
            stats.clone(),
        )?;

        let renderer = Arc::new(Mutex::new(renderer));
        let limiter = sender.load_limiter();
        let stream = on_device_thread(|| {
            start_stream(&renderer, &clock, &output, &options.config, limiter, &stats)
        })?;

        Ok(Self {
            renderer,
            clock,
            stream: Some(stream),
            sender,
            stats,
            options,
            stream_params,
        })
    }

    pub fn reload_config(
        &mut self,
        options: RealtimeEngineOptions,
    ) -> Result<bool, MaestroComponentError> {
        let (old, new) = (&self.options, &options);
        if old.ports != new.ports {
            return Ok(false);
        }

        let reloaded = self.renderer.lock().unwrap().reload_config(
            new.renderer,
            new.event_processor.clone(),
            new.post_processor,
        )?;
        if !reloaded {
            return Ok(false);
        }

        self.sender.reload_config(&new.config);

        // Whatever the sender doesn't handle belongs to the output stream
        let stream_config = |c: &config::RealtimeConfig| config::RealtimeConfig {
            auto_timestamp: false,
            max_nps: None,
            coalesce_window_ms: None,
            ..c.clone()
        };
        if stream_config(&old.config) != stream_config(&new.config)
            || old.audio_params != new.audio_params
        {
            self.stream = None;
            let output =
                on_device_thread(|| stream::resolve_output(requested_params(new), &new.config))?;
            if output.params != self.stream_params {
                return Ok(false);
            }

            let limiter = self.sender.load_limiter();
            let (renderer, clock, stats) = (&self.renderer, &self.clock, &self.stats);
            self.stream = Some(on_device_thread(|| {
                start_stream(renderer, clock, &output, &new.config, limiter, stats)
            })?);
        }

        self.options = options;
        Ok(true)
    }

    pub fn sender(&mut self) -> &mut RealtimeEventSender {
        &mut self.sender
    }

    pub fn reset(&mut self) {
        self.renderer.lock().unwrap().reset();
        self.sender.reset();
    }

    pub fn set_soundfonts(&self, list: SoundFontList) -> Result<(), RendererError> {
        let loader = self.renderer.lock().unwrap().soundfont_loader();
        let loaded = loader(&list)?;
        self.renderer.lock().unwrap().apply_soundfonts(loaded)
    }

    pub fn get_statistics(&self) -> MaestroRealtimeStatistics {
        self.stats.clone()
    }

    pub fn get_event_sender(&self) -> RealtimeEventSender {
        self.sender.clone()
    }

    pub fn stream_params(&self) -> AudioParameters {
        self.stream_params
    }

    pub fn latency(&self) -> Duration {
        self.clock.latency()
    }

    pub fn pause(&mut self) -> Result<(), cpal::Error> {
        self.stream.as_ref().map_or(Ok(()), |s| s.pause())
    }

    pub fn play(&mut self) -> Result<(), cpal::Error> {
        self.stream.as_ref().map_or(Ok(()), |s| s.play())
    }
}
