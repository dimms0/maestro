use std::sync::Arc;

use crate::{
    audio_params::AudioParameters,
    error::RendererError,
    event::MaestroTimedEvent,
    renderer::{
        config::{EventProcessorConfig, SynthConfig},
        sender::PortEventSender,
        synth::{
            SoundFontHandle, SynthModule, bassmidi::BASSMIDISynth, fluidsynth::FluidSynthSynth,
            kestrel::KestrelSynth, svms::SVMSSynth,
        },
    },
};

#[allow(clippy::upper_case_acronyms)]
pub(super) enum PortRenderer {
    BASSMIDI(PortSynthManager<BASSMIDISynth>),
    FluidSynth(PortSynthManager<FluidSynthSynth>),
    SVMS(PortSynthManager<SVMSSynth>),
    Kestrel(PortSynthManager<KestrelSynth>),
}

macro_rules! dispatch {
    ($self:expr, $mgr:ident => $body:expr) => {
        match $self {
            PortRenderer::BASSMIDI($mgr) => $body,
            PortRenderer::FluidSynth($mgr) => $body,
            PortRenderer::SVMS($mgr) => $body,
            PortRenderer::Kestrel($mgr) => $body,
        }
    };
}

impl PortRenderer {
    pub fn process_event(&mut self, event: MaestroTimedEvent) {
        dispatch!(self, mgr => mgr.process_event(event))
    }

    pub fn read_audio(&mut self, dest: &mut [f32]) -> usize {
        dispatch!(self, mgr => mgr.read_audio(dest))
    }

    pub fn set_soundfonts(&mut self, handles: &[SoundFontHandle]) -> Result<(), RendererError> {
        dispatch!(self, mgr => mgr.set_soundfonts(handles))
    }

    pub fn reload_config(&mut self, config: &SynthConfig) -> bool {
        match (self, config) {
            (Self::BASSMIDI(mgr), SynthConfig::BASSMIDI(cfg)) => mgr.synth.reload_config(cfg),
            (Self::FluidSynth(mgr), SynthConfig::FluidSynth(cfg)) => mgr.synth.reload_config(cfg),
            (Self::SVMS(mgr), SynthConfig::SVMS(cfg)) => mgr.synth.reload_config(cfg),
            (Self::Kestrel(mgr), SynthConfig::Kestrel(cfg)) => mgr.synth.reload_config(cfg),
            _ => false,
        }
    }

    pub fn reset(&mut self) {
        dispatch!(self, mgr => mgr.reset())
    }

    pub fn voice_count(&self) -> u64 {
        dispatch!(self, mgr => mgr.voice_count())
    }

    pub fn get_buffer(&self) -> Arc<PortEventSender> {
        dispatch!(self, mgr => mgr.get_buffer())
    }
}

pub(crate) struct PortSynthManager<T: SynthModule> {
    synth: T,
    event_snd: Arc<PortEventSender>,
    events: Vec<MaestroTimedEvent>,
    passed_over: bool,
}

impl<T: SynthModule + 'static> PortSynthManager<T> {
    pub fn new(
        port: u8,
        synth: T,
        evproc: Option<EventProcessorConfig>,
        audio_params: &AudioParameters,
    ) -> Self {
        Self {
            synth,
            event_snd: Arc::new(PortEventSender::new(port, evproc, audio_params)),
            events: Vec::new(),
            passed_over: false,
        }
    }

    pub fn process_event(&self, event: MaestroTimedEvent) {
        self.event_snd.send(event);
    }

    pub fn read_audio(&mut self, dest: &mut [f32]) -> usize {
        dest.fill(0.0);

        // events are stamped ahead of the block, so if we cant lock wait for the next one
        // only once, or a sender could keep them back
        self.passed_over = !self.event_snd.swap(&mut self.events, self.passed_over);
        for event in self.events.drain(..) {
            self.synth.process_event(event);
        }

        self.synth.read_audio(dest);

        dest.len()
    }

    pub fn set_soundfonts(&mut self, handles: &[SoundFontHandle]) -> Result<(), RendererError> {
        self.synth.set_soundfonts(handles)?;

        Ok(())
    }

    pub fn reset(&mut self) {
        self.event_snd.reset();
        self.synth.reset();
    }

    pub fn voice_count(&self) -> u64 {
        self.synth.voice_count()
    }

    pub fn get_buffer(&self) -> Arc<PortEventSender> {
        self.event_snd.clone()
    }
}
