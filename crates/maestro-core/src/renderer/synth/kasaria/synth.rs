use std::ffi::c_long;
use std::sync::Arc;

use crate::{
    audio_params::{AudioParameters, ChannelCount},
    error::RendererError,
    event::{MaestroEvent, MaestroTimedEvent, sysex},
    helpers::prepapre_cache_vec,
    renderer::{
        config::kasaria::KasariaConfig,
        synth::{
            EventBuffer, SoundFontHandle, SynthModule,
            kasaria::{
                KasariaLib,
                library::{Kasaria, KsrSoundfontOpts},
            },
        },
    },
};

fn short(event: MaestroEvent) -> Option<[u8; 3]> {
    Some(match event {
        MaestroEvent::NoteOff { channel, key } => [0x80 | channel, key, 0],
        MaestroEvent::NoteOn { channel, key, vel } => [0x90 | channel, key, vel],
        MaestroEvent::PolyphonicAftertouch {
            channel,
            key,
            pressure,
        } => [0xA0 | channel, key, pressure],
        MaestroEvent::ControlChange {
            channel,
            param,
            val,
        } => [0xB0 | channel, param, val],
        MaestroEvent::ProgramChange { channel, program } => [0xC0 | channel, program, 0],
        MaestroEvent::PitchBendChange { channel, lsb, msb } => [0xE0 | channel, lsb, msb],
        // Kasaria has no channel pressure
        MaestroEvent::ChannelAftertouch { .. }
        | MaestroEvent::SystemReset
        | MaestroEvent::SystemExclusive { .. } => return None,
    })
}

pub(crate) struct KasariaSynth {
    lib: Arc<KasariaLib>,
    // Null until soundfonts are set
    ksr: *mut Kasaria,

    sample_rate: u32,
    channels: ChannelCount,
    config: KasariaConfig,

    event_buf: EventBuffer,
    stereo: Vec<f32>,
    last_pos: u32,
}

// The instance is only ever touched through &mut self
unsafe impl Send for KasariaSynth {}
unsafe impl Sync for KasariaSynth {}

impl KasariaSynth {
    pub fn new(
        lib: Arc<KasariaLib>,
        config: &KasariaConfig,
        audio_params: &AudioParameters,
    ) -> Result<Self, RendererError> {
        Ok(Self {
            lib,
            ksr: std::ptr::null_mut(),
            sample_rate: audio_params.sample_rate,
            channels: audio_params.channels,
            config: *config,
            event_buf: EventBuffer::new(),
            stereo: Vec::new(),
            last_pos: 0,
        })
    }

    fn destroy(&mut self) {
        if !self.ksr.is_null() {
            unsafe { (self.lib.fns.ksr_shutdown)(self.ksr) };
            self.ksr = std::ptr::null_mut();
        }
    }

    fn create(&self) -> Result<*mut Kasaria, RendererError> {
        let fns = &self.lib.fns;
        let ksr = unsafe { (fns.ksr_init)(true) };
        if ksr.is_null() {
            return Err(RendererError::SynthInit(
                "Kasaria could not create a synthesizer instance. It allows a limited number \
                 of instances per process."
                    .to_string(),
            ));
        }

        let c = &self.config;
        let mut raw = unsafe { (fns.ksr_get_config)(ksr) };
        raw.sample_rate = self.sample_rate as _;
        raw.voice_limit = c.voice_limit as _;
        raw.fast_decay = c.fast_decay;
        raw.antialiasing = c.antialiasing;
        raw.pre_resample = c.pre_resample;
        raw.audio_compressor = false;
        raw.mono_audio = false;
        unsafe { (fns.ksr_set_config)(ksr, raw) };

        Ok(ksr)
    }

    fn render(&mut self, from: usize, to: usize) {
        if to > from {
            unsafe {
                (self.lib.fns.ksr_render_float)(
                    self.ksr,
                    self.stereo[from * 2..].as_mut_ptr(),
                    (to - from) as c_long,
                )
            };
        }
    }

