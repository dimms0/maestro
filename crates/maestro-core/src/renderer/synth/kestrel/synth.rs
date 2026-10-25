use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use kestrel::{
    Backend, Driver, EnvelopeCurve, Interpolation, StealRule,
    config::AdmitRule,
    gpu::GpuSynth,
    midi::{Event, sysex_to_events},
};

use crate::{
    audio_params::{AudioParameters, ChannelCount},
    error::RendererError,
    event::{MaestroEvent, MaestroTimedEvent, sysex},
    helpers::prepapre_cache_vec,
    renderer::{
        config::kestrel::{
            KestrelAdmitRule, KestrelConfig, KestrelEnvelopeCurve, KestrelInterpolation,
            KestrelStealRule,
        },
        synth::{
            EventBuffer, SoundFontHandle, SynthModule,
            kestrel::{
                gpu,
                library::{KestrelBank, KestrelLib},
            },
        },
    },
};

pub(super) fn base_config(c: &KestrelConfig, sample_rate: u32) -> kestrel::Config {
    let curve = |c: KestrelEnvelopeCurve| match c {
        KestrelEnvelopeCurve::Exponential => EnvelopeCurve::Exponential,
        KestrelEnvelopeCurve::Linear => EnvelopeCurve::Linear,
    };

    kestrel::Config {
        sample_rate,
        block_frames: c.block_frames,
        gate_frames: c.gate_frames,
        workgroup_size: c.workgroup_size,
        max_render_workgroups: c.render_workgroups,
        submit_voices: c.submit_voices,
        max_voices: c.voice_limit,
        max_layers: c.max_layers,
        steal_rule: match c.steal_rule {
            KestrelStealRule::Quietest => StealRule::Quietest,
            KestrelStealRule::Oldest => StealRule::Oldest,
            KestrelStealRule::DropNew => StealRule::DropNew,
        },
        admit_rule: match c.admit_rule {
            KestrelAdmitRule::Loudest => AdmitRule::Loudest,
            KestrelAdmitRule::Even => AdmitRule::Even,
        },
        note_grid: c.note_grid,
        materialise_threads: c.build_threads as usize,
        max_steal_percent: c.max_steal_percent,
        steal_fade_frames: c.steal_fade_frames,
        sort_voices: c.sort_voices,
        interpolation: match c.interpolation {
            KestrelInterpolation::Nearest => Interpolation::Nearest,
            KestrelInterpolation::Linear => Interpolation::Linear,
            KestrelInterpolation::Cubic => Interpolation::Cubic,
        },
        decay_curve: curve(c.decay_curve),
        release_curve: curve(c.release_curve),
        env_floor: 10f32.powf(c.release_floor_db / 20.0),
        filter_enabled: c.filter,
        max_param_variants: c.max_param_variants,
        resample_pool: c.resample_pool,
        gain_ramp: c.gain_ramp,
        filter_ramp: c.filter_ramp,
        lfo_enabled: c.lfo,
        mod_env_enabled: c.mod_envelope,
        kahan_reduce: c.kahan_reduce,
        unchecked_shaders: c.unchecked_shaders,
        limiter: false,
        clamp_output: false,
        dc_blocker: false,
        skip_silence: true,
        nan_guard: false,
        ..Default::default()
    }
}

fn engine_config(c: &KestrelConfig, sample_rate: u32) -> kestrel::Config {
    let mut cfg = base_config(c, sample_rate);
    if let Some(adapter) = gpu::resolve(c.gpu, &cfg) {
        cfg.gpu_adapter = Some(adapter.name);
        cfg.gpu_backend = Some(gpu::backend_word(adapter.gpu.backend).to_string());
        cfg.max_voices = cfg.max_voices.min(adapter.max_voices.max(1));
    }
    cfg.sample_pool_budget = match c.pool_budget_mib {
        0 => kestrel::gpu::auto_pool_budget(&cfg),
        mib => (mib as u64) << 20,
    };
    cfg
}

