use std::{
    ops::Range,
    sync::{Arc, MutexGuard},
};

use crate::{
    error::RealtimeEngineError,
    event::{MaestroEvent, MaestroTimedEvent, MidiTranslator},
    realtime::clock::RealtimeClock,
    realtime::{
        MaestroRealtimeStatistics, RealtimeEngineOptions,
        config::RealtimeConfig,
        event_sender::nps::{NpsBatch, NpsTracker},
        host_time,
    },
    renderer::sender::{PortEventSender, PortQueue},
};

mod nps;

pub use crate::event::ump::UMP_GROUPS;

impl Clone for RealtimeEventSender {
    fn clone(&self) -> Self {
        Self {
            ports: self.ports.clone(),
            nps: self.nps.clone(),
            clock: self.clock.clone(),
            translator: MidiTranslator::new(),
        }
    }
}

pub struct RealtimeEventSender {
    ports: Box<[Arc<PortEventSender>]>,

    nps: NpsTracker,

    clock: Arc<RealtimeClock>,
    translator: MidiTranslator,
}

impl RealtimeEventSender {
    pub(super) fn new(
        options: &RealtimeEngineOptions,
        clock: Arc<RealtimeClock>,
        port_senders: Box<[Arc<PortEventSender>]>,
        stats: MaestroRealtimeStatistics,
    ) -> Result<Self, RealtimeEngineError> {
        let ports = options.ports.unwrap_or(1);

        let nps = NpsTracker::new(ports as usize * 16, options.config.max_nps, stats)
            .map_err(RealtimeEngineError::Thread)?;

        Ok(Self {
            ports: port_senders,
            nps,
            clock,
            translator: MidiTranslator::new(),
        })
    }

    pub(super) fn reload_config(&self, config: &RealtimeConfig) {
        self.clock.set_auto_timestamp(config.auto_timestamp);
        self.clock.set_skip_behind(config.skip_behind);
        self.nps.set_limit(config.max_nps);
    }

    #[inline]
    fn admit(&self, port: u8, event: MaestroEvent, now: u64, behind: bool) -> bool {
        let tracked = port as usize * 16;

        let counted = match event {
            MaestroEvent::NoteOn { channel, key, vel } => {
                return self
                    .nps
                    .note_on(tracked + channel as usize, key, vel, now, behind);
            }
            MaestroEvent::NoteOff { channel, key } => {
                self.nps.note_off(tracked + channel as usize, key)
            }
            event => {
                self.nps.clear(silences_channels(tracked, event));
                true
            }
        };
        if counted {
            self.nps.count_event();
        }
        counted
    }

    #[inline]
    fn push(&self, port: u8, event: MaestroEvent, at: u64) {
        let Some(sender) = self.ports.get(port as usize) else {
            return;
        };

        let now = host_time::now();
        let behind = self.clock.behind(now);
        if self.admit(port, event, now, behind) {
            let pos = self.clock.stamp(at, now, behind);
            sender.send(MaestroTimedEvent { event, pos });
        }
    }

    pub fn process_short(&self, port: u8, short: u32, at: u64) {
        self.translator.short(port, short, |event| {
            self.push(port, event, at);
        });
    }

    pub fn process_long(&self, port: u8, long: &[u8], at: u64) {
        let mut batch = Batch::new(self, at);
        self.translator
            .long(port, long, |event| batch.push(port, event));
        batch.send();
    }

    pub fn process_ump(&self, words: &[u32], at: u64) {
        let mut batch = Batch::new(self, at);
        self.translator
            .ump(words, |group, event| batch.push(group, event));
        batch.send();
    }

    pub fn notes_off(&self) {
        let now = host_time::now();
        let pos = self.clock.stamp(0, now, self.clock.behind(now));
        self.nps.clear(0..self.ports.len() * 16);

        for port in &self.ports {
            let mut queue = port.lock();
            for channel in 0..16 {
                for param in [120, 123] {
                    let event = MaestroEvent::ControlChange {
                        channel,
                        param,
                        val: 0,
                    };
                    queue.push(MaestroTimedEvent { event, pos });
                }
            }
        }
    }

    pub fn reset(&self) {
        for port in &self.ports {
            port.reset();
        }

        self.translator.reset();
        self.nps.reset();
    }
}

#[inline]
fn silences_channels(port_base: usize, event: MaestroEvent) -> Range<usize> {
    match event {
        MaestroEvent::ControlChange {
            channel,
            param: 120 | 123,
            ..
        } => port_base + channel as usize..port_base + channel as usize + 1,
        MaestroEvent::SystemReset => port_base..port_base + 16,
        _ => 0..0,
    }
}

struct Batch<'a> {
    sender: &'a RealtimeEventSender,
    at: u64,
    now: u64,
    behind: bool,
    pos: Option<u32>,
    nps: NpsBatch,
    queue: Option<(u8, MutexGuard<'a, PortQueue>)>,
}

impl<'a> Batch<'a> {
    fn new(sender: &'a RealtimeEventSender, at: u64) -> Self {
        let now = host_time::now();
        Self {
            sender,
            at,
            now,
            behind: sender.clock.behind(now),
            pos: None,
            nps: sender.nps.batch(now),
            queue: None,
        }
    }

    #[inline]
    fn push(&mut self, port: u8, event: MaestroEvent) {
        let sender = self.sender;
        let Some(port_sender) = sender.ports.get(port as usize) else {
            return;
        };

        let tracked = port as usize * 16;
        let admitted = match event {
            MaestroEvent::NoteOn { channel, key, vel } => sender.nps.batch_note_on(
                &mut self.nps,
                tracked + channel as usize,
                key,
                vel,
                self.behind,
            ),
            MaestroEvent::NoteOff { channel, key } => {
                sender.nps.note_off(tracked + channel as usize, key)
            }
            event => {
                sender.nps.clear(silences_channels(tracked, event));
                true
            }
        };
        if !admitted {
            return;
        }

        self.nps.events += 1;

        let (at, now, behind) = (self.at, self.now, self.behind);
        let pos = *self
            .pos
            .get_or_insert_with(|| sender.clock.stamp(at, now, behind));

        let queue = match &mut self.queue {
            Some((locked, queue)) if *locked == port => queue,
            slot => {
                *slot = None;
                &mut slot.insert((port, port_sender.lock())).1
            }
        };
        queue.push(MaestroTimedEvent { event, pos });
    }

    fn send(self) {
        self.sender.nps.commit(&self.nps);
    }
}
