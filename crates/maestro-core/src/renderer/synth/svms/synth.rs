use std::sync::Arc;

use crate::{
    audio_params::{AudioParameters, ChannelCount},
    error::RendererError,
    event::{MaestroEvent, MaestroTimedEvent, sysex},
    helpers::prepapre_cache_vec,
    renderer::{
        config::svms::SVMSConfig,
        synth::{
            EventBuffer, SoundFontHandle, SynthModule,
            svms::{
                SVMSLib,
                library::{
                    SVMS_MAX_SOUNDFONT_STACK, SVMS_RESULT_OK, SVMS_SESSION_OFFLINE_RENDER,
                    SVMS_STRUCT_VERSION_1, SVMSFont, SVMSOfflineEvent, SVMSOfflineSessionConfig,
                    SVMSOfflineSoundFontEntry, SVMSOfflineTelemetry, SVMSSession,
                },
            },
        },
    },
};

const MAX_BLOCK_FRAMES: u32 = 8192;

fn pack(event: MaestroEvent) -> Option<u32> {
    let msg = |status: u8, d1: u8, d2: u8| status as u32 | (d1 as u32) << 8 | (d2 as u32) << 16;

    Some(match event {
        MaestroEvent::NoteOff { channel, key } => msg(0x80 | channel, key, 0),
        MaestroEvent::NoteOn { channel, key, vel } => msg(0x90 | channel, key, vel),
        MaestroEvent::PolyphonicAftertouch {
            channel,
            key,
            pressure,
        } => msg(0xA0 | channel, key, pressure),
        MaestroEvent::ControlChange {
            channel,
            param,
            val,
        } => msg(0xB0 | channel, param, val),
        MaestroEvent::ProgramChange { channel, program } => msg(0xC0 | channel, program, 0),
        MaestroEvent::ChannelAftertouch { channel, pressure } => msg(0xD0 | channel, pressure, 0),
        MaestroEvent::PitchBendChange { channel, lsb, msb } => msg(0xE0 | channel, lsb, msb),
        MaestroEvent::SystemReset | MaestroEvent::SystemExclusive { .. } => return None,
    })
}

pub(crate) struct SVMSSynth {
    lib: Arc<SVMSLib>,
    session: Option<SVMSSession>,

    sample_rate: u32,
    channels: ChannelCount,
    config: SVMSConfig,

    event_buf: EventBuffer,
    pending: Vec<SVMSOfflineEvent>,
    left: Vec<f32>,
    right: Vec<f32>,
    last_pos: u32,
}

impl SVMSSynth {
    pub fn new(
        lib: Arc<SVMSLib>,
        config: &SVMSConfig,
        audio_params: &AudioParameters,
    ) -> Result<Self, RendererError> {
        Ok(Self {
            lib,
            session: None,
            sample_rate: audio_params.sample_rate,
            channels: audio_params.channels,
            config: config.clamped(),
            event_buf: EventBuffer::new(),
            pending: Vec::new(),
            left: Vec::new(),
            right: Vec::new(),
            last_pos: 0,
        })
    }

    fn create_session(&self, fonts: &[SVMSFont]) -> Result<SVMSSession, RendererError> {
        let c = &self.config;
        let session_config = SVMSOfflineSessionConfig {
            struct_size: size_of::<SVMSOfflineSessionConfig>() as u32,
            struct_version: SVMS_STRUCT_VERSION_1,
            session_kind: SVMS_SESSION_OFFLINE_RENDER,
            flags: 0,
            sample_rate: self.sample_rate,
            max_voices: c.voice_limit,
            render_threads: c.render_threads,
            max_block_frames: MAX_BLOCK_FRAMES,
            render_backend: i32::from(c.render_backend) as u32,
            master_volume: 1.0,
            // The limiter stays disabled, but SVMS still range-checks these
            // (threshold 0.1..=1, attack >= 0.01, release >= 1), so zero is rejected
            limiter_threshold: 0.95,
            limiter_lookahead_ms: 3.0,
            limiter_attack_ms: 0.5,
            limiter_release_ms: 100.0,
            reserved: [0; 4],
            ..Default::default()
        };

        let soundfont = &fonts[0].path;
        let mut session = 0;
        let res = unsafe {
            (self.lib.api.create_offline_session)(&session_config, soundfont.as_ptr(), &mut session)
        };
        if res != SVMS_RESULT_OK {
            return Err(RendererError::SynthInit(format!(
                "SuperVirtualMIDISynth could not create a session for {soundfont:?} (code {res}). \
                 The SoundFont may be invalid, or the selected render backend unsupported \
                 on this CPU."
            )));
        }

        if let Some(set_entries) = self.lib.api.set_offline_soundfont_entries
            && (fonts.len() > 1 || fonts[0].bank >= 0 || fonts[0].program >= 0)
        {
            let entries: Vec<_> = fonts
                .iter()
                .map(|f| SVMSOfflineSoundFontEntry {
                    struct_size: size_of::<SVMSOfflineSoundFontEntry>() as u32,
                    struct_version: SVMS_STRUCT_VERSION_1,
                    path_utf8: f.path.as_ptr(),
                    bank: f.bank,
                    program: f.program,
                    reserved: [0; 4],
                })
                .collect();

            let res = unsafe { set_entries(session, entries.as_ptr(), entries.len() as u32) };
            if res != SVMS_RESULT_OK {
                unsafe { (self.lib.api.destroy_session)(session) };
                return Err(RendererError::SoundFont(format!(
                    "SuperVirtualMIDISynth could not load the SoundFont stack (code {res})"
                )));
            }
        }

        Ok(session)
    }

