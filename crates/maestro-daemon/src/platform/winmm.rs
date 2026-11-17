use std::fs::File;
use std::hint::spin_loop;
use std::io::{ErrorKind, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use maestro_core::ipc::{STATE_LIVE, STATE_OFF, STATE_PAUSED};
use windows_sys::Win32::System::Threading::INFINITE;

use crate::{
    error::DaemonError,
    gate::{GateSession, SharedGate},
    log_warn,
    platform::{DeviceSpec, MidiBackend},
    ring::{Consumer, Record, proto},
    shm::{self, Event, Section},
};

const BATCH: usize = 4096;
const SPIN: Duration = Duration::from_millis(2);
const PARK_MS: u32 = 10;
const POLL: Duration = Duration::from_millis(20);
const CLOSE_GRACE: Duration = Duration::from_millis(100);

// Process-wide so ids stay unique across backend restarts, where a client may
// reconnect while still holding handles from before
static NEXT_ID: AtomicU32 = AtomicU32::new(0);

struct Ring {
    id: u32,
    port: u8,
    consumer: Consumer<Section>,
}

struct Shared {
    stop: AtomicBool,
    rings: ArcSwap<Vec<Arc<Ring>>>,
    wake: Event,
}

impl Shared {
    fn remove(&self, id: u32) {
        self.rings.rcu(|rings| rings.iter().filter(|r| r.id != id).cloned().collect::<Vec<_>>());
    }
}

pub struct WinmmBackend {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    accept: Option<JoinHandle<()>>,
    pipe: String,
    description: String,
}

fn backend_err(what: &str) -> impl FnOnce(std::io::Error) -> DaemonError {
    move |err| DaemonError::Backend(format!("WinMM: {what} failed: {err}"))
}

impl WinmmBackend {
    pub fn new(spec: &DeviceSpec, gate: SharedGate) -> Result<Self, DaemonError> {
        let pid = std::process::id();
        let pipe = shm::pipe_name();
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            rings: ArcSwap::from_pointee(Vec::new()),
            wake: Event::create(&shm::wake_name(pid)).map_err(backend_err("creating the wake event"))?,
        });

        let hello = proto::hello(spec.num_ports, pid, &spec.device_name);
        let ports = spec.num_ports;
        let (tx, rx) = channel();
        let accept = {
            let (shared, pipe) = (shared.clone(), pipe.clone());
            thread::Builder::new()
                .name("maestro-winmm-accept".into())
                .spawn(move || accept_loop(&shared, &pipe, tx))
                .map_err(backend_err("starting the accept thread"))?
        };
        let control = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("maestro-winmm-ctl".into())
                .spawn(move || control_loop(&shared, rx, &hello, ports))
                .map_err(backend_err("starting the control thread"))?
        };
        let consumer = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("maestro-winmm-rx".into())
                .spawn(move || consume_loop(&shared, &gate))
                .map_err(backend_err("starting the event thread"))?
        };

        Ok(Self {
            shared,
            threads: vec![control, consumer],
            accept: Some(accept),
            description: format!("WinMM driver link: {ports} port(s) on {pipe}"),
            pipe,
        })
    }
}

impl MidiBackend for WinmmBackend {
    fn describe(&self) -> String {
        self.description.clone()
    }

    fn has_devices_connected(&self) -> bool {
        !self.shared.rings.load().is_empty()
    }
}

