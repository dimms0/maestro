use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arc_swap::{ArcSwapOption, Guard};
use maestro_core::realtime::RealtimeEventSender;

use crate::{log_info, log_warn, watcher::ServiceEvent};

const MAX_BUFFERED_EVENTS: usize = 4096;
const MAX_BUFFERED_BYTES: usize = 1 << 20;
const MAX_ENTRY_BYTES: usize = MAX_BUFFERED_BYTES / 4;
const BUFFER_IDLE_CAPACITY: usize = 256;
const QUIESCE_DEADLINE: Duration = Duration::from_millis(250);

const INLINE_BYTES: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateMode {
    Off,
    Idle,
    Starting,
    Live,
}

impl GateMode {
    fn code(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::Starting => 1,
            Self::Live => 2,
            Self::Idle => 3,
        }
    }
}

enum Msg<'a> {
    Short { port: u8, msg: u32 },
    Long { port: u8, data: &'a [u8] },
    Ump(&'a [u32]),
}

impl Msg<'_> {
    #[inline]
    fn send_to(&self, snd: &RealtimeEventSender) {
        match *self {
            Msg::Short { port, msg } => snd.process_short(port, msg, None),
            Msg::Long { port, data } => snd.process_long(port, data, None),
            Msg::Ump(words) => snd.process_ump(words, None),
        }
    }
}

enum GateEntry {
    Short {
        port: u8,
        msg: u32,
    },
    LongInline {
        port: u8,
        len: u8,
        bytes: [u8; INLINE_BYTES],
    },
    LongHeap {
        port: u8,
        data: Box<[u8]>,
    },
    UmpInline {
        len: u8,
        words: [u32; 4],
    },
    UmpHeap {
        words: Box<[u32]>,
    },
}

impl GateEntry {
    fn from(msg: Msg<'_>) -> Self {
        match msg {
            Msg::Short { port, msg } => Self::Short { port, msg },
            Msg::Long { port, data } if data.len() <= INLINE_BYTES => {
                let mut bytes = [0u8; INLINE_BYTES];
                bytes[..data.len()].copy_from_slice(data);
                Self::LongInline {
                    port,
                    len: data.len() as u8,
                    bytes,
                }
            }
            Msg::Long { port, data } => Self::LongHeap {
                port,
                data: data.into(),
            },
            // One UMP message is at most four words; CoreMIDI event packets
            // can carry several, hence the heap fallback.
            Msg::Ump(words) if words.len() <= 4 => {
                let mut buf = [0u32; 4];
                buf[..words.len()].copy_from_slice(words);
                Self::UmpInline {
                    len: words.len() as u8,
                    words: buf,
                }
            }
            Msg::Ump(words) => Self::UmpHeap {
                words: words.into(),
            },
        }
    }

    fn heap_bytes(&self) -> usize {
        match self {
            Self::LongHeap { data, .. } => data.len(),
            Self::UmpHeap { words } => words.len() * 4,
            _ => 0,
        }
    }

    fn replay(&self, snd: &RealtimeEventSender) {
        match self {
            Self::Short { port, msg } => snd.process_short(*port, *msg, None),
            Self::LongInline { port, len, bytes } => {
                snd.process_long(*port, &bytes[..*len as usize], None)
            }
            Self::LongHeap { port, data } => snd.process_long(*port, data, None),
            Self::UmpInline { len, words } => snd.process_ump(&words[..*len as usize], None),
            Self::UmpHeap { words } => snd.process_ump(words, None),
        }
    }

