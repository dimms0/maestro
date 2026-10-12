use std::collections::HashSet;
use std::ffi::CString;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use alsa::seq::{
    Addr, Connect, EvCtrl, EvNote, Event, EventType, PortCap, PortInfo, PortType, Seq,
};
use alsa::{Direction, poll::Descriptors};
use maestro_core::realtime::host_time::Anchor;

use crate::{
    error::DaemonError,
    gate::{GateSession, SharedGate},
    log_error, log_info, log_warn,
    platform::{DeviceSpec, MidiBackend},
};

mod ump;

const POLL_TIMEOUT_MS: i32 = 250;
const ANCHOR_RESYNC: Duration = Duration::from_secs(1);

pub struct AlsaBackend {
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    description: String,
    devices_connected: Arc<AtomicBool>,
}

impl AlsaBackend {
    pub fn new(spec: &DeviceSpec, gate: SharedGate) -> Result<Self, DaemonError> {
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut description = Vec::new();
        let devices_connected = Arc::new(AtomicBool::new(false));

        let thread = if spec.midi2 {
            match ump::UmpEndpoint::create(&spec.device_name, &devices_connected) {
                Ok(endpoint) => {
                    let client_id = endpoint.client_id().unwrap_or_default();

                    description.push(format!(
                        "MIDI 2.0 UMP endpoint \"{}\" (16 groups) with ID={}",
                        spec.device_name, client_id
                    ));

                    spawn_named("maestro-midi2", {
                        let gate = gate.clone();
                        let shutdown = shutdown.clone();
                        move || endpoint.run(gate, shutdown)
                    })
                }
                Err(err) => {
                    log_warn!(
                        "MIDI 2.0 requested but unavailable ({err}); \
                         falling back to MIDI 1.0 ports"
                    );

                    let (handle, client_id) =
                        spawn_midi1(spec, &gate, &shutdown, &devices_connected)?;

                    description.push(format!(
                        "MIDI 1.0 device \"{}\" with {} port(s) and ID={} [MIDI 2.0 fallback]",
                        spec.device_name, spec.num_ports, client_id
                    ));

                    handle
                }
            }
        } else {
            let (handle, client_id) = spawn_midi1(spec, &gate, &shutdown, &devices_connected)?;

            description.push(format!(
                "MIDI 1.0 device \"{}\" with {} port(s) and ID={}",
                spec.device_name, spec.num_ports, client_id
            ));

            handle
        };

        Ok(Self {
            shutdown,
            thread: Some(thread),
            description: format!("ALSA devices created: {}", description.join(", ")),
            devices_connected,
        })
    }
}

impl MidiBackend for AlsaBackend {
    fn describe(&self) -> String {
        self.description.clone()
    }

    fn has_devices_connected(&self) -> bool {
        self.devices_connected.load(Ordering::Relaxed)
    }
}

impl Drop for AlsaBackend {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        log_info!("ALSA devices removed");
    }
}

fn spawn_named(name: &str, f: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(f)
        .expect("failed to spawn thread")
}

fn spawn_midi1(
    spec: &DeviceSpec,
    gate: &SharedGate,
    shutdown: &Arc<AtomicBool>,
    devices_connected: &Arc<AtomicBool>,
) -> Result<(JoinHandle<()>, i32), DaemonError> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let spec = spec.clone();
    let gate = gate.clone();
    let shutdown = shutdown.clone();
    let devices_connected = devices_connected.clone();

    let handle = spawn_named("maestro-midi1", move || {
        let (seq, queue) = match create_midi1_client(&spec) {
            Ok((seq, queue)) => {
                let client_id = seq.client_id().unwrap_or(-1);
                let _ = ready_tx.send(Ok(client_id));
                (seq, queue)
            }
            Err(err) => {
                let _ = ready_tx.send(Err(err));
                return;
            }
        };
        input_loop(seq, queue, &gate, &shutdown, &devices_connected);
    });

    match ready_rx.recv() {
        Ok(Ok(client_id)) => Ok((handle, client_id)),
        Ok(Err(err)) => {
            let _ = handle.join();
            Err(err)
        }
        Err(_) => Err(DaemonError::Backend(
            "MIDI device thread died during setup".to_string(),
        )),
    }
}

