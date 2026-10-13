use std::{
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle, Thread},
    time::Duration,
};

use crate::{
    event::{MaestroEvent, MaestroTimedEvent},
    realtime::clock::RealtimeClock,
    renderer::sender::PortEventSender,
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct RpnAddress {
    is_nrpn: bool,
    msb: u8,
    lsb: u8,
}

impl RpnAddress {
    /// (127, 127) is the MIDI convention for "no parameter selected"
    fn is_null(self) -> bool {
        self.msb == 127 && self.lsb == 127
    }
}

struct RpnEntry {
    address: RpnAddress,
    value_msb: u8,
    value_lsb: u8,
    dirty: bool,
    touched_at: u64,
}

const MAX_RPN_ENTRIES_PER_CHANNEL: usize = 64;

struct RpnBoard {
    current: RpnAddress,
    entries: Vec<RpnEntry>,
    clock: u64,
}

impl RpnBoard {
    fn new() -> Self {
        Self {
            current: RpnAddress {
                is_nrpn: false,
                msb: 0,
                lsb: 0,
            },
            entries: Vec::new(),
            clock: 0,
        }
    }

    fn select_msb(&mut self, is_nrpn: bool, msb: u8) {
        self.current.is_nrpn = is_nrpn;
        self.current.msb = msb;
    }

    fn select_lsb(&mut self, is_nrpn: bool, lsb: u8) {
        self.current.is_nrpn = is_nrpn;
        self.current.lsb = lsb;
    }

    fn current_entry(&mut self) -> Option<&mut RpnEntry> {
        let address = self.current;
        if address.is_null() {
            return None;
        }

        self.clock += 1;
        let clock = self.clock;

        if let Some(idx) = self.entries.iter().position(|e| e.address == address) {
            let entry = &mut self.entries[idx];
            entry.touched_at = clock;
            return Some(entry);
        }

        if self.entries.len() < MAX_RPN_ENTRIES_PER_CHANNEL {
            self.entries.push(RpnEntry {
                address,
                value_msb: 0,
                value_lsb: 0,
                dirty: false,
                touched_at: clock,
            });
            return self.entries.last_mut();
        }

        let evict = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.dirty)
            .min_by_key(|(_, e)| e.touched_at)
            .or_else(|| {
                self.entries
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, e)| e.touched_at)
            })
            .map(|(i, _)| i)?;

        self.entries[evict] = RpnEntry {
            address,
            value_msb: 0,
            value_lsb: 0,
            dirty: false,
            touched_at: clock,
        };
        Some(&mut self.entries[evict])
    }

    fn write_msb(&mut self, val: u8) {
        if let Some(entry) = self.current_entry() {
            entry.value_msb = val;
            entry.dirty = true;
        }
    }

    fn write_lsb(&mut self, val: u8) {
        if let Some(entry) = self.current_entry() {
            entry.value_lsb = val;
            entry.dirty = true;
        }
    }

    fn step(&mut self, delta: i32) {
        if let Some(entry) = self.current_entry() {
            let combined = ((entry.value_msb as i32) << 7) | entry.value_lsb as i32;
            let stepped = (combined + delta).clamp(0, 0x3FFF);
            entry.value_msb = (stepped >> 7) as u8;
            entry.value_lsb = (stepped & 0x7F) as u8;
            entry.dirty = true;
        }
    }

    fn drain_into(&mut self, channel: u8, out: &mut Vec<MaestroEvent>) {
        let mut dirty: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.dirty)
            .map(|(i, _)| i)
            .collect();
        dirty.sort_unstable_by_key(|&i| self.entries[i].touched_at);

        for i in dirty {
            let entry = &mut self.entries[i];
            let (select_msb, select_lsb) = if entry.address.is_nrpn {
                (99, 98)
            } else {
                (101, 100)
            };

            let mut push = |param: u8, val: u8| {
                out.push(MaestroEvent::ControlChange {
                    channel,
                    param,
                    val,
                });
            };
            push(select_msb, entry.address.msb);
            push(select_lsb, entry.address.lsb);
            push(6, entry.value_msb);
            push(38, entry.value_lsb);

            entry.dirty = false;
        }
    }
}

struct PortState {
    cc: [[Option<(MaestroEvent, bool)>; 128]; 16],
    pitch: [Option<MaestroEvent>; 16],
    program: [Option<MaestroEvent>; 16],
    channel_pressure: [Option<MaestroEvent>; 16],
    poly_pressure: [[Option<MaestroEvent>; 128]; 16],
    rpn: [RpnBoard; 16],
}

impl PortState {
    fn new() -> Self {
        Self {
            cc: [[None; 128]; 16],
            pitch: [None; 16],
            program: [None; 16],
            channel_pressure: [None; 16],
            poly_pressure: [[None; 128]; 16],
            rpn: std::array::from_fn(|_| RpnBoard::new()),
        }
    }