impl Drop for WinmmBackend {
    fn drop(&mut self) {
        self.shared.stop.store(true, Relaxed);
        self.shared.wake.set();
        // Connecting is the only way to release a blocked ConnectNamedPipe;
        // retried in case the accept thread was between two instances
        if let Some(accept) = self.accept.take() {
            while !accept.is_finished() {
                let _ = shm::connect_pipe(&self.pipe);
                thread::sleep(Duration::from_millis(1));
            }
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

// Control: connections, opens and closes. Cold, so it just polls.

struct Conn {
    pipe: File,
    buf: Vec<u8>,
    rings: Vec<u32>,
}

fn accept_loop(shared: &Shared, pipe: &str, tx: Sender<File>) {
    while !shared.stop.load(Relaxed) {
        match shm::accept_pipe(pipe) {
            Ok(conn) => {
                if tx.send(conn).is_err() {
                    return;
                }
            }
            Err(err) => {
                log_warn!("WinMM driver pipe failed: {err}");
                thread::sleep(POLL);
            }
        }
    }
}

fn control_loop(shared: &Shared, rx: Receiver<File>, hello: &[u8], ports: u8) {
    let mut conns: Vec<Conn> = Vec::new();
    while !shared.stop.load(Relaxed) {
        for mut pipe in rx.try_iter() {
            if pipe.write_all(hello).is_ok() {
                conns.push(Conn { pipe, buf: Vec::new(), rings: Vec::new() });
            }
        }

        conns.retain_mut(|conn| {
            let open = serve(shared, conn, ports);
            if !open {
                for &id in &conn.rings {
                    retire(shared, id);
                }
            }
            open
        });

        thread::sleep(POLL);
    }

    // Releases producers blocked on a full ring
    for ring in shared.rings.load().iter() {
        ring.consumer.close();
    }
}

/// Returns false once the connection is finished
fn serve(shared: &Shared, conn: &mut Conn, ports: u8) -> bool {
    let Ok(n) = shm::pipe_available(&conn.pipe) else {
        return false;
    };
    let start = conn.buf.len();
    conn.buf.resize(start + n, 0);
    if conn.pipe.read_exact(&mut conn.buf[start..]).is_err() {
        return false;
    }

    loop {
        let (request, used) = match proto::parse(&conn.buf) {
            Ok(Some(parsed)) => parsed,
            Ok(None) => return true,
            Err(_) => return false,
        };
        conn.buf.drain(..used);

        match request {
            proto::Request::Open { port } => {
                let reply = match open_ring(shared, port, ports) {
                    Ok(id) => {
                        conn.rings.push(id);
                        proto::opened(id).to_vec()
                    }
                    Err(err) => {
                        log_warn!("Failed to open WinMM port {}: {err}", port as u32 + 1);
                        vec![proto::FAILED]
                    }
                };
                if conn.pipe.write_all(&reply).is_err() {
                    return false;
                }
            }
            proto::Request::Close { id } => {
                if let Some(i) = conn.rings.iter().position(|&r| r == id) {
                    conn.rings.swap_remove(i);
                    retire(shared, id);
                }
            }
        }
    }
}

fn open_ring(shared: &Shared, port: u8, ports: u8) -> std::io::Result<u32> {
    if port >= ports {
        return Err(ErrorKind::InvalidInput.into());
    }
    let id = NEXT_ID.fetch_add(1, Relaxed);

    let section = Section::create(&shm::ring_name(std::process::id(), id))?;
    let ring = Arc::new(Ring { id, port, consumer: Consumer::new(section) });
    shared.rings.rcu(|rings| {
        let mut rings = Vec::clone(rings);
        rings.push(ring.clone());
        rings
    });
    // The consumer may be parked with no timeout while no ring exists
    shared.wake.set();
    Ok(id)
}

/// Gives the consumer a moment to play what the app sent before closing, so
/// final note-offs are not lost
fn retire(shared: &Shared, id: u32) {
    let Some(ring) = shared.rings.load().iter().find(|r| r.id == id).cloned() else {
        return;
    };
    let deadline = Instant::now() + CLOSE_GRACE;
    while !ring.consumer.is_drained() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    ring.consumer.close();
    shared.remove(id);
}

// Consumer: drains every ring into the gate, off the render path

fn consume_loop(shared: &Shared, gate: &SharedGate) {
    let state = gate.state_cell();
    let mut scratch = Vec::new();
    let mut last = Instant::now();
    let mut idle = 0u32;
    // One event already went to the gate to wake the engine
    let mut woke = false;

    while !shared.stop.load(Relaxed) {
        let rings = shared.rings.load();

        if rings.iter().any(|r| r.consumer.pending()) {
            last = Instant::now();
            idle = 0;
            let mode = state.load(Relaxed);
            if mode != STATE_PAUSED {
                woke = false;
            }

            match mode {
                STATE_LIVE => {
                    let session = gate.session();
                    for ring in rings.iter() {
                        drain(shared, ring, BATCH, &session, &mut scratch);
                    }
                }
                STATE_PAUSED if !woke => {
                    woke = true;
                    let session = gate.session();
                    if let Some(ring) = rings.iter().find(|r| r.consumer.pending()) {
                        drain(shared, ring, 1, &session, &mut scratch);
                    }
                }
                STATE_OFF => {
                    for ring in rings.iter() {
                        ring.consumer.discard();
                    }
                }
                // Starting: events wait in the rings, apps block once one fills
                _ => {
                    drop(rings);
                    thread::sleep(Duration::from_millis(1));
                }
            }
            continue;
        }

        if rings.is_empty() {
            drop(rings);
            shared.wake.wait(INFINITE);
            continue;
        }

        idle = idle.wrapping_add(1);
        if !idle.is_multiple_of(64) || last.elapsed() < SPIN {
            spin_loop();
            continue;
        }

        for ring in rings.iter() {
            ring.consumer.set_parked(true);
        }
        if rings.iter().any(|r| r.consumer.pending()) {
            for ring in rings.iter() {
                ring.consumer.set_parked(false);
            }
            continue;
        }
        drop(rings);
        shared.wake.wait(PARK_MS);
        for ring in shared.rings.load().iter() {
            ring.consumer.set_parked(false);
        }
    }
}

fn drain(shared: &Shared, ring: &Ring, max: usize, session: &GateSession, scratch: &mut Vec<u8>) {
    let port = ring.port;
    let result = ring.consumer.drain(max, scratch, |record| match record {
        Record::Short(msg) => session.short(port, msg, 0),
        Record::Long(data) => session.long(port, data, 0),
        Record::Reset => session.short(port, 0xFF, 0),
    });
    if result.is_err() {
        log_warn!("Disconnected a WinMM client that sent a malformed event stream");
        ring.consumer.close();
        shared.remove(ring.id);
    }
}
