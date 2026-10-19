pub mod bassmidi;
pub mod fluidsynth;

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum SynthConfig {
    BASSMIDI(bassmidi::BASSMIDIConfig),
    FluidSynth(fluidsynth::FluidSynthConfig),
}

impl Default for SynthConfig {
    fn default() -> Self {
        let available = crate::renderer::SynthAvailability::cached();
        match !available.bassmidi && available.fluidsynth {
            true => Self::FluidSynth(Default::default()),
            false => Self::BASSMIDI(Default::default()),
        }
    }
}