    fn send(&self, event: MaestroEvent) {
        let fns = &self.lib.fns;

        match event {
            MaestroEvent::SystemReset => unsafe { (fns.ksr_reset)(self.ksr) },
            MaestroEvent::SystemExclusive { id } => {
                sysex::with(id, |data| {
                    let mut msg = Vec::with_capacity(data.len() + 2);
                    if data.first() != Some(&0xF0) {
                        msg.push(0xF0);
                    }
                    msg.extend_from_slice(data);
                    if msg.last() != Some(&0xF7) {
                        msg.push(0xF7);
                    }
                    unsafe { (fns.ksr_write_sysex)(self.ksr, msg.as_mut_ptr(), msg.len() as _) };
                });
            }
            ev => {
                if let Some([status, d1, d2]) = short(ev) {
                    unsafe { (fns.ksr_write_midi_ev)(self.ksr, status, d1, d2) };
                }
            }
        }
    }
}

impl SynthModule for KasariaSynth {
    type Config = KasariaConfig;

    fn reload_config(&mut self, config: &KasariaConfig) -> bool {
        // Some settings reload the sample bank, so any change rebuilds the synthesizer
        self.config == *config
    }

    fn set_soundfonts(&mut self, handles: &[SoundFontHandle]) -> Result<(), RendererError> {
        self.destroy();

        let paths: Vec<_> = handles
            .iter()
            .filter_map(|h| self.lib.get_soundfont(h))
            .collect();
        if paths.is_empty() {
            return Ok(());
        }

        let ksr = self.create()?;

        // Fonts loaded later override earlier ones, while `handles` is ordered
        // highest-priority-first, so load in reverse.
        for path in paths.iter().rev() {
            let opts = KsrSoundfontOpts {
                active_presets: 128,
                bank: 0,
                preset: 0,
                load_percussion_bank: true,
            };
            if unsafe { (self.lib.fns.ksr_load_soundfont_file)(ksr, path.as_ptr(), opts) } == 0 {
                unsafe { (self.lib.fns.ksr_shutdown)(ksr) };
                return Err(RendererError::SoundFont(format!(
                    "Kasaria could not load the SoundFont {path:?}"
                )));
            }
        }

        self.ksr = ksr;
        Ok(())
    }

    fn reset(&mut self) {
        self.event_buf.clear();
        if !self.ksr.is_null() {
            unsafe { (self.lib.fns.ksr_reset)(self.ksr) };
        }
    }

    fn process_event(&mut self, event: MaestroTimedEvent) {
        self.event_buf.insert(event, self.last_pos);
    }

    fn read_audio(&mut self, buffer: &mut [f32]) {
        let channels = u16::from(self.channels) as usize;
        let frames = buffer.len() / channels;
        let base = self.last_pos;
        let end = base.wrapping_add(buffer.len() as u32);
        self.last_pos = end;

        if self.ksr.is_null() {
            while let Some(event) = self.event_buf.pop_before(end, base) {
                sysex::release(&event.event);
            }
            return;
        }

        prepapre_cache_vec(&mut self.stereo, frames * 2, 0.0);

        let mut rendered = 0;
        while let Some(event) = self.event_buf.pop_before(end, base) {
            let offset = ((event.pos.wrapping_sub(base) as i32).max(0) as usize / channels)
                .clamp(rendered, frames);

            self.render(rendered, offset);
            rendered = offset;
            self.send(event.event);
            sysex::release(&event.event);
        }
        self.render(rendered, frames);

        match self.channels {
            ChannelCount::Stereo => buffer.copy_from_slice(&self.stereo),
            ChannelCount::Mono => {
                for (out, lr) in buffer.iter_mut().zip(self.stereo.chunks_exact(2)) {
                    *out = (lr[0] + lr[1]) * 0.5;
                }
            }
        }
    }

    fn voice_count(&self) -> u64 {
        match self.ksr.is_null() {
            true => 0,
            false => unsafe { (self.lib.fns.ksr_get_active_voices)(self.ksr) }.max(0) as u64,
        }
    }
}

impl Drop for KasariaSynth {
    fn drop(&mut self) {
        self.destroy();
    }
}