    #[cfg(test)]
    fn decode(&self) -> Decoded {
        match self {
            Self::Short { port, msg } => Decoded::Short(*port, *msg),
            Self::LongInline { port, len, bytes } => {
                Decoded::Long(*port, bytes[..*len as usize].to_vec())
            }
            Self::LongHeap { port, data } => Decoded::Long(*port, data.to_vec()),
            Self::UmpInline { len, words } => Decoded::Ump(words[..*len as usize].to_vec()),
            Self::UmpHeap { words } => Decoded::Ump(words.to_vec()),
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum Decoded {
    Short(u8, u32),
    Long(u8, Vec<u8>),
    Ump(Vec<u32>),
}

struct Slow {
    mode: GateMode,
    buffer: VecDeque<GateEntry>,
    heap_bytes: usize,
    wake_sent: bool,
    dropped: u64,
    control: Sender<ServiceEvent>,
}

impl Slow {
    fn push(&mut self, msg: Msg<'_>) {
        let entry = GateEntry::from(msg);
        let bytes = entry.heap_bytes();

        if bytes > MAX_ENTRY_BYTES {
            self.dropped += 1;
            return;
        }

        // Drop-oldest
        while self.buffer.len() >= MAX_BUFFERED_EVENTS
            || self.heap_bytes + bytes > MAX_BUFFERED_BYTES
        {
            let Some(old) = self.buffer.pop_front() else {
                break;
            };
            self.heap_bytes -= old.heap_bytes();
            self.dropped += 1;
        }

        self.heap_bytes += bytes;
        self.buffer.push_back(entry);
    }

    fn clear(&mut self) {
        self.buffer.clear();
        self.buffer.shrink_to(BUFFER_IDLE_CAPACITY);
        self.heap_bytes = 0;
        self.wake_sent = false;
    }
}

/// On its own cache line: producers store into it, and if it shared a line with
/// `fast` every store would invalidate the pointer the other producer threads
/// are about to read.
#[repr(align(64))]
struct Activity(AtomicBool);

pub struct MidiGate {
    fast: ArcSwapOption<RealtimeEventSender>,
    slow: Mutex<Slow>,
    activity: Activity,
    state: Arc<AtomicU32>,
}

pub type SharedGate = Arc<MidiGate>;

pub struct GateSession<'a> {
    gate: &'a MidiGate,
    live: Guard<Option<Arc<RealtimeEventSender>>>,
}

impl GateSession<'_> {
    #[inline]
    pub fn short(&self, port: u8, msg: u32) {
        match self.live.as_ref() {
            Some(snd) => snd.process_short(port, msg, None),
            None => self.gate.slow_path(Msg::Short { port, msg }),
        }
    }

    #[inline]
    pub fn long(&self, port: u8, data: &[u8]) {
        match self.live.as_ref() {
            Some(snd) => snd.process_long(port, data, None),
            None => self.gate.slow_path(Msg::Long { port, data }),
        }
    }

    #[inline]
    pub fn ump(&self, words: &[u32]) {
        match self.live.as_ref() {
            Some(snd) => snd.process_ump(words, None),
            None => self.gate.slow_path(Msg::Ump(words)),
        }
    }
}

impl MidiGate {
    pub fn new(control: Sender<ServiceEvent>) -> SharedGate {
        Arc::new(Self {
            fast: ArcSwapOption::empty(),
            slow: Mutex::new(Slow {
                mode: GateMode::Off,
                buffer: VecDeque::with_capacity(BUFFER_IDLE_CAPACITY),
                heap_bytes: 0,
                wake_sent: false,
                dropped: 0,
                control,
            }),
            activity: Activity(AtomicBool::new(false)),
            state: Arc::new(AtomicU32::new(GateMode::Off.code())),
        })
    }

    // producer side

    #[inline]
    pub fn session(&self) -> GateSession<'_> {
        // Load-then-conditional-store rather than an unconditional store: while
        // MIDI is flowing the line stays shared and every producer core only
        // reads it, so there is exactly one invalidation per control-loop tick
        // however many producer threads exist.
        if !self.activity.0.load(Ordering::Relaxed) {
            self.activity.0.store(true, Ordering::Relaxed);
        }
        GateSession {
            gate: self,
            live: self.fast.load(),
        }
    }