    fn record(&mut self, event: MaestroEvent) -> bool {
        match event {
            MaestroEvent::ControlChange {
                channel,
                param,
                val,
            } => {
                let ch = channel as usize;
                match param {
                    101 => self.rpn[ch].select_msb(false, val),
                    100 => self.rpn[ch].select_lsb(false, val),
                    99 => self.rpn[ch].select_msb(true, val),
                    98 => self.rpn[ch].select_lsb(true, val),
                    6 => self.rpn[ch].write_msb(val),
                    38 => self.rpn[ch].write_lsb(val),
                    96 => self.rpn[ch].step(1),
                    97 => self.rpn[ch].step(-1),
                    _ => {
                        let released = (param == 64 || param == 66) && val < 64;
                        match &mut self.cc[ch][param as usize] {
                            Some((ev, saw_release)) => {
                                *saw_release |= released;
                                *ev = event;
                            }
                            slot => *slot = Some((event, released)),
                        }
                    }
                }
                true
            }
            MaestroEvent::PitchBendChange { channel, .. } => {
                self.pitch[channel as usize] = Some(event);
                true
            }
            MaestroEvent::ProgramChange { channel, .. } => {
                self.program[channel as usize] = Some(event);
                true
            }
            MaestroEvent::ChannelAftertouch { channel, .. } => {
                self.channel_pressure[channel as usize] = Some(event);
                true
            }
            MaestroEvent::PolyphonicAftertouch { channel, key, .. } => {
                self.poly_pressure[channel as usize][key as usize] = Some(event);
                true
            }
            _ => false,
        }
    }

    fn drain_into(&mut self, out: &mut Vec<MaestroEvent>) {
        for channel in 0..16 {
            self.rpn[channel].drain_into(channel as u8, out);

            for slot in &mut self.cc[channel] {
                if let Some((ev, saw_release)) = slot.take() {
                    if saw_release
                        && let MaestroEvent::ControlChange {
                            channel,
                            param,
                            val,
                        } = ev
                        && val >= 64
                    {
                        out.push(MaestroEvent::ControlChange {
                            channel,
                            param,
                            val: 0,
                        });
                    }
                    out.push(ev);
                }
            }
            if let Some(ev) = self.pitch[channel].take() {
                out.push(ev);
            }
            if let Some(ev) = self.program[channel].take() {
                out.push(ev);
            }
            if let Some(ev) = self.channel_pressure[channel].take() {
                out.push(ev);
            }
            for slot in &mut self.poly_pressure[channel] {
                if let Some(ev) = slot.take() {
                    out.push(ev);
                }
            }
        }
    }
}

struct Shared {
    senders: Box<[Arc<PortEventSender>]>,
    ports: Box<[Mutex<PortState>]>,
    clock: Arc<RealtimeClock>,
    event_counter: Arc<AtomicU64>,
    window_ms: AtomicU32, // 0 while coalescing is off
    stop: AtomicBool,
}

impl Shared {
    fn flush(&self, pending: &mut Vec<MaestroEvent>) {
        let pos = self.clock.stamp(None);

        for (sender, state) in self.senders.iter().zip(self.ports.iter()) {
            let mut state = state.lock().unwrap();
            state.drain_into(pending);
            self.event_counter
                .fetch_add(pending.len() as u64, Ordering::Relaxed);
            for event in pending.drain(..) {
                sender.send(MaestroTimedEvent { event, pos });
            }
        }
    }
}

pub(super) struct EventCoalescer {
    shared: Arc<Shared>,
    thread: Thread,
    join_handle: Option<JoinHandle<()>>,
}

impl EventCoalescer {
    pub fn new(
        senders: Box<[Arc<PortEventSender>]>,
        clock: Arc<RealtimeClock>,
        window_ms: Option<u32>,
        event_counter: Arc<AtomicU64>,
    ) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            ports: senders
                .iter()
                .map(|_| Mutex::new(PortState::new()))
                .collect(),
            senders,
            clock,
            event_counter,
            window_ms: AtomicU32::new(window(window_ms)),
            stop: AtomicBool::new(false),
        });

        let join_handle = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("event_coalescer".to_string())
                .spawn(move || {
                    let mut pending = Vec::new();
                    loop {
                        match shared.window_ms.load(Ordering::Acquire) {
                            0 => thread::park(),
                            ms => thread::park_timeout(Duration::from_millis(ms as u64)),
                        }

                        shared.flush(&mut pending);
                        if shared.stop.load(Ordering::Acquire) {
                            return;
                        }
                    }
                })?
        };

        Ok(Self {
            shared,
            thread: join_handle.thread().clone(),
            join_handle: Some(join_handle),
        })
    }

    pub fn set_window(&self, window_ms: Option<u32>) {
        let ms = window(window_ms);
        if self.shared.window_ms.swap(ms, Ordering::AcqRel) == ms {
            return;
        }

        // The thread parks for good once off, so whatever is pending goes out now
        if ms == 0 {
            self.shared.flush(&mut Vec::new());
        }
        self.thread.unpark();
    }

    pub fn record(&self, port: u8, event: MaestroEvent) -> bool {
        let off = || self.shared.window_ms.load(Ordering::Relaxed) == 0;
        if off() {
            return false;
        }

        // Checked again under the lock: once the flush in `set_window` has
        // taken it, nothing may be recorded into a parked coalescer
        let mut state = self.shared.ports[port as usize].lock().unwrap();
        !off() && state.record(event)
    }

    pub fn reset(&self) {
        for port in self.shared.ports.iter() {
            *port.lock().unwrap() = PortState::new();
        }
    }
}

fn window(window_ms: Option<u32>) -> u32 {
    window_ms.map_or(0, |ms| ms.max(1))
}

impl Clone for EventCoalescer {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            thread: self.thread.clone(),
            join_handle: None,
        }
    }
}

impl Drop for EventCoalescer {
    fn drop(&mut self) {
        if let Some(handle) = self.join_handle.take() {
            self.shared.stop.store(true, Ordering::Release);
            handle.thread().unpark();
            handle.join().ok();
        }
    }
}
