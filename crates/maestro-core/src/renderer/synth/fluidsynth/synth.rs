use std::{
    ffi::{CStr, c_int, c_short, c_void},
    sync::Arc,
};

use crate::{
    audio_params::{AudioParameters, ChannelCount},
    error::RendererError,
    event::{MaestroEvent, MaestroTimedEvent, sysex},
    renderer::{
        config::fluidsynth::{FluidSynthConfig, FluidSynthInterpolation},
        synth::{SoundFontHandle, SynthModule, fluidsynth::FluidSynthLib},
    },
};

const FLUID_FAILED: c_int = -1;

fn interp_to_fluid(interp: FluidSynthInterpolation) -> c_int {
    match interp {
        FluidSynthInterpolation::None => 0,
        FluidSynthInterpolation::Linear => 1,
        FluidSynthInterpolation::Sinc4th => 4,
        FluidSynthInterpolation::Sinc7th => 7,
    }
}

pub(crate) struct FluidSynthSynth {
    lib: Arc<FluidSynthLib>,
    settings: *mut c_void,
    synth: *mut c_void,
    seq: *mut c_void,
    seq_event: *mut c_void,

    loaded_fonts: Vec<c_int>,
    channels: ChannelCount,
    stereo_scratch: Vec<f32>,

    rendered: u64,
    last_pos: u32,
}

// SAFETY: the settings/synth handles are owned exclusively by this instance
// and every call that touches them goes through &mut self, except
// voice_count(&self) which only reads a plain counter. The renderer only ever
// drives an instance from one thread at a time (PortSynthManager owns it),
// matching the "synth.threadsafe-api" = 0 contract set below.
unsafe impl Send for FluidSynthSynth {}
unsafe impl Sync for FluidSynthSynth {}

impl FluidSynthSynth {
    pub fn new(
        lib: Arc<FluidSynthLib>,
        config: &FluidSynthConfig,
        audio_params: &AudioParameters,
    ) -> Result<Self, RendererError> {
        let settings = unsafe { (lib.fns.new_fluid_settings)() };
        if settings.is_null() {
            return Err(RendererError::SynthInit(
                "Could not create FluidSynth settings".to_string(),
            ));
        }

        if let Err(err) = Self::apply_settings(&lib, settings, config, audio_params) {
            unsafe { (lib.fns.delete_fluid_settings)(settings) };
            return Err(err);
        }

        let synth = unsafe { (lib.fns.new_fluid_synth)(settings) };
        if synth.is_null() {
            unsafe { (lib.fns.delete_fluid_settings)(settings) };
            return Err(RendererError::SynthInit(
                "Could not create FluidSynth synthesizer".to_string(),
            ));
        }

        // -1 applies the interpolation method to all MIDI channels.
        unsafe {
            (lib.fns.fluid_synth_set_interp_method)(
                synth,
                -1,
                interp_to_fluid(config.interpolation),
            )
        };

        let seq = unsafe { (lib.fns.new_fluid_sequencer2)(0) };
        let seq_event = unsafe { (lib.fns.new_fluid_event)() };
        unsafe {
            (lib.fns.fluid_sequencer_set_time_scale)(seq, audio_params.sample_rate as f64);
            let id = (lib.fns.fluid_sequencer_register_fluidsynth)(seq, synth);
            (lib.fns.fluid_event_set_dest)(seq_event, id);
        }

        Ok(Self {
            lib,
            settings,
            synth,
            seq,
            seq_event,
            loaded_fonts: Vec::new(),
            channels: audio_params.channels,
            stereo_scratch: Vec::new(),
            rendered: 0,
            last_pos: 0,
        })
    }

