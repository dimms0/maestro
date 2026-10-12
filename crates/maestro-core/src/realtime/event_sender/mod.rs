use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use crate::{
    error::RealtimeEngineError,
    event::{MaestroEvent, MaestroTimedEvent, MidiTranslator},
    realtime::clock::RealtimeClock,
    realtime::{
        MaestroRealtimeStatistics, RealtimeEngineOptions,
        config::RealtimeConfig,
        event_sender::{coalescer::EventCoalescer, nps::NpsTracker},
        load::LoadLimiter,
    },
    renderer::sender::PortEventSender,
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
            precision: self.precision.clone(),
            clock: self.clock.clone(),
            translator: MidiTranslator::new(),
            last_timed: Mutex::new(None),
        }
    }
}

pub struct RealtimeEventSender {
    ports: Box<[Arc<PortEventSender>]>,

    nps: NpsTracker,
    coalescer: EventCoalescer,
    precision: Arc<AtomicBool>,

    clock: Arc<RealtimeClock>,
    translator: MidiTranslator,
    last_timed: Mutex<Option<(Instant, u64)>>,
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

        let precision = Arc::new(AtomicBool::new(options.config.precision_playback));

        let coalescer = EventCoalescer::new(
            port_senders.clone(),
            clock.clone(),
            precision.clone(),
            options.config.coalesce_window_ms,
            nps.event_counter(),
        )
        .map_err(RealtimeEngineError::Thread)?;

        Ok(Self {
            ports: port_senders,
            nps,
            coalescer,
            precision,
            clock,
            translator: MidiTranslator::new(),
            last_timed: Mutex::new(None),
        })
    }

    pub(super) fn reload_config(&self, config: &RealtimeConfig) {
        self.precision
            .store(config.precision_playback, Ordering::Relaxed);
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
    fn push(&self, port: u8, event: MaestroEvent, at: Option<Instant>) {
        let Some(sender) = self.ports.get(port as usize) else {
            return;
        };

        // Timestamped events keep the timing they were given, so the coalescer
        // has nothing to merge them into
        if !self.admit(port, event) || (at.is_none() && self.coalescer.record(port, event)) {
            return;
        }

        self.nps.count_event();

        // Timestamp only if it passes the filters to reduce cycles per send
        let pos = match at {
            Some(at) => self.timed_stamp(at),
            None => self.stamp(),
        };
        sender.send(MaestroTimedEvent { event, pos });
    }

    #[inline]
    fn stamp(&self) -> u32 {
        if self.precision.load(Ordering::Relaxed) {
            self.clock.get_position() as u32
        } else {
            self.clock.unstamped_position()
        }
    }

    fn timed_stamp(&self, at: Instant) -> u32 {
        let pos = self.clock.position_for(at);

        // A correction of the clock between two stamps can rewind the timeline
        // under them, and the ports order what they hold by position, so a note
        // off sent right after its note on could end up before it and leave the
        // note playing. Latching to the last position keeps a source that sends
        // in time order in that order. An earlier timestamp is taken as it is: it
        // comes from a source that does not send in time order, and dragging it
        // forward would only add latency.
        let mut last = self.last_timed.lock().unwrap();
        let pos = match *last {
            Some((last_at, last_pos)) if at >= last_at => pos.max(last_pos),
            _ => pos,
        };
        *last = Some((at, pos));

        pos as u32
    }

    pub fn process_short(&self, port: u8, short: u32, at: Option<Instant>) {
        self.translator.short(port, short, |event| {
            self.push(port, event, at);
        });
    }

    pub fn process_long(&self, port: u8, long: &[u8], at: Option<Instant>) {
        self.translator.long(port, long, |event| {
            self.push(port, event, at);
        });
    }

    pub fn process_ump(&self, words: &[u32], at: Option<Instant>) {
        self.translator.ump(words, |group, event| {
            self.push(group, event, at);
        });
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
