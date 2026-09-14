use std::{ffi::c_void, sync::Arc};

use super::{BASSMIDILib, consts::*};
use crate::{
    audio_params::{AudioParameters, ChannelCount},
    error::RendererError,
    event::{MaestroEvent, MaestroTimedEvent, sysex},
    helpers::gcd,
    renderer::{
        config::bassmidi::BASSMIDIConfig,
        synth::{EventBuffer, SoundFontHandle, SynthModule},
    },
};

fn write_delta(buf: &mut Vec<u8>, mut delta: u32) {
    let mut bytes = [0u8; 5];
    let mut len = 0;

    loop {
        bytes[len] = (delta & 0x7f) as u8;
        delta >>= 7;
        len += 1;

        if delta == 0 {
            break;
        }
    }

    while len > 0 {
        len -= 1;
        buf.push(bytes[len] | if len > 0 { 0x80 } else { 0 });
    }
}

fn write_event(buf: &mut Vec<u8>, event: MaestroEvent) {
    match event {
        MaestroEvent::NoteOff { channel, key } => buf.extend([0x80 | channel, key, 0]),
        MaestroEvent::NoteOn { channel, key, vel } => buf.extend([0x90 | channel, key, vel]),
        MaestroEvent::PolyphonicAftertouch {
            channel,
            key,
            pressure,
        } => buf.extend([0xA0 | channel, key, pressure]),
        MaestroEvent::ControlChange {
            channel,
            param,
            val,
        } => buf.extend([0xB0 | channel, param, val]),
        MaestroEvent::ProgramChange { channel, program } => buf.extend([0xC0 | channel, program]),
        MaestroEvent::ChannelAftertouch { channel, pressure } => {
            buf.extend([0xD0 | channel, pressure])
        }
        MaestroEvent::PitchBendChange { channel, lsb, msb } => {
            buf.extend([0xE0 | channel, lsb, msb])
        }
        MaestroEvent::SystemExclusive { id } => {
            sysex::with(id, |data| {
                buf.push(0xF0);
                buf.extend_from_slice(data);
                buf.push(0xF7);
            });
        }
        MaestroEvent::SystemReset => {
            // GS reset
            buf.extend([
                0xF0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41, 0xF7,
            ]);
        }
    }
}

pub(crate) struct BASSMIDIStream {
    lib: Arc<BASSMIDILib>,
    stream: u32,
    channels: u32,

    event_buf: EventBuffer,
    raw: Vec<u8>,
    last_pos: u32,
}

impl BASSMIDIStream {
    pub fn initialize(
        lib: Arc<BASSMIDILib>,
        config: &BASSMIDIConfig,
        audio_params: &AudioParameters,
    ) -> Result<Self, RendererError> {
        let flags = BASS_MIDI_DECAYEND | BASS_SAMPLE_FLOAT | BASS_STREAM_DECODE;
        let flags = flags
            | if config.note_off1 {
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
                if config.fade_out_killing { 1.0 } else { 0.0 },
            );
            (lib.bass.BASS_ChannelSetAttribute)(
                stream,
                BASS_ATTRIB_MIDI_CPU,
                config.render_time_limit,
            );
            (lib.bass.BASS_ChannelSetAttribute)(
                stream,
                BASS_ATTRIB_MIDI_EXCKEYS,
                config.exclusive_keys.clamp(0, 2) as f32,
            );
        }

        // g = gcd(sample_rate, 1_000_000), ppqn = sample_rate / g makes
        // tempo = ppqn * 1_000_000 / sample_rate collapse to 1_000_000 / g,
        // which is always an exact integer, that makes 1 tick == 1 frame
        let sample_rate = audio_params.sample_rate as u64;
        let divisor = gcd(sample_rate, 1_000_000);
        let ppqn = sample_rate / divisor;
        let tempo = (1_000_000 / divisor) as u32;

        unsafe {
            (lib.bass.BASS_ChannelSetAttribute)(stream, BASS_ATTRIB_MIDI_PPQN, ppqn as f32);
            (lib.bassmidi.BASS_MIDI_StreamEvent)(stream, 0, MIDI_EVENT_TEMPO, tempo);
        }

        Ok(Self {
            lib,
            stream,
            channels: u16::from(audio_params.channels) as u32,
            last_pos: 0,
            event_buf: EventBuffer::new(),
            raw: Vec::new(),
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
            (self.lib.bassmidi.BASS_MIDI_StreamEvents)(
                self.stream,
                BASS_MIDI_EVENTS_STRUCT | BASS_MIDI_EVENTS_CANCEL,
                std::ptr::null(),
                0,
            );
            (self.lib.bassmidi.BASS_MIDI_StreamEvent)(self.stream, 0, MIDI_EVENT_SYSTEM, 0);
        }
    }

    fn process_event(&mut self, event: MaestroTimedEvent) {
        self.event_buf.push(event);
    }

    fn read_audio(&mut self, buffer: &mut [f32]) {
        let base = self.last_pos;
        self.event_buf.sort(base);
        self.raw.clear();

        let mut prev = 0;
        while let Some(event) = self.event_buf.pop_front() {
            let delta = (event.pos.wrapping_sub(base) as i32).max(0) as u32 / self.channels;

            write_delta(&mut self.raw, delta - prev);
            write_event(&mut self.raw, event.event);
            prev = delta;

            sysex::release(&event.event);
        }

        unsafe {
            if !self.raw.is_empty() {
                (self.lib.bassmidi.BASS_MIDI_StreamEvents)(
                    self.stream,
                    BASS_MIDI_EVENTS_RAW | BASS_MIDI_EVENTS_NORSTATUS | BASS_MIDI_EVENTS_TIME,
                    self.raw.as_ptr() as *const c_void,
                    self.raw.len() as u32,
                );
            }

            (self.lib.bass.BASS_ChannelGetData)(
                self.stream,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as u32 * 4,
            );
        }

        self.last_pos = base.wrapping_add(buffer.len() as u32);
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

impl Drop for BASSMIDIStream {
    fn drop(&mut self) {
        unsafe {
            (self.lib.bass.BASS_StreamFree)(self.stream);
        }
    }
}