    fn destroy_session(&mut self) {
        if let Some(session) = self.session.take() {
            unsafe { (self.lib.api.destroy_session)(session) };
        }
    }

    fn render_span(&mut self, session: SVMSSession, from: usize, to: usize) {
        let mut start = from;
        let mut next = 0;

        loop {
            let len = (to - start).min(MAX_BLOCK_FRAMES as usize);
            let end = start + len;
            let last = end == to;

            let first = next;
            while let Some(ev) = self.pending.get_mut(next) {
                let offset = ev.frame_offset as usize;
                if offset > end || (offset == end && !last) {
                    break;
                }
                ev.frame_offset = (offset - start) as u32;
                next += 1;
            }

            let events = &self.pending[first..next];
            if len > 0 || !events.is_empty() {
                unsafe {
                    (self.lib.api.render_offline)(
                        session,
                        events.as_ptr(),
                        events.len() as u32,
                        self.left[start..].as_mut_ptr(),
                        self.right[start..].as_mut_ptr(),
                        len as u32,
                    )
                };
            }

            start = end;
            if last {
                break;
            }
        }

        self.pending.clear();
    }
}

impl SynthModule for SVMSSynth {
    type Config = SVMSConfig;

    fn reload_config(&mut self, config: &SVMSConfig) -> bool {
        // Every setting is fixed when the session is created
        self.config == config.clamped()
    }

    fn set_soundfonts(&mut self, handles: &[SoundFontHandle]) -> Result<(), RendererError> {
        self.destroy_session();

        let mut fonts: Vec<_> = handles
            .iter()
            .filter_map(|h| self.lib.get_soundfont(h))
            .take(SVMS_MAX_SOUNDFONT_STACK)
            .collect();
        if self.lib.api.set_offline_soundfont_entries.is_none() {
            fonts.truncate(1);
        }
        if fonts.is_empty() {
            return Ok(());
        }

        self.session = Some(self.create_session(&fonts)?);
        Ok(())
    }

    fn reset(&mut self) {
        self.event_buf.clear();
        if let Some(session) = self.session {
            unsafe { (self.lib.api.reset)(session) };
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

        let Some(session) = self.session else {
            while let Some(event) = self.event_buf.pop_before(end, base) {
                sysex::release(&event.event);
            }
            return;
        };

        prepapre_cache_vec(&mut self.left, frames, 0.0);
        prepapre_cache_vec(&mut self.right, frames, 0.0);

        let mut rendered = 0;
        while let Some(event) = self.event_buf.pop_before(end, base) {
            let offset = ((event.pos.wrapping_sub(base) as i32).max(0) as usize / channels)
                .clamp(rendered, frames);

            match event.event {
                // Not part of the offline message set, so the reset splits
                // the render to land on its exact frame.
                MaestroEvent::SystemReset => {
                    self.render_span(session, rendered, offset);
                    rendered = offset;
                    unsafe { (self.lib.api.reset)(session) };
                }
                // The offline sessions take channel messages only
                MaestroEvent::SystemExclusive { .. } => {}
                ev => {
                    if let Some(packed_message) = pack(ev) {
                        self.pending.push(SVMSOfflineEvent {
                            frame_offset: offset as u32,
                            packed_message,
                            reserved: [0; 2],
                        });
                    }
                }
            }

            sysex::release(&event.event);
        }

        self.render_span(session, rendered, frames);

        match self.channels {
            ChannelCount::Stereo => {
                for ((out, l), r) in buffer
                    .as_chunks_mut::<2>()
                    .0
                    .iter_mut()
                    .zip(&self.left)
                    .zip(&self.right)
                {
                    out[0] = *l;
                    out[1] = *r;
                }
            }
            ChannelCount::Mono => {
                for ((out, l), r) in buffer.iter_mut().zip(&self.left).zip(&self.right) {
                    *out = (l + r) * 0.5;
                }
            }
        }
    }

    fn voice_count(&self) -> u64 {
        let Some(session) = self.session else {
            return 0;
        };

        let mut telemetry = SVMSOfflineTelemetry {
            struct_size: size_of::<SVMSOfflineTelemetry>() as u32,
            struct_version: SVMS_STRUCT_VERSION_1,
            ..Default::default()
        };
        match unsafe { (self.lib.api.get_offline_telemetry)(session, &mut telemetry) } {
            SVMS_RESULT_OK => telemetry.active_voices as u64,
            _ => 0,
        }
    }
}

impl Drop for SVMSSynth {
    fn drop(&mut self) {
        self.destroy_session();
    }
}