    #[cold]
    #[inline(never)]
    fn slow_path(&self, msg: Msg<'_>) {
        let mut slow = self.slow.lock().unwrap_or_else(|e| e.into_inner());

        // Re-check under the lock. `promote` publishes `fast` while holding
        // this same lock and only after draining, so seeing `Some` here proves
        // the replay has finished and forwarding now cannot overtake a buffered
        // event. The guard is dropped before anything else happens: holding an
        // arc-swap guard across a blocking operation is what would deadlock
        // against the quiesce wait in `demote`.
        {
            let live = self.fast.load();
            if let Some(snd) = live.as_ref() {
                msg.send_to(snd);
                return;
            }
        }

        match slow.mode {
            GateMode::Off => return,
            GateMode::Idle => {
                if !slow.wake_sent {
                    slow.wake_sent = true;
                    // An unbounded std channel never blocks on send, so doing
                    // this under the lock cannot stall a MIDI thread
                    let _ = slow.control.send(ServiceEvent::WakeEngine);
                }
            }
            GateMode::Starting => {}
            GateMode::Live => {
                debug_assert!(false, "gate is Live with no published sender");
                return;
            }
        }

        slow.push(msg);
    }

    // control-loop side

    /// Must not be called while holding self.slow
    pub fn demote(&self, mode: GateMode) {
        debug_assert!(mode != GateMode::Live);

        let old = {
            let mut slow = self.slow.lock().unwrap_or_else(|e| e.into_inner());
            slow.mode = mode;
            if mode == GateMode::Off {
                slow.clear();
            }
            self.state.store(mode.code(), Ordering::Relaxed);
            // Swapped under the lock so a producer taking the lock next can
            // never see a non-Live mode alongside a published sender
            self.fast.swap(None)
        };

        let Some(old) = old else { return };

        // `ArcSwap::swap` converts every outstanding `load()` guard on the old
        // value into a real strong reference, so the strong count is an
        // accurate liveness test: once it is back to our own reference, every
        // producer that held a guard has finished its call and no new one can
        // obtain one.
        //
        // This is best-effort, not load-bearing. If it times out, a straggler's
        // send into the dropped engine simply fails and is counted by the
        // engine's own dropped-event counter — a handful of events at worst,
        // nothing unsafe.
        let deadline = Instant::now() + QUIESCE_DEADLINE;
        while Arc::strong_count(&old) > 1 {
            if Instant::now() >= deadline {
                log_warn!("MIDI gate: a producer did not quiesce within {QUIESCE_DEADLINE:?}");
                break;
            }
            std::thread::yield_now();
        }
    }

    pub fn promote(&self, sender: RealtimeEventSender) {
        let mut slow = self.slow.lock().unwrap_or_else(|e| e.into_inner());

        let replayed = slow.buffer.len();
        for entry in slow.buffer.drain(..) {
            entry.replay(&sender);
        }
        slow.buffer.shrink_to(BUFFER_IDLE_CAPACITY);
        slow.heap_bytes = 0;

        if replayed > 0 {
            log_info!("MIDI gate replayed {replayed} buffered event(s)");
        }
        if slow.dropped > 0 {
            log_warn!(
                "MIDI gate dropped {} event(s) while the engine was down",
                slow.dropped
            );
            slow.dropped = 0;
        }
        slow.wake_sent = false;
        slow.mode = GateMode::Live;
        self.state.store(GateMode::Live.code(), Ordering::Relaxed);

        self.fast.store(Some(Arc::new(sender)));
    }

    pub fn wants_start(&self) -> bool {
        let slow = self.slow.lock().unwrap_or_else(|e| e.into_inner());
        slow.mode == GateMode::Idle && slow.wake_sent
    }

    // Returns and clears "MIDI arrived since the last call"
    pub fn take_activity(&self) -> bool {
        self.activity.0.swap(false, Ordering::Relaxed)
    }

    // Lock-free view of the current mode, for the statistics publisher
    pub fn state_cell(&self) -> Arc<AtomicU32> {
        self.state.clone()
    }

    #[cfg(test)]
    fn mode(&self) -> GateMode {
        self.slow.lock().unwrap().mode
    }

    #[cfg(test)]
    fn dropped(&self) -> u64 {
        self.slow.lock().unwrap().dropped
    }