    fn apply_settings(
        lib: &FluidSynthLib,
        settings: *mut c_void,
        config: &FluidSynthConfig,
        audio_params: &AudioParameters,
    ) -> Result<(), RendererError> {
        let setint = |name: &CStr, val: c_int| unsafe {
            (lib.fns.fluid_settings_setint)(settings, name.as_ptr(), val)
        };
        let setnum = |name: &CStr, val: f64| unsafe {
            (lib.fns.fluid_settings_setnum)(settings, name.as_ptr(), val)
        };

        // it only supports 8kHz..96kHz, failing here beats rendering detuned audio at the default rate.
        let sample_rate = audio_params.sample_rate;
        if setnum(c"synth.sample-rate", sample_rate as f64) == FLUID_FAILED {
            return Err(RendererError::SynthInit(format!(
                "FluidSynth does not support a sample rate of {sample_rate} Hz"
            )));
        }

        setint(
            c"synth.polyphony",
            config.voice_limit.clamp(1, 65535) as c_int,
        );
        setint(
            c"synth.min-note-length",
            config.minimum_note_length.min(65535) as c_int,
        );
        setint(c"synth.device-id", config.system_id.clamp(0, 126));
        setint(c"synth.chorus.active", config.chorus_active as c_int);
        setint(c"synth.reverb.active", config.reverb_active as c_int);

        let overflow = |v: f64| v.clamp(-10000.0, 10000.0);
        setnum(c"synth.overflow.age", overflow(config.overflow_age));
        setnum(
            c"synth.overflow.percussion",
            overflow(config.overflow_percussion),
        );
        setnum(
            c"synth.overflow.released",
            overflow(config.overflow_released),
        );
        setnum(
            c"synth.overflow.sustained",
            overflow(config.overflow_sustained),
        );
        setnum(c"synth.overflow.volume", overflow(config.overflow_volume));

        setint(c"synth.threadsafe-api", 0);

        Ok(())
    }
}

impl SynthModule for FluidSynthSynth {
    fn set_soundfonts(&mut self, handles: &[SoundFontHandle]) -> Result<(), RendererError> {
        for id in self.loaded_fonts.drain(..) {
            unsafe { (self.lib.fns.fluid_synth_sfunload)(self.synth, id, 0) };
        }

        // FluidSynth resolves presets newest-loaded-first while `handles` is
        // ordered highest-priority-first, so load in reverse.
        for handle in handles.iter().rev() {
            let Some(spec) = self.lib.get_soundfont(handle) else {
                continue;
            };

            let id =
                unsafe { (self.lib.fns.fluid_synth_sfload)(self.synth, spec.path.as_ptr(), 1) };
            if id == FLUID_FAILED {
                return Err(RendererError::SoundFont(format!(
                    "FluidSynth could not load the SoundFont {:?}",
                    spec.path
                )));
            }

            if spec.bank_offset > 0 {
                unsafe {
                    (self.lib.fns.fluid_synth_set_bank_offset)(self.synth, id, spec.bank_offset)
                };
            }

            self.loaded_fonts.push(id);
        }

        Ok(())
    }

    fn reset(&mut self) {
        unsafe {
            (self.lib.fns.fluid_sequencer_remove_events)(self.seq, -1, -1, -1);
            (self.lib.fns.fluid_synth_system_reset)(self.synth);
        }
    }