fn create_midi1_client(spec: &DeviceSpec) -> Result<(Seq, i32), DaemonError> {
    let backend_err =
        |e: &dyn std::fmt::Display| DaemonError::Backend(format!("ALSA sequencer: {e}"));

    let seq = Seq::open(None, None, true).map_err(|e| backend_err(&e))?;

    let client_name = CString::new(spec.device_name.as_str())
        .map_err(|_| DaemonError::Backend("Device name contains a NUL byte".to_string()))?;
    seq.set_client_name(&client_name)
        .map_err(|e| backend_err(&e))?;
    seq.set_client_pool_input(1 << 21)
        .map_err(|e| backend_err(&e))?;
    let queue = seq.alloc_queue().map_err(|e| backend_err(&e))?;

    for i in 0..spec.num_ports {
        let port_name = CString::new(format!("{} - Port {}", spec.device_name, i + 1))
            .map_err(|_| DaemonError::Backend("Port name contains a NUL byte".to_string()))?;

        let mut pinfo = PortInfo::empty().map_err(|e| backend_err(&e))?;
        pinfo.set_name(&port_name);
        pinfo.set_capability(PortCap::WRITE | PortCap::SUBS_WRITE);
        pinfo.set_type(
            PortType::MIDI_GENERIC
                | PortType::MIDI_GM
                | PortType::MIDI_GM2
                | PortType::MIDI_GS
                | PortType::MIDI_XG
                | PortType::SYNTHESIZER
                | PortType::APPLICATION
                | PortType::SOFTWARE,
        );
        pinfo.set_timestamping(true);
        pinfo.set_timestamp_real(true);
        pinfo.set_timestamp_queue(queue);
        seq.create_port(&pinfo).map_err(|e| backend_err(&e))?;
    }

    seq.control_queue(queue, EventType::Start, 0, None)
        .map_err(|e| backend_err(&e))?;
    seq.drain_output().map_err(|e| backend_err(&e))?;

    Ok((seq, queue))
}

fn queue_anchor(seq: &Seq, queue: i32) -> Option<Anchor> {
    seq.get_queue_status(queue)
        .ok()
        .map(|status| Anchor::new(status.get_real_time()))
}

fn input_loop(
    seq: Seq,
    queue: i32,
    gate: &SharedGate,
    shutdown: &Arc<AtomicBool>,
    devices_connected: &Arc<AtomicBool>,
) {
    let mut fds = match (&seq, Some(Direction::Capture)).get() {
        Ok(fds) => fds,
        Err(err) => {
            log_error!("ALSA poll setup failed: {err}");
            return;
        }
    };
    let mut input = seq.input();

    let mut active: HashSet<(Addr, Addr)> = HashSet::new();
    let mut anchor = queue_anchor(&seq, queue);
    let mut synced = Instant::now();

    while !shutdown.load(Ordering::Relaxed) {
        match alsa::poll::poll(&mut fds, POLL_TIMEOUT_MS) {
            Ok(0) => continue,
            Ok(_) => {}
            Err(err) => {
                if err.errno() == libc::EINTR {
                    continue;
                }
                log_error!("ALSA poll failed: {err}");
                return;
            }
        }

        loop {
            let ev = match input.event_input() {
                Ok(ev) => ev,
                Err(err) if err.errno() == libc::EAGAIN => break,
                Err(err) if err.errno() == libc::EINTR => continue,
                Err(err) if err.errno() == libc::ENOSPC => {
                    // ALSA input buffer overrun, some events were dropped
                    // no need to error or notify
                    continue;
                }
                Err(err) => {
                    log_error!("ALSA event input failed: {err}");
                    return;
                }
            };

            if handle_subscription_event(&ev, &mut active, devices_connected) {
                continue;
            }

            if synced.elapsed() >= ANCHOR_RESYNC {
                anchor = queue_anchor(&seq, queue);
                synced = Instant::now();
            }

            let at = anchor
                .zip(ev.get_time())
                .map(|(anchor, time)| anchor.at(time));
            dispatch_event(&ev, &gate.session(), at);
        }
    }
}

