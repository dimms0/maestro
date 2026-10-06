use crate::{
    audio_params::AudioParameters,
    error::{MaestroComponentError, RealtimeEngineError, RendererError},
    realtime::{buffered_renderer::BufferedRenderer, load::LoadLimiter},
    renderer::{MaestroRenderer, RealtimeClock},
    soundfont::SoundFontList,
};
use cpal::{Stream, traits::StreamTrait};
use std::sync::{Arc, Mutex};

mod buffered_renderer;
pub mod config;
mod event_sender;
pub use event_sender::{RealtimeEventSender, UMP_GROUPS};
mod load;
mod options;
mod statistics;
pub use statistics::MaestroRealtimeStatistics;
mod stream;
pub use options::*;
pub use stream::{device_caps, output_devices, output_hosts, set_stream_error_handler};

pub struct MaestroRealtimeEngine {
    renderer: Arc<Mutex<MaestroRenderer>>,
    stream: Option<Stream>,
    sender: RealtimeEventSender,
    stats: MaestroRealtimeStatistics,
    options: RealtimeEngineOptions,
    stream_params: AudioParameters,
}

fn requested_params(options: &RealtimeEngineOptions) -> Option<AudioParameters> {
    (!options.config.device_audio_params).then_some(options.audio_params)
}

fn start_stream(
    renderer: &Arc<Mutex<MaestroRenderer>>,
    output: &stream::ResolvedOutput,
    config: &config::RealtimeConfig,
    limiter: Arc<LoadLimiter>,
) -> Result<Stream, RealtimeEngineError> {
    let renderer = renderer.clone();
    let func = move |buffer: &mut [f32]| {
        renderer.lock().unwrap().render(buffer);
    };

    let mut buffered = BufferedRenderer::new(func, output.params, config, limiter)?;

    let func = move |buffer: &mut [f32]| {
        buffered.read(buffer);
    };

    let stream = stream::build_stream(output, config, func)?;
    stream
        .play()
        .map_err(|e| RealtimeEngineError::Audio(format!("Failed to start audio stream: {e:?}")))?;
    Ok(stream)
}

impl MaestroRealtimeEngine {
    pub fn new(options: RealtimeEngineOptions) -> Result<Self, MaestroComponentError> {
        let output = stream::resolve_output(requested_params(&options), &options.config)?;
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

        let clock = Arc::new(RealtimeClock::new(&stream_params));
        renderer.set_realtime_clock(clock.clone());

        let sender = RealtimeEventSender::new(
            &options,
            &stream_params,
            clock,
            renderer.get_port_senders().into_boxed_slice(),
            stats.clone(),
        )?;

        let renderer = Arc::new(Mutex::new(renderer));
        let stream = start_stream(&renderer, &output, &options.config, sender.load_limiter())?;

        Ok(Self {
            renderer,
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
            precision_playback: false,
            max_nps: None,
            coalesce_window_ms: None,
            ..c.clone()
        };
        if stream_config(&old.config) != stream_config(&new.config)
            || old.audio_params != new.audio_params
        {
            self.stream = None;
            let output = stream::resolve_output(requested_params(new), &new.config)?;
            if output.params != self.stream_params {
                return Ok(false);
            }

            let limiter = self.sender.load_limiter();
            self.stream = Some(start_stream(&self.renderer, &output, &new.config, limiter)?);
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
        self.renderer.lock().unwrap().load_soundfonts(&list)
    }

    pub fn get_statistics(&self) -> MaestroRealtimeStatistics {
        self.stats.clone()
    }

    pub fn get_event_sender(&self) -> RealtimeEventSender {
        self.sender.clone()
    }

    pub fn pause(&mut self) -> Result<(), cpal::Error> {
        self.stream.as_ref().map_or(Ok(()), |s| s.pause())
    }

    pub fn play(&mut self) -> Result<(), cpal::Error> {
        self.stream.as_ref().map_or(Ok(()), |s| s.play())
    }
}