    fn process_event(&mut self, event: MaestroTimedEvent) {
        let channels = u16::from(self.channels) as u64;
        let delta = (event.pos.wrapping_sub(self.last_pos) as i32).max(0) as u64 / channels;
        let tick = self.rendered.wrapping_add(delta) as u32;
        let fns = &self.lib.fns;
        let evt = self.seq_event;

        unsafe {
            match event.event {
                MaestroEvent::NoteOff { channel, key } => {
                    (fns.fluid_event_noteoff)(evt, channel as c_int, key as c_short);
                }
                MaestroEvent::NoteOn { channel, key, vel } => {
                    (fns.fluid_event_noteon)(evt, channel as c_int, key as c_short, vel as c_short);
                }
                MaestroEvent::ControlChange {
                    channel,
                    param,
                    val,
                } => {
                    (fns.fluid_event_control_change)(
                        evt,
                        channel as c_int,
                        param as c_short,
                        val as c_int,
                    );
                }
                MaestroEvent::PitchBendChange { channel, lsb, msb } => {
                    let value = ((msb as c_int) << 7) | (lsb as c_int);
                    (fns.fluid_event_pitch_bend)(evt, channel as c_int, value);
                }
                MaestroEvent::ProgramChange { channel, program } => {
                    (fns.fluid_event_program_change)(evt, channel as c_int, program as c_int);
                }
                MaestroEvent::ChannelAftertouch { channel, pressure } => {
                    (fns.fluid_event_channel_pressure)(evt, channel as c_int, pressure as c_int);
                }
                MaestroEvent::PolyphonicAftertouch {
                    channel,
                    key,
                    pressure,
                } => {
                    (fns.fluid_event_key_pressure)(
                        evt,
                        channel as c_int,
                        key as c_short,
                        pressure as c_int,
                    );
                }
                MaestroEvent::SystemReset => (fns.fluid_event_system_reset)(evt),
                MaestroEvent::SystemExclusive { id } => {
                    sysex::with(id, |data| {
                        let ev = (fns.new_fluid_midi_event)();
                        (fns.fluid_synth_sysex)(
                            self.synth,
                            data.as_ptr() as *const _,
                            data.len() as c_int,
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            0,
                        );
                        (fns.fluid_midi_event_set_sysex)(
                            ev,
                            data.as_ptr() as *mut _,
                            data.len() as c_int,
                            0,
                        );
                        (fns.fluid_event_from_midi_event)(evt, ev);
                        (fns.delete_fluid_midi_event)(ev);
                    });
                    sysex::release(&event.event);
                    return;
                }
            }

            (fns.fluid_sequencer_send_at)(self.seq, evt, tick, 1);
        }

        sysex::release(&event.event);
    }

    fn read_audio(&mut self, buffer: &mut [f32]) {
        self.write_to(buffer);
        self.rendered += (buffer.len() / u16::from(self.channels) as usize) as u64;
        self.last_pos = self.last_pos.wrapping_add(buffer.len() as u32);
    }

    fn voice_count(&self) -> u64 {
        unsafe { (self.lib.fns.fluid_synth_get_active_voice_count)(self.synth) }.max(0) as u64
    }
}

impl FluidSynthSynth {
    fn write_to(&mut self, buffer: &mut [f32]) {
        match self.channels {
            ChannelCount::Stereo => {
                let frames = (buffer.len() / 2) as c_int;
                let ptr = buffer.as_mut_ptr() as *mut c_void;
                // Interleave in place: left samples at even offsets, right at odd.
                unsafe {
                    (self.lib.fns.fluid_synth_write_float)(self.synth, frames, ptr, 0, 2, ptr, 1, 2)
                };
            }
            ChannelCount::Mono => {
                let frames = buffer.len();
                if self.stereo_scratch.len() < frames * 2 {
                    self.stereo_scratch.resize(frames * 2, 0.0);
                }

                let scratch = &mut self.stereo_scratch[..frames * 2];
                let ptr = scratch.as_mut_ptr() as *mut c_void;
                unsafe {
                    (self.lib.fns.fluid_synth_write_float)(
                        self.synth,
                        frames as c_int,
                        ptr,
                        0,
                        2,
                        ptr,
                        1,
                        2,
                    )
                };

                for (i, out) in buffer.iter_mut().enumerate() {
                    *out = (scratch[i * 2] + scratch[i * 2 + 1]) * 0.5;
                }
            }
        }
    }
}

impl Drop for FluidSynthSynth {
    fn drop(&mut self) {
        unsafe {
            (self.lib.fns.delete_fluid_event)(self.seq_event);
            (self.lib.fns.delete_fluid_sequencer)(self.seq);
            (self.lib.fns.delete_fluid_synth)(self.synth);
            (self.lib.fns.delete_fluid_settings)(self.settings);
        }
    }
}