    #[cfg(test)]
    fn drain_buffer(&self) -> Vec<Decoded> {
        let mut slow = self.slow.lock().unwrap();
        let drained: Vec<Decoded> = slow.buffer.iter().map(GateEntry::decode).collect();
        slow.buffer.clear();
        slow.heap_bytes = 0;
        drained
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::mpsc::{Receiver, TryRecvError, channel};

    fn gate() -> (SharedGate, Receiver<ServiceEvent>) {
        let (tx, rx) = channel();
        (MidiGate::new(tx), rx)
    }

    /// The engine is down for inactivity, which is the state the wake-up queue
    /// exists for.
    fn idle_gate() -> (SharedGate, Receiver<ServiceEvent>) {
        let (gate, rx) = gate();
        gate.demote(GateMode::Idle);
        (gate, rx)
    }

    #[test]
    fn an_off_gate_discards_everything() {
        let (gate, rx) = gate();
        assert_eq!(gate.mode(), GateMode::Off);

        gate.session().short(0, 0x90_40_7F);
        gate.session().long(1, &[0xF0, 0x7E, 0xF7]);

        assert!(gate.drain_buffer().is_empty());
        assert!(!gate.wants_start());
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn an_idle_gate_buffers_in_order_across_message_kinds() {
        let (gate, _rx) = idle_gate();

        gate.session().short(0, 0x90_40_7F);
        gate.session().long(1, &[0xF0, 0x43, 0x10, 0xF7]);
        gate.session().ump(&[0x2091_407F]);
        gate.session().short(0, 0x80_40_00);

        assert_eq!(
            gate.drain_buffer(),
            vec![
                Decoded::Short(0, 0x90_40_7F),
                Decoded::Long(1, vec![0xF0, 0x43, 0x10, 0xF7]),
                Decoded::Ump(vec![0x2091_407F]),
                Decoded::Short(0, 0x80_40_00),
            ]
        );
    }

    /// Payloads longer than the inline capacity take the heap path; both must
    /// survive the round trip byte for byte.
    #[test]
    fn long_payloads_survive_the_heap_path() {
        let (gate, _rx) = idle_gate();

        let sysex: Vec<u8> = (0..64u8).collect();
        let ump: Vec<u32> = (0..16u32).collect();
        gate.session().long(3, &sysex);
        gate.session().ump(&ump);

        assert_eq!(
            gate.drain_buffer(),
            vec![Decoded::Long(3, sysex), Decoded::Ump(ump)]
        );
    }

    #[test]
    fn idle_asks_for_a_start_exactly_once_per_down_cycle() {
        let (gate, rx) = idle_gate();

        for note in 0..8u32 {
            gate.session().short(0, 0x90_00_7F | (note << 8));
        }

        assert_eq!(rx.try_recv(), Ok(ServiceEvent::WakeEngine));
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
        assert!(gate.wants_start());
    }

    /// A start is already under way, so asking for another one would just make
    /// the control loop restart the engine it is in the middle of building.
    #[test]
    fn starting_buffers_without_asking_for_a_start() {
        let (gate, rx) = gate();
        gate.demote(GateMode::Starting);

        gate.session().short(0, 0x90_40_7F);

        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
        assert!(!gate.wants_start());
        assert_eq!(gate.drain_buffer(), vec![Decoded::Short(0, 0x90_40_7F)]);
    }

    /// Drop-oldest evicts a prefix, so a NoteOn that survives always keeps its
    /// NoteOff. Dropping the newest instead would leave notes sounding forever.
    #[test]
    fn overflow_evicts_the_oldest_and_counts_the_loss() {
        let (gate, _rx) = idle_gate();

        let total = MAX_BUFFERED_EVENTS + 100;
        for n in 0..total as u32 {
            gate.session().short(0, n);
        }

        assert_eq!(gate.dropped(), 100);

        let drained = gate.drain_buffer();
        assert_eq!(drained.len(), MAX_BUFFERED_EVENTS);
        // The tail survived; the head is what went.
        assert_eq!(drained.first(), Some(&Decoded::Short(0, 100)));
        assert_eq!(
            drained.last(),
            Some(&Decoded::Short(0, total as u32 - 1)),
            "the most recent event must always be kept"
        );
    }

    #[test]
    fn the_byte_cap_bounds_a_flood_of_sysex() {
        let (gate, _rx) = idle_gate();

        let chunk = vec![0u8; 8192];
        for _ in 0..(MAX_BUFFERED_BYTES / chunk.len()) + 10 {
            gate.session().long(0, &chunk);
        }

        let drained = gate.drain_buffer();
        let held: usize = drained
            .iter()
            .map(|d| match d {
                Decoded::Long(_, data) => data.len(),
                _ => 0,
            })
            .sum();
        assert!(held <= MAX_BUFFERED_BYTES, "held {held} bytes");
        assert!(gate.dropped() > 0);
    }

    /// One absurd payload must not be allowed to evict everything else to make
    /// room for itself.
    #[test]
    fn an_oversized_entry_is_refused_without_clearing_the_buffer() {
        let (gate, _rx) = idle_gate();

        gate.session().short(0, 0x90_40_7F);
        gate.session().long(0, &vec![0u8; MAX_ENTRY_BYTES + 1]);

        assert_eq!(gate.dropped(), 1);
        assert_eq!(gate.drain_buffer(), vec![Decoded::Short(0, 0x90_40_7F)]);
    }

    /// `Off` means the engine is not coming back, so holding events would just
    /// leak. `Idle` means it is, so they must survive.
    #[test]
    fn demoting_to_off_clears_the_buffer_but_idle_keeps_it() {
        let (gate, _rx) = idle_gate();
        gate.session().short(0, 0x90_40_7F);

        gate.demote(GateMode::Idle);
        assert_eq!(gate.drain_buffer().len(), 1);

        gate.session().short(0, 0x90_40_7F);
        gate.demote(GateMode::Off);
        assert!(gate.drain_buffer().is_empty());
        assert!(!gate.wants_start());
    }

    #[test]
    fn the_state_cell_mirrors_the_mode_for_the_stats_publisher() {
        let (gate, _rx) = gate();
        let cell = gate.state_cell();

        assert_eq!(cell.load(Ordering::Relaxed), GateMode::Off.code());
        gate.demote(GateMode::Starting);
        assert_eq!(cell.load(Ordering::Relaxed), GateMode::Starting.code());
        gate.demote(GateMode::Idle);
        assert_eq!(cell.load(Ordering::Relaxed), GateMode::Idle.code());
    }

    #[test]
    fn activity_is_reported_once_then_cleared() {
        let (gate, _rx) = gate();

        assert!(!gate.take_activity());
        gate.session().short(0, 0x90_40_7F);
        assert!(gate.take_activity());
        assert!(!gate.take_activity());
    }

    /// Several producer threads against a control thread flapping the gate: no
    /// deadlock, no lost lock, and whatever survives for a given port must stay
    /// in the order that port sent it. Drop-oldest means the result is a
    /// subsequence rather than everything, so monotonicity is the invariant.
    #[test]
    fn concurrent_producers_keep_per_port_ordering() {
        const PRODUCERS: u8 = 4;
        const PER_PRODUCER: u32 = 20_000;

        let (gate, _rx) = idle_gate();
        let stop = Arc::new(AtomicBool::new(false));

        let control = std::thread::spawn({
            let gate = gate.clone();
            let stop = stop.clone();
            move || {
                let mut seen: Vec<Vec<u32>> = vec![Vec::new(); PRODUCERS as usize];
                while !stop.load(Ordering::Relaxed) {
                    for decoded in gate.drain_buffer() {
                        if let Decoded::Short(port, msg) = decoded {
                            seen[port as usize].push(msg);
                        }
                    }
                    gate.demote(GateMode::Idle);
                    std::thread::yield_now();
                }
                for decoded in gate.drain_buffer() {
                    if let Decoded::Short(port, msg) = decoded {
                        seen[port as usize].push(msg);
                    }
                }
                seen
            }
        });

        let producers: Vec<_> = (0..PRODUCERS)
            .map(|port| {
                let gate = gate.clone();
                std::thread::spawn(move || {
                    for n in 0..PER_PRODUCER {
                        gate.session().short(port, n);
                    }
                })
            })
            .collect();

        for producer in producers {
            producer.join().expect("producer finished");
        }
        stop.store(true, Ordering::Relaxed);
        let seen = control.join().expect("control thread finished");

        for (port, msgs) in seen.iter().enumerate() {
            assert!(
                msgs.windows(2).all(|w| w[0] < w[1]),
                "port {port} observed events out of order"
            );
        }
    }
}
