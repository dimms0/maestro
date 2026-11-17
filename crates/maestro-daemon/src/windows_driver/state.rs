use std::fs::File;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex, OnceLock};

use super::ffi::*;
use super::log_driver_warn;
use crate::ring::{Link, Producer, proto};
use crate::shm::{self, Event, Process, Section};

pub struct DaemonLink {
    wake: Arc<Event>,
    daemon: Arc<Process>,
}

impl Link for DaemonLink {
    fn wake(&self) {
        self.wake.set();
    }

    fn alive(&self) -> bool {
        self.daemon.alive()
    }
}

// Boxed and handed to winmm through `dwUser`,
// so every open/close pair owns exactly one allocation
pub struct OutClient {
    pub ring: Producer<Section, DaemonLink>,
    id: u32,
    pid: u32,
    pub callback: usize,
    pub instance: usize,
    pub hmidi: usize,
    pub flags: u32,
}

// shortcut: reads have no timeout, so a hung maestrod stalls midiOutOpen
struct Connection {
    stream: File,
    pid: u32,
    ports: u8,
    name: String,
    wake: Arc<Event>,
    daemon: Arc<Process>,
}

impl Connection {
    fn connect() -> io::Result<Self> {
        let mut stream = shm::connect_pipe(&shm::pipe_name())?;

        let mut head = [0u8; 6];
        stream.read_exact(&mut head)?;
        let ports = head[0];
        let pid = u32::from_le_bytes([head[1], head[2], head[3], head[4]]);
        let mut name = vec![0; head[5] as usize];
        stream.read_exact(&mut name)?;

        Ok(Self {
            wake: Arc::new(Event::open(&shm::wake_name(pid))?),
            daemon: Arc::new(Process::open(pid)?),
            stream,
            pid,
            ports,
            name: String::from_utf8_lossy(&name).into_owned(),
        })
    }

    /// maestrod never sends unprompted, so pending bytes mean a stale reply
    fn alive(&mut self) -> bool {
        matches!(shm::pipe_available(&self.stream), Ok(0))
    }

    fn open(&mut self, port: u8) -> io::Result<u32> {
        self.stream.write_all(&proto::open(port))?;
        let mut reply = [0u8; 1];
        self.stream.read_exact(&mut reply)?;
        if reply[0] != proto::OPENED {
            return Err(io::Error::other("maestrod refused the port"));
        }
        let mut id = [0u8; 4];
        self.stream.read_exact(&mut id)?;
        Ok(u32::from_le_bytes(id))
    }
}

pub struct DriverState {
    conn: Mutex<Option<Connection>>,
}

pub(super) fn get_state() -> &'static DriverState {
    static STATE: OnceLock<DriverState> = OnceLock::new();
    STATE.get_or_init(|| DriverState {
        conn: Mutex::new(None),
    })
}

fn utf16_name(name: &str) -> [u16; 32] {
    let mut buf = [0u16; 32];
    for (i, unit) in name.encode_utf16().take(31).enumerate() {
        buf[i] = unit;
    }
    buf
}

impl DriverState {
    fn with_conn<T>(&self, f: impl FnOnce(&mut Connection) -> io::Result<T>) -> io::Result<T> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if conn.as_mut().is_some_and(|c| !c.alive()) {
            *conn = None;
        }
        if conn.is_none() {
            *conn = Some(Connection::connect()?);
        }
        let result = f(conn.as_mut().unwrap());
        if result.is_err() {
            *conn = None;
        }
        result
    }

    pub fn num_output_devices(&self) -> u32 {
        self.with_conn(|c| Ok(c.ports as u32)).unwrap_or(0)
    }

    pub fn output_caps(&self, device: u32) -> Option<MIDIOUTCAPSW> {
        let name = self
            .with_conn(|c| {
                Ok((device < c.ports as u32).then(|| format!("{} - Port {}", c.name, device + 1)))
            })
            .ok()??;

        Some(MIDIOUTCAPSW {
            wMid: 0,
            wPid: 0,
            vDriverVersion: 0x0100,
            szPname: utf16_name(&name),
            wTechnology: MOD_SWSYNTH as u16,
            wVoices: u16::MAX,
            wNotes: u16::MAX,
            wChannelMask: 0xFFFF,
            dwSupport: 0,
        })
    }

    pub fn open_output(
        &self,
        device: u32,
        desc: &MIDIOPENDESC,
        flags: u32,
    ) -> Result<Box<OutClient>, u32> {
        let opened = self.with_conn(|c| {
            if device >= c.ports as u32 {
                return Ok(None);
            }
            let id = c.open(device as u8)?;
            let section = Section::open(&shm::ring_name(c.pid, id))?;
            let link = DaemonLink {
                wake: c.wake.clone(),
                daemon: c.daemon.clone(),
            };
            Ok(Some((Producer::new(section, link), id, c.pid)))
        });

        match opened {
            Ok(Some((ring, id, pid))) => Ok(Box::new(OutClient {
                ring,
                id,
                pid,
                callback: desc.dwCallback,
                instance: desc.dwInstance,
                hmidi: desc.hMidi,
                flags,
            })),
            Ok(None) => Err(MMSYSERR_BADDEVICEID),
            Err(err) => {
                log_driver_warn("failed to open the port through maestrod", err);
                Err(MMSYSERR_NODRIVER)
            }
        }
    }

    pub fn close_output(&self, client: &OutClient) {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = conn.as_mut().filter(|c| c.pid == client.pid)
            && c.stream.write_all(&proto::close(client.id)).is_err()
        {
            *conn = None;
        }
    }
}