fn handle_subscription_event(
    ev: &Event,
    active: &mut HashSet<(Addr, Addr)>,
    devices_connected: &Arc<AtomicBool>,
) -> bool {
    let ty = ev.get_type();
    if ty != EventType::PortSubscribed && ty != EventType::PortUnsubscribed {
        return false;
    }
    let Some(conn) = ev.get_data::<Connect>() else {
        return true;
    };

    let key = (conn.sender, conn.dest);
    if ty == EventType::PortSubscribed {
        if active.insert(key) && active.len() == 1 {
            devices_connected.store(true, Ordering::Relaxed);
        }
    } else if active.remove(&key) && active.is_empty() {
        devices_connected.store(false, Ordering::Relaxed);
    }
    true
}

fn dispatch_event(ev: &Event, sender: &GateSession<'_>, at: Option<Instant>) {
    let port = ev.get_dest().port as u8;

    let short = |status: u32, d1: u32, d2: u32| status | ((d1 & 0x7F) << 8) | ((d2 & 0x7F) << 16);

    match ev.get_type() {
        EventType::Noteon => {
            if let Some(n) = ev.get_data::<EvNote>() {
                let ch = (n.channel & 0xF) as u32;
                sender.short(port, short(0x90 | ch, n.note as u32, n.velocity as u32), at);
            }
        }
        EventType::Noteoff => {
            if let Some(n) = ev.get_data::<EvNote>() {
                let ch = (n.channel & 0xF) as u32;
                sender.short(port, short(0x80 | ch, n.note as u32, n.velocity as u32), at);
            }
        }
        EventType::Keypress => {
            if let Some(n) = ev.get_data::<EvNote>() {
                let ch = (n.channel & 0xF) as u32;
                sender.short(port, short(0xA0 | ch, n.note as u32, n.velocity as u32), at);
            }
        }
        EventType::Controller => {
            if let Some(c) = ev.get_data::<EvCtrl>() {
                let ch = (c.channel & 0xF) as u32;
                sender.short(port, short(0xB0 | ch, c.param, c.value as u32), at);
            }
        }
        EventType::Pgmchange => {
            if let Some(c) = ev.get_data::<EvCtrl>() {
                let ch = (c.channel & 0xF) as u32;
                sender.short(port, 0xC0 | ch | ((c.value as u32 & 0x7F) << 8), at);
            }
        }
        EventType::Chanpress => {
            if let Some(c) = ev.get_data::<EvCtrl>() {
                let ch = (c.channel & 0xF) as u32;
                sender.short(port, 0xD0 | ch | ((c.value as u32 & 0x7F) << 8), at);
            }
        }
        EventType::Pitchbend => {
            if let Some(c) = ev.get_data::<EvCtrl>() {
                let ch = (c.channel & 0xF) as u32;
                // The sequencer uses -8192..8191; MIDI wants 0..16383.
                let v = (c.value + 8192).clamp(0, 16383) as u32;
                sender.short(port, short(0xE0 | ch, v, v >> 7), at);
            }
        }
        EventType::Control14 => {
            if let Some(c) = ev.get_data::<EvCtrl>() {
                let ch = (c.channel & 0xF) as u32;
                let v = c.value as u32;
                // MSB controller plus its LSB pair when one exists (0..31).
                sender.short(port, short(0xB0 | ch, c.param, v >> 7), at);
                if c.param < 32 {
                    sender.short(port, short(0xB0 | ch, c.param + 32, v), at);
                }
            }
        }
        EventType::Regparam | EventType::Nonregparam => {
            if let Some(c) = ev.get_data::<EvCtrl>() {
                let ch = (c.channel & 0xF) as u32;
                let (bank_cc, index_cc) = if ev.get_type() == EventType::Regparam {
                    (101, 100)
                } else {
                    (99, 98)
                };
                let v = c.value as u32;
                sender.short(port, short(0xB0 | ch, bank_cc, c.param >> 7), at);
                sender.short(port, short(0xB0 | ch, index_cc, c.param), at);
                sender.short(port, short(0xB0 | ch, 6, v >> 7), at);
                sender.short(port, short(0xB0 | ch, 38, v), at);
            }
        }
        EventType::Sysex => {
            if let Some(data) = ev.get_ext() {
                sender.long(port, data, at);
            }
        }
        EventType::Reset => {
            sender.short(port, 0xFF, at);
        }
        _ => {}
    }
}
