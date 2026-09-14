use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::{
    event::{MaestroEvent, MaestroTimedEvent},
    renderer::{RealtimeClock, sender::PortEventSender},
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
    cc: [[Option<MaestroEvent>; 128]; 16],
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
                    _ => self.cc[ch][param as usize] = Some(event),
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
                if let Some(ev) = slot.take() {
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

pub(super) struct EventCoalescer {
    ports: Arc<[Mutex<PortState>]>,
    stop: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl EventCoalescer {
    pub fn new(
        senders: Box<[Arc<PortEventSender>]>,
        clock: Arc<RealtimeClock>,
        precision: bool,
        window_ms: u32,
    ) -> Self {
        let ports: Arc<[Mutex<PortState>]> = senders
            .iter()
            .map(|_| Mutex::new(PortState::new()))
            .collect::<Vec<_>>()
            .into();
        let stop = Arc::new(AtomicBool::new(false));

        let join_handle = {
            let ports = ports.clone();
            let stop = stop.clone();
            thread::Builder::new()
                .name("event_coalescer".to_string())
                .spawn(move || {
                    let mut pending = Vec::new();
                    while !stop.load(Ordering::Acquire) {
                        thread::sleep(Duration::from_millis(window_ms as u64));
                        let pos = if precision {
                            clock.get_position() as u32
                        } else {
                            clock.unstamped_position()
                        };

                        for (sender, state) in senders.iter().zip(ports.iter()) {
                            state.lock().unwrap().drain_into(&mut pending);
                            for event in pending.drain(..) {
                                sender.send(MaestroTimedEvent { event, pos });
                            }
                        }
                    }
                })
                .ok()
        };

        Self {
            ports,
            stop,
            join_handle,
        }
    }

    pub fn record(&self, port: u8, event: MaestroEvent) -> bool {
        self.ports[port as usize].lock().unwrap().record(event)
    }

    pub fn reset(&self) {
        for port in self.ports.iter() {
            *port.lock().unwrap() = PortState::new();
        }
    }
}

impl Clone for EventCoalescer {
    fn clone(&self) -> Self {
        Self {
            ports: self.ports.clone(),
            stop: self.stop.clone(),
            join_handle: None,
        }
    }
}

impl Drop for EventCoalescer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.join_handle.take() {
            handle.join().ok();
        }
    }
}