fn translate(event: MaestroEvent, scratch: &mut Vec<Event>, mut emit: impl FnMut(Event)) {
    let ev = match event {
        MaestroEvent::NoteOn { channel, key, vel } if vel & 0x7F != 0 => Event::NoteOn {
            ch: channel,
            key: key,
            vel: vel,
        },
        MaestroEvent::NoteOn { channel, key, .. } | MaestroEvent::NoteOff { channel, key } => {
            Event::NoteOff {
                ch: channel,
                key: key,
            }
        }
        MaestroEvent::ControlChange {
            channel,
            param,
            val,
        } => Event::Cc {
            ch: channel,
            num: param,
            val: val,
        },
        MaestroEvent::ProgramChange { channel, program } => Event::Program {
            ch: channel,
            val: program,
        },
        MaestroEvent::PitchBendChange { channel, lsb, msb } => Event::PitchBend {
            ch: channel,
            val: ((msb as i16) << 7 | lsb as i16) - 8192,
        },
        MaestroEvent::PolyphonicAftertouch { .. } | MaestroEvent::ChannelAftertouch { .. } => {
            return;
        }
        MaestroEvent::SystemReset => {
            emit(Event::ResetParts);
            // just in case, untested if only the above is enough
            for ch in 0..16 {
                for num in [120, 121] {
                    emit(Event::Cc { ch, num, val: 0 });
                }
            }
            return;
        }
        MaestroEvent::SystemExclusive { id } => {
            scratch.clear();
            sysex::with(id, |data| sysex_to_events(data, 0, scratch));
            scratch
                .drain(..)
                .filter(|ev| *ev != Event::Other)
                .for_each(emit);
            return;
        }
    };

    emit(ev);
}

fn live_driver(cfg: &kestrel::Config, bank: &KestrelBank) -> Result<Driver, RendererError> {
    Driver::live(cfg, bank.bank.clone())
        .map_err(|e| RendererError::SynthInit(format!("Kestrel: {e:#}")))
}

struct Engine {
    synth: GpuSynth,
    driver: Driver,
    bank: Arc<KestrelBank>,
}

pub(crate) struct KestrelSynth {
    lib: Arc<KestrelLib>,
    // Boxed, the driver's per-channel tables are large
    engine: Option<Box<Engine>>,

    config: KestrelConfig,
    engine_cfg: Box<kestrel::Config>,
    channels: ChannelCount,

    event_buf: EventBuffer,
    sysex_events: Vec<Event>,
    last_pos: u32,

    // Frames handed out so far, and the one the driver's frame 0 lines up with
    frames_out: u64,
    origin: u64,
    // How far the output trails the driver: nothing while every read covers
    // whole blocks, one block once a read ends inside one
    lag: u64,
    // Rendered stereo audio not handed out yet
    pending: Vec<f32>,
    block: Vec<f32>,
}

impl KestrelSynth {
    pub fn new(
        lib: Arc<KestrelLib>,
        config: &KestrelConfig,
        audio_params: &AudioParameters,
    ) -> Result<Self, RendererError> {
        let config = config.clamped();
        let engine_cfg = engine_config(&config, audio_params.sample_rate);
        engine_cfg
            .validate()
            .map_err(|e| RendererError::SynthInit(format!("Kestrel: {e:#}")))?;

        Ok(Self {
            lib,
            engine: None,
            config,
            engine_cfg: Box::new(engine_cfg),
            channels: audio_params.channels,
            event_buf: EventBuffer::new(),
            sysex_events: Vec::new(),
            last_pos: 0,
            frames_out: 0,
            origin: 0,
            lag: 0,
            pending: Vec::new(),
            block: Vec::new(),
        })
    }

    fn restart(&mut self) {
        self.origin = self.frames_out;
        self.lag = 0;
        self.pending.clear();
    }
}

