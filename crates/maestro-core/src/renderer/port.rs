use std::sync::Arc;

use crate::{
    error::RendererError,
    event::MaestroTimedEvent,
    renderer::{
        config::EventProcessorConfig,
        sender::PortEventSender,
        synth::{
            SoundFontHandle, SynthModule, bassmidi::BASSMIDISynth, fluidsynth::FluidSynthSynth,
        },
    },
};

#[allow(clippy::upper_case_acronyms)]
pub(super) enum PortRenderer {
    BASSMIDI(PortSynthManager<BASSMIDISynth>),
    FluidSynth(PortSynthManager<FluidSynthSynth>),
}

impl PortRenderer {
    pub fn process_event(&mut self, event: MaestroTimedEvent) {
        match self {
            Self::BASSMIDI(mgr) => mgr.process_event(event),
            Self::FluidSynth(mgr) => mgr.process_event(event),
        }
    }

    pub fn read_audio(&mut self, dest: &mut [f32], precision_threshold: usize) -> usize {
        match self {
            Self::BASSMIDI(mgr) => mgr.read_audio(dest, precision_threshold),
            Self::FluidSynth(mgr) => mgr.read_audio(dest, precision_threshold),
        }
    }

    pub fn set_soundfonts(&mut self, handles: &[SoundFontHandle]) -> Result<(), RendererError> {
        match self {
            Self::BASSMIDI(mgr) => mgr.set_soundfonts(handles),
            Self::FluidSynth(mgr) => mgr.set_soundfonts(handles),
        }
    }

    pub fn reset(&mut self) {
        match self {
            Self::BASSMIDI(mgr) => mgr.reset(),
            Self::FluidSynth(mgr) => mgr.reset(),
        }
    }

    pub fn voice_count(&self) -> u64 {
        match self {
            Self::BASSMIDI(mgr) => mgr.voice_count(),
            Self::FluidSynth(mgr) => mgr.voice_count(),
        }
    }

    pub fn get_buffer(&self) -> Arc<PortEventSender> {
        match self {
            Self::BASSMIDI(mgr) => mgr.get_buffer(),
            Self::FluidSynth(mgr) => mgr.get_buffer(),
        }
    }
}

pub(crate) struct PortSynthManager<T: SynthModule> {
    synth: T,
    event_snd: Arc<PortEventSender>,
}

impl<T: SynthModule + 'static> PortSynthManager<T> {
    pub fn new(port: u8, synth: T, evproc: Option<EventProcessorConfig>) -> Self {
        Self {
            synth,
            event_snd: Arc::new(PortEventSender::new(port, evproc)),
        }
    }

    pub fn process_event(&self, event: MaestroTimedEvent) {
        self.event_snd.send(event);
    }

    pub fn read_audio(&mut self, dest: &mut [f32], precision_threshold: usize) -> usize {
        dest.fill(0.0);

        for event in self.event_snd.iter() {
            self.synth.process_event(event);
        }

        self.synth.read_audio(dest, precision_threshold);

        dest.len()
    }

    pub fn set_soundfonts(&mut self, handles: &[SoundFontHandle]) -> Result<(), RendererError> {
        self.synth.set_soundfonts(handles)?;

        Ok(())
    }

    pub fn reset(&mut self) {
        self.synth.reset();
    }

    pub fn voice_count(&self) -> u64 {
        self.synth.voice_count()
    }

    pub fn get_buffer(&self) -> Arc<PortEventSender> {
        self.event_snd.clone()
    }
}
