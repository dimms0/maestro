use std::{ffi::c_void, sync::Arc};

use super::{BASSMIDILib, consts::*};
use crate::{
    audio_params::{AudioParameters, ChannelCount},
    error::RendererError,
    event::{MaestroEvent, MaestroTimedEvent},
    renderer::{
        config::bassmidi::BASSMIDIConfig,
        synth::{EventBuffer, MidiStreamState, RenderableMidiStream, SoundFontHandle, SynthModule},
    },
};

pub(crate) struct BASSMIDIStream {
    lib: Arc<BASSMIDILib>,
    stream: u32,

    event_buf: EventBuffer,
    last_pos: u64,
}

impl BASSMIDIStream {
    pub fn initialize(
        lib: Arc<BASSMIDILib>,
        config: &BASSMIDIConfig,
        audio_params: &AudioParameters,
    ) -> Result<Self, RendererError> {
        let flags = BASS_MIDI_DECAYEND | BASS_SAMPLE_FLOAT | BASS_STREAM_DECODE;
        let flags = flags
            | if config.follow_overlaps {
                BASS_MIDI_NOTEOFF1
            } else {
                0
            };
        let flags = flags
            | if config.disable_effects {
                BASS_MIDI_NOFX
            } else {
                0
            };
        let flags = flags
            | if audio_params.channels == ChannelCount::Mono {
                BASS_SAMPLE_MONO
            } else {
                0
            };

        let stream =
            unsafe { (lib.bassmidi.BASS_MIDI_StreamCreate)(16, flags, audio_params.sample_rate) };

        if stream == 0 {
            let code = unsafe { (lib.bass.BASS_ErrorGetCode)() };
            let error = format!("Could not create BASSMIDI stream. Code={code}");
            return Err(RendererError::SynthInit(error));
        }

        unsafe {
            (lib.bass.BASS_ChannelSetAttribute)(
                stream,
                BASS_ATTRIB_MIDI_VOICES,
                config.voice_limit as f32,
            );
            (lib.bass.BASS_ChannelSetAttribute)(
                stream,
                BASS_ATTRIB_MIDI_SRC,
                Into::<i32>::into(config.interpolation) as f32,
            );
            (lib.bass.BASS_ChannelSetAttribute)(
                stream,
                BASS_ATTRIB_MIDI_KILL,
                if config.fade_out_killing { 0.0 } else { 1.0 },
            );
            (lib.bass.BASS_ChannelSetAttribute)(
                stream,
                BASS_ATTRIB_MIDI_CPU,
                config.render_time_limit,
            );
        }

        Ok(Self {
            lib,
            stream,
            last_pos: 0,
            event_buf: EventBuffer::new(),
        })
    }

    pub fn set_drum_channel(&mut self, channel: u8, is_percussion: bool) {
        let b = if is_percussion { 1 } else { 0 };

        unsafe {
            (self.lib.bassmidi.BASS_MIDI_StreamEvent)(
                self.stream,
                channel as u32,
                MIDI_EVENT_DEFDRUMS,
                b,
            );
        }
    }
}

impl SynthModule for BASSMIDIStream {
    fn set_soundfonts(&mut self, handles: &[SoundFontHandle]) -> Result<(), RendererError> {
        let bass_handles = handles
            .iter()
            .filter_map(|h| self.lib.get_soundfont(h))
            .collect::<Vec<_>>();

        unsafe {
            let res = (self.lib.bassmidi.BASS_MIDI_StreamSetFonts)(
                self.stream,
                bass_handles.as_ptr() as *const c_void,
                bass_handles.len() as u32,
            );

            if res == 0 {
                let code = (self.lib.bass.BASS_ErrorGetCode)();
                let error = format!("Error setting soundfonts: Code={code}");
                return Err(RendererError::SoundFont(error));
            }
        }

        Ok(())
    }

    fn reset(&mut self) {
        self.event_buf.clear();
        unsafe {
            (self.lib.bassmidi.BASS_MIDI_StreamEvent)(self.stream, 0, MIDI_EVENT_SYSTEM, 0);
        }
    }

