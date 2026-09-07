use crate::{
    error::{MaestroComponentError, RealtimeEngineError, RendererError},
    realtime::buffered_renderer::BufferedRenderer,
    renderer::{MaestroRenderer, RealtimeClock},
    soundfont::SoundFontList,
    statistics::MaestroRenderStatistics,
};
use cpal::{Stream, traits::StreamTrait};
use std::sync::{Arc, Mutex};

mod buffered_renderer;
pub mod config;
mod event_sender;
pub use event_sender::{RealtimeEventSender, UMP_GROUPS};
mod options;
mod stream;
pub use options::*;
pub use stream::{device_caps, output_devices, output_hosts, set_stream_error_handler};

pub struct MaestroRealtimeEngine {
    renderer: Arc<Mutex<MaestroRenderer>>,
    stream: Stream,
    sender: RealtimeEventSender,
    stats: Arc<MaestroRenderStatistics>,
}

impl MaestroRealtimeEngine {
    pub fn new(options: RealtimeEngineOptions) -> Result<Self, MaestroComponentError> {
        let requested = if options.config.device_audio_params {
            None
        } else {
            Some(options.audio_params)
        };

        let output = stream::resolve_output(requested, &options.config)?;
        let stream_params = output.params;

        let ports = options.ports.unwrap_or(1).clamp(1, 16);
        let mut renderer = MaestroRenderer::new(
            ports,
            options.renderer,
            &stream_params,
            options.event_processor.clone(),
            options.post_processor,
        )?;
        let stats = renderer.get_statistics();

        let clock = Arc::new(RealtimeClock::new(&stream_params));
        renderer.set_realtime_clock(clock.clone());

        let sender = RealtimeEventSender::new(
            &options,
            &stream_params,
            clock,
            renderer.get_port_senders().into_boxed_slice(),
        )?;

        let renderer = Arc::new(Mutex::new(renderer));

        let renderer_cln = renderer.clone();
        let func = move |buffer: &mut [f32]| {
            renderer_cln.lock().unwrap().render(buffer);
        };

        let mut buffered = BufferedRenderer::new(func, stream_params, &options.config)?;

        let func = move |buffer: &mut [f32]| {
            buffered.read(buffer);
        };

        let stream = stream::build_stream(&output, &options.config, func)?;
        stream.play().map_err(|e| {
            RealtimeEngineError::Audio(format!("Failed to start audio stream: {e:?}"))
        })?;

        Ok(Self {
            renderer,
            stream,
            sender,
            stats,
        })
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

    pub fn get_statistics(&self) -> Arc<MaestroRenderStatistics> {
        self.stats.clone()
    }

    pub fn get_event_sender(&self) -> RealtimeEventSender {
        self.sender.clone()
    }

    pub fn pause(&mut self) -> Result<(), cpal::Error> {
        self.stream.pause()
    }

    pub fn play(&mut self) -> Result<(), cpal::Error> {
        self.stream.play()
    }
}
