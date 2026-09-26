use midi_parser::Division;
use std::sync::{Arc, Mutex};

use crate::{
    audio_params::AudioParameters,
    error::RealtimeEngineError,
    event::{MaestroEvent, MaestroTimedEvent, MidiTranslator},
    realtime::{
        MaestroRealtimeStatistics, RealtimeEngineOptions,
        event_sender::{coalescer::EventCoalescer, nps::NpsTracker},
        load::LoadLimiter,
    },
    renderer::{RealtimeClock, sender::PortEventSender},
    tempo::TempoClock,
};

mod coalescer;
mod nps;

pub use crate::event::ump::UMP_GROUPS;

struct TickState {
    tempo: TempoClock,
    pos: u64,
    anchor: Option<u64>,
}

impl TickState {
    fn new(sample_rate: u32) -> Self {
        Self {
            tempo: TempoClock::new(Division::Ppq(96), sample_rate),
            pos: 0,
            anchor: None,
        }
    }
}

impl Clone for RealtimeEventSender {
    fn clone(&self) -> Self {
        Self {
            ports: self.ports.clone(),
            nps: self.nps.clone(),
            coalescer: self.coalescer.clone(),
            precision: self.precision,
            clock: self.clock.clone(),
            translator: MidiTranslator::new(),
            tick: Mutex::new(TickState::new(self.sample_rate)),
            sample_rate: self.sample_rate,
            channels: self.channels,
        }
    }
}

pub struct RealtimeEventSender {
    ports: Box<[Arc<PortEventSender>]>,

    nps: NpsTracker,
    coalescer: Option<EventCoalescer>,
    precision: bool,

    clock: Arc<RealtimeClock>,
    translator: MidiTranslator,
    tick: Mutex<TickState>,
    sample_rate: u32,
    channels: u64,
}

impl RealtimeEventSender {
    pub(super) fn new(
        options: &RealtimeEngineOptions,
        params: &AudioParameters,
        clock: Arc<RealtimeClock>,
        port_senders: Box<[Arc<PortEventSender>]>,
        stats: MaestroRealtimeStatistics,
    ) -> Result<Self, RealtimeEngineError> {
        let ports = options.ports.unwrap_or(1);

        let nps = NpsTracker::new(ports as usize * 16, options.config.max_nps, stats)
            .map_err(RealtimeEngineError::Thread)?;

        let sample_rate = params.sample_rate;

        let coalescer = options.config.coalesce_window_ms.map(|ms| {
            EventCoalescer::new(
                port_senders.clone(),
                clock.clone(),
                options.config.precision_playback,
                ms,
                nps.event_counter(),
            )
        });

        Ok(Self {
            ports: port_senders,
            nps,
            coalescer,
            precision: options.config.precision_playback,
            clock,
            translator: MidiTranslator::new(),
            tick: Mutex::new(TickState::new(sample_rate)),
            sample_rate,
            channels: u16::from(params.channels) as u64,
        })
    }

    pub(super) fn load_limiter(&self) -> Option<Arc<LoadLimiter>> {
        self.nps.limiter()
    }

    fn push(&self, port: u8, event: MaestroEvent, delta_ticks: Option<u64>) {
        // Do not filter timed events with wall-clock limiters
        if delta_ticks.is_none() {
            let tracked = |channel: u8| port as usize * 16 + channel as usize;

            match event {
                MaestroEvent::NoteOn { channel, key, vel }
                    if !self.nps.note_on(tracked(channel), key, vel) =>
                {
                    return;
                }
                MaestroEvent::NoteOff { channel, key }
                    if !self.nps.note_off(tracked(channel), key) =>
                {
                    return;
                }

                _ => {}
            }

            if let Some(coalescer) = &self.coalescer
                && coalescer.record(port, event)
            {
                return;
            }

            self.nps.count_event();
        }

        // Timestamp only if it passes the filters to reduce cycles per send
        let pos = if let Some(ticks) = delta_ticks {
            self.tick_stamp(ticks)
        } else {
            self.stamp()
        };

        self.ports[port as usize].send(MaestroTimedEvent { event, pos });
    }

    fn stamp(&self) -> u32 {
        if self.precision {
            self.clock.get_position() as u32
        } else {
            self.clock.unstamped_position()
        }
    }

    fn tick_stamp(&self, ticks: u64) -> u32 {
        let mut tick = self.tick.lock().unwrap();
        let anchor = *tick.anchor.get_or_insert_with(|| self.clock.get_position());

        let samples = tick.tempo.advance(ticks);
        tick.pos += samples;

        (anchor + tick.pos * self.channels) as u32
    }

    pub fn set_tick_division(&self, division: Division) {
        *self.tick.lock().unwrap() = TickState {
            tempo: TempoClock::new(division, self.sample_rate),
            pos: 0,
            anchor: None,
        };
    }

    pub fn set_tick_tempo(&self, micros_per_quarter: u32) {
        self.tick
            .lock()
            .unwrap()
            .tempo
            .set_tempo(micros_per_quarter);
    }

    pub fn advance_ticks(&self, ticks: u64) {
        self.tick_stamp(ticks);
    }

    pub fn process_short(&self, port: u8, short: u32, delta_ticks: Option<u64>) {
        self.translator.short(port, short, |event| {
            self.push(port, event, delta_ticks);
        });
    }

    pub fn process_long(&self, port: u8, long: &[u8], delta_ticks: Option<u64>) {
        self.translator.long(port, long, |event| {
            self.push(port, event, delta_ticks);
        });
    }

    pub fn process_ump(&self, words: &[u32], delta_ticks: Option<u64>) {
        self.translator.ump(words, |group, event| {
            self.push(group, event, delta_ticks);
        });
    }

    pub fn reset(&self) {
        for port in &self.ports {
            port.reset();
        }
        if let Some(coalescer) = &self.coalescer {
            coalescer.reset();
        }

        self.translator.reset();
        self.nps.reset();

        let mut tick = self.tick.lock().unwrap();
        tick.pos = 0;
        tick.anchor = None;
    }
}