    fn process_event(&mut self, event: MaestroTimedEvent) {
        self.event_buf.push(event);
    }

    fn read_audio(&mut self, buffer: &mut [f32], precision_threshold: usize) {
        self.render(buffer, precision_threshold);
    }

    fn voice_count(&self) -> u64 {
        let mut active_voices = 0.0;
        unsafe {
            (self.lib.bass.BASS_ChannelGetAttribute)(
                self.stream,
                BASS_ATTRIB_MIDI_VOICES_ACTIVE,
                &mut active_voices,
            );
        }

        active_voices as u64
    }
}

impl MidiStreamState for BASSMIDIStream {
    fn event_buf(&mut self) -> &mut EventBuffer {
        &mut self.event_buf
    }

    fn last_pos(&self) -> u64 {
        self.last_pos
    }

    fn set_last_pos(&mut self, pos: u64) {
        self.last_pos = pos;
    }

    fn write_to(&mut self, buffer: &mut [f32]) {
        unsafe {
            (self.lib.bass.BASS_ChannelGetData)(
                self.stream,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as u32 * 4,
            );
        }
    }

    fn flush_event(&mut self, event: MaestroEvent) {
        let chan;
        let ev;
        let param;

        match event {
            MaestroEvent::NoteOff { channel, key } => {
                chan = channel as u32;
                ev = MIDI_EVENT_NOTE;
                param = key as u32;
            }
            MaestroEvent::NoteOn { channel, key, vel } => {
                chan = channel as u32;
                ev = MIDI_EVENT_NOTE;
                param = (key as u32) | ((vel as u32) << 8);
            }
            MaestroEvent::ControlChange {
                channel,
                param,
                val,
            } => {
                // TODO maybe match this to each control event....
                // keep in mind: RPN/NRPN
                let status = 0xB0 | (channel & 0xF);
                let midi_bytes = [status, param, val];
                unsafe {
                    (self.lib.bassmidi.BASS_MIDI_StreamEvents)(
                        self.stream,
                        BASS_MIDI_EVENTS_RAW | BASS_MIDI_EVENTS_NORSTATUS,
                        midi_bytes.as_ptr() as *const c_void,
                        3,
                    );
                }
                return;
            }
            MaestroEvent::PitchBendChange { channel, lsb, msb } => {
                chan = channel as u32;
                ev = MIDI_EVENT_PITCH;
                param = (msb as u32) << 7 | (lsb as u32);
            }
            MaestroEvent::ProgramChange { channel, program } => {
                chan = channel as u32;
                ev = MIDI_EVENT_PROGRAM;
                param = program as u32;
            }
            MaestroEvent::ChannelAftertouch { channel, pressure } => {
                chan = channel as u32;
                ev = MIDI_EVENT_CHANPRES;
                param = pressure as u32;
            }
            MaestroEvent::PolyphonicAftertouch {
                channel,
                key,
                pressure,
            } => {
                chan = channel as u32;
                ev = MIDI_EVENT_KEYPRES;
                param = (pressure as u32) << 8 | key as u32;
            }
            MaestroEvent::SystemReset => {
                chan = 0;
                ev = MIDI_EVENT_SYSTEM;
                param = 0;
            }
            MaestroEvent::SystemExclusive(data) => {
                let mut ev = Vec::with_capacity(data.len() + 2);
                ev.push(0xF0);
                ev.extend_from_slice(&data);
                ev.push(0xF7);

                unsafe {
                    (self.lib.bassmidi.BASS_MIDI_StreamEvents)(
                        self.stream,
                        BASS_MIDI_EVENTS_RAW | BASS_MIDI_EVENTS_NORSTATUS,
                        ev.as_ptr() as *const c_void,
                        ev.len() as u32,
                    );
                }
                return;
            }
        }

        unsafe {
            (self.lib.bassmidi.BASS_MIDI_StreamEvent)(self.stream, chan, ev, param);
        }
    }
}

impl Drop for BASSMIDIStream {
    fn drop(&mut self) {
        unsafe {
            (self.lib.bass.BASS_StreamFree)(self.stream);
        }
    }
}