impl SynthModule for KestrelSynth {
    type Config = KestrelConfig;

    fn reload_config(&mut self, config: &KestrelConfig) -> bool {
        // Every setting is fixed when the GPU buffers are made
        self.config == config.clamped()
    }

    fn set_soundfonts(&mut self, handles: &[SoundFontHandle]) -> Result<(), RendererError> {
        let fonts: Vec<_> = handles
            .iter()
            .rev()
            .filter_map(|h| self.lib.get_soundfont(h))
            .collect();
        if fonts.is_empty() {
            self.engine = None;
            return Ok(());
        }

        let bank = self.lib.bank(&fonts, &self.config, &self.engine_cfg)?;
        let synth = self.lib.synth(&bank, &self.engine_cfg)?;
        let driver = live_driver(&self.engine_cfg, &bank)?;
        self.engine = Some(Box::new(Engine {
            synth,
            driver,
            bank,
        }));
        self.restart();

        Ok(())
    }

    fn reset(&mut self) {
        self.event_buf.clear();
        if let Some(engine) = &mut self.engine {
            engine.synth.reset();
            // Only fails on a config that already made the first one
            if let Ok(driver) = live_driver(&self.engine_cfg, &engine.bank) {
                engine.driver = driver;
            }
        }
        self.restart();
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
        let start = self.frames_out;
        self.frames_out += frames as u64;

        let Some(engine) = &mut self.engine else {
            while let Some(event) = self.event_buf.pop_before(end, base) {
                sysex::release(&event.event);
            }
            self.pending.clear();
            return;
        };

        while let Some(event) = self.event_buf.pop_before(end, base) {
            let offset =
                ((event.pos.wrapping_sub(base) as i32).max(0) as usize / channels).min(frames);
            let frame = (start + offset as u64).saturating_sub(self.origin);
            translate(event.event, &mut self.sysex_events, |ev| {
                engine.driver.push(frame, ev)
            });
            sysex::release(&event.event);
        }

        // every event before this driver frame is in, so a block ending by it can render
        let known = (start + frames as u64).saturating_sub(self.origin);
        let block_frames = self.engine_cfg.block_frames as u64;
        let block_len = self.engine_cfg.block_samples();
        let mut failed = false;

        while self.pending.len() / 2 < frames {
            if engine.driver.next_frame() + block_frames > known {
                if self.lag != 0 {
                    break;
                }
                // This read ends inside a block, so from here on the output
                // trails by one block to keep every event on its frame
                self.lag = block_frames;
                self.pending
                    .splice(0..0, std::iter::repeat_n(0.0, block_len));
                continue;
            }

            prepapre_cache_vec(&mut self.block, block_len, 0.0);
            let rendered = catch_unwind(AssertUnwindSafe(|| {
                engine
                    .driver
                    .render_live(&mut engine.synth, &mut self.block)
            }));
            match rendered {
                Ok(Ok(())) => self.pending.extend_from_slice(&self.block),
                Ok(Err(e)) => {
                    eprintln!("Kestrel stopped rendering: {e:#}");
                    failed = true;
                    break;
                }
                Err(_) => {
                    eprintln!("Kestrel stopped rendering after a GPU error");
                    failed = true;
                    break;
                }
            }
        }

        let take = frames.min(self.pending.len() / 2);
        let rendered = &self.pending[..take * 2];
        match self.channels {
            ChannelCount::Stereo => buffer[..take * 2].copy_from_slice(rendered),
            ChannelCount::Mono => {
                for (out, lr) in buffer.iter_mut().zip(rendered.as_chunks::<2>().0) {
                    *out = (lr[0] + lr[1]) * 0.5;
                }
            }
        }
        self.pending.drain(..take * 2);

        if failed {
            self.engine = None;
        }
    }

    fn voice_count(&self) -> u64 {
        self.engine
            .as_ref()
            .map_or(0, |e| e.synth.stats().active_voices)
    }
}
