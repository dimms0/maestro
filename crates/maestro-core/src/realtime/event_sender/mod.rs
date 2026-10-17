use std::sync::{Arc, MutexGuard};

use crate::{
    error::RealtimeEngineError,
    event::{MaestroEvent, MaestroTimedEvent, MidiTranslator},
    realtime::clock::RealtimeClock,
    realtime::{
        MaestroRealtimeStatistics, RealtimeEngineOptions,
        config::RealtimeConfig,
        event_sender::{
            coalescer::EventCoalescer,
            nps::{NpsBatch, NpsTracker},
        },
        load::LoadLimiter,
    },
    renderer::sender::{PortEventSender, PortQueue},
};

mod coalescer;
mod nps;

pub use crate::event::ump::UMP_GROUPS;

impl Clone for RealtimeEventSender {
    fn clone(&self) -> Self {
        Self {
            ports: self.ports.clone(),
            nps: self.nps.clone(),
            coalescer: self.coalescer.clone(),
            clock: self.clock.clone(),
            translator: MidiTranslator::new(),
        }
    }
}

pub struct RealtimeEventSender {
    ports: Box<[Arc<PortEventSender>]>,

    nps: NpsTracker,
    coalescer: EventCoalescer,

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

        let coalescer = EventCoalescer::new(
            port_senders.clone(),
            clock.clone(),
            options.config.coalesce_window_ms,
            nps.event_counter(),
        )
        .map_err(RealtimeEngineError::Thread)?;

        Ok(Self {
            ports: port_senders,
            nps,
            coalescer,
            clock,
            translator: MidiTranslator::new(),
        })
    }

    pub(super) fn reload_config(&self, config: &RealtimeConfig) {
        self.clock.set_auto_timestamp(config.auto_timestamp);
        self.nps.set_limit(config.max_nps);
        self.coalescer.set_window(config.coalesce_window_ms);
    }

    pub(super) fn load_limiter(&self) -> Arc<LoadLimiter> {
        self.nps.limiter()
    }

    #[inline]
    fn admit(&self, port: u8, event: MaestroEvent) -> bool {
        let tracked = |channel: u8| port as usize * 16 + channel as usize;

        match event {
            MaestroEvent::NoteOn { channel, key, vel } => {
                self.nps.note_on(tracked(channel), key, vel)
            }
            MaestroEvent::NoteOff { channel, key } => self.nps.note_off(tracked(channel), key),
            _ => true,
        }
    }

    #[inline]
    fn push(&self, port: u8, event: MaestroEvent, at: u64) {
        let Some(sender) = self.ports.get(port as usize) else {
            return;
        };

        if !self.admit(port, event) || self.coalescer.record(port, event) {
            return;
        }

        self.nps.count_event();

        // Timestamp only if it passes the filters to reduce cycles per send
        sender.send(MaestroTimedEvent {
            event,
            pos: self.clock.stamp(at),
        });
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
        let pos = self.clock.stamp(0);
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
        self.coalescer.reset();

        self.translator.reset();
        self.nps.reset();
    }
}

struct Batch<'a> {
    sender: &'a RealtimeEventSender,
    at: u64,
    pos: Option<u32>,
    nps: NpsBatch,
    queue: Option<(u8, MutexGuard<'a, PortQueue>)>,
}

impl<'a> Batch<'a> {
    fn new(sender: &'a RealtimeEventSender, at: u64) -> Self {
        Self {
            sender,
            at,
            pos: None,
            nps: sender.nps.batch(),
            queue: None,
        }
    }

    #[inline]
    fn push(&mut self, port: u8, event: MaestroEvent) {
        let sender = self.sender;
        let Some(port_sender) = sender.ports.get(port as usize) else {
            return;
        };

        let tracked = |channel: u8| port as usize * 16 + channel as usize;
        let admitted = match event {
            MaestroEvent::NoteOn { channel, key, vel } => {
                sender
                    .nps
                    .batch_note_on(&mut self.nps, tracked(channel), key, vel)
            }
            MaestroEvent::NoteOff { channel, key } => sender.nps.note_off(tracked(channel), key),
            _ => true,
        };
        if !admitted || sender.coalescer.record(port, event) {
            return;
        }

        self.nps.events += 1;
        let pos = *self.pos.get_or_insert_with(|| sender.clock.stamp(self.at));

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
