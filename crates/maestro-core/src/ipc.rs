use std::{
    env,
    fs::{self, File, TryLockError},
    io::{self, Read, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use interprocess::local_socket::{
    GenericNamespaced, ListenerOptions, Stream, ToNsName, prelude::*,
};

use crate::{realtime::MaestroRealtimeStatistics, system_cfg::ConfigComponent};

pub type SharedStats = Arc<Mutex<Option<MaestroRealtimeStatistics>>>;

const MAGIC: u32 = 0x4d53_5433; // "MST3"
const NAME_CAP: usize = 64;
const FRAME_LEN: usize = size_of::<MaestroStats>();

pub const DAEMON_LABEL: &str = "Maestro";

pub const STATE_OFF: u32 = 0;
pub const STATE_STARTING: u32 = 1;
pub const STATE_LIVE: u32 = 2;
pub const STATE_PAUSED: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, PartialEq)]
pub struct MaestroStats {
    magic: u32,
    pub pid: u32,
    pub voices: u64,
    pub rss_bytes: u64,
    pub nps: u64,
    pub eps: u64,
    pub render_last: f32,
    pub render_avg: f32,
    pub render_peak: f32,
    pub state: u32,
    pub component: u32,
    name_len: u32,
    name: [u8; NAME_CAP],
}

impl MaestroStats {
    fn new(label: &str, stats: &SharedStats, component: u32, state: u32) -> Self {
        let engine = stats.lock().unwrap().clone();

        let mut frame = Self {
            magic: MAGIC,
            pid: std::process::id(),
            voices: 0,
            rss_bytes: crate::sysinfo::rss_bytes().unwrap_or(0),
            nps: 0,
            eps: 0,
            render_last: 0.0,
            render_avg: 0.0,
            render_peak: 0.0,
            state,
            component,
            name_len: 0,
            name: [0; NAME_CAP],
        };

        if let Some(engine) = engine {
            let renderer = engine.get_renderer();
            frame.voices = renderer.read_voice_count();
            frame.render_last = renderer.get_last_render_time();
            frame.render_avg = renderer.get_average_render_time();
            frame.render_peak = engine.read_render_peak();
            frame.nps = engine.read_nps();
            frame.eps = engine.read_eps();
        }

        let mut len = label.len().min(NAME_CAP);
        while len > 0 && !label.is_char_boundary(len) {
            len -= 1;
        }
        frame.name[..len].copy_from_slice(&label.as_bytes()[..len]);
        frame.name_len = len as u32;

        frame
    }

    pub fn name(&self) -> &str {
        let len = (self.name_len as usize).min(NAME_CAP);
        std::str::from_utf8(&self.name[..len]).unwrap_or_default()
    }

    pub fn component(&self) -> Option<ConfigComponent> {
        ConfigComponent::from_code(self.component)
    }

    pub fn is_daemon(&self) -> bool {
        self.component() == Some(ConfigComponent::System) && self.name() == DAEMON_LABEL
    }

    pub fn is_active(&self) -> bool {
        self.state != STATE_OFF || self.is_daemon()
    }

    fn as_bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts((self as *const Self).cast::<u8>(), FRAME_LEN) }
    }

    fn from_bytes(bytes: [u8; FRAME_LEN]) -> Option<Self> {
        let frame: Self = unsafe { std::mem::transmute(bytes) };
        (frame.magic == MAGIC).then_some(frame)
    }
}

fn runtime_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    // Mode 0700, per-user, and cleaned up at logout
    if let Some(dir) = env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join("maestro");
    }

    #[cfg(target_os = "macos")]
    if let Some(dir) = darwin_user_temp_dir() {
        return dir.join("maestro");
    }

    #[cfg(target_os = "windows")]
    if let Some(dir) = env::var_os("LOCALAPPDATA").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join("Temp").join("maestro");
    }

    let user = env::var("USER")
        .or_else(|_| env::var("USERNAME"))
        .unwrap_or_default();
    env::temp_dir().join(format!("maestro-{user}"))
}

#[cfg(target_os = "macos")]
fn darwin_user_temp_dir() -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    const CS_DARWIN_USER_TEMP_DIR: libc::c_int = 65537;

    let len = unsafe { libc::confstr(CS_DARWIN_USER_TEMP_DIR, std::ptr::null_mut(), 0) };
    if len == 0 {
        return None;
    }

    let mut buf = vec![0u8; len];
    let written = unsafe { libc::confstr(CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr().cast(), len) };
    if written == 0 || written > len {
        return None;
    }

    // confstr counts the trailing NUL
    buf.truncate(written - 1);
    Some(PathBuf::from(OsString::from_vec(buf)))
}

fn socket_name(pid: u32, component: &ConfigComponent) -> String {
    format!("maestro-stats-{pid}-{}.sock", component.to_filename())
}

pub fn process_name() -> String {
    env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "Unknown".to_string())
}

const REQ_STATS: u8 = 0;
const REQ_SHUTDOWN: u8 = 1;
const REQ_RESET: u8 = 2;

pub type Hook = Option<Box<dyn Fn() + Send + Sync>>;

#[derive(Default)]
pub struct PublishOptions {
    pub state: Option<Arc<AtomicU32>>,
    pub on_shutdown: Hook,
    pub on_reset: Hook,
}

pub fn publish(
    label: &str,
    stats: SharedStats,
    component: &ConfigComponent,
    options: PublishOptions,
) -> io::Result<()> {
    let pid = std::process::id();
    let name = socket_name(pid, component).to_ns_name::<GenericNamespaced>()?;
    let listener = ListenerOptions::new().name(name).create_sync()?;

    let dir = runtime_dir();
    fs::create_dir_all(&dir)?;
    File::create(dir.join(format!("{pid}-{}", component.to_filename())))?;

    let label = label.to_string();
    let code = component.code();
    let options = Arc::new(options);
    thread::Builder::new()
        .name("maestro-stats".to_string())
        .spawn(move || {
            for conn in listener.incoming().flatten() {
                serve(conn, &label, &stats, code, &options);
            }
        })?;

    Ok(())
}

fn serve(
    mut conn: Stream,
    label: &str,
    stats: &SharedStats,
    component: u32,
    options: &PublishOptions,
) {
    let mut request = [0u8; 1];
    while conn.read_exact(&mut request).is_ok() {
        match request[0] {
            REQ_SHUTDOWN => {
                if let Some(hook) = &options.on_shutdown {
                    hook();
                }
                return;
            }
            REQ_RESET => {
                if let Some(hook) = &options.on_reset {
                    hook();
                }
            }
            _ => {
                let state = match &options.state {
                    Some(cell) => cell.load(Ordering::Relaxed),
                    None if stats.lock().unwrap().is_some() => STATE_LIVE,
                    None => STATE_OFF,
                };
                let frame = MaestroStats::new(label, stats, component, state);
                if conn.write_all(frame.as_bytes()).is_err() {
                    return;
                }
            }
        }
    }
}

pub fn request_shutdown_all(component: &ConfigComponent) -> usize {
    let dir = runtime_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return 0;
    };

    let suffix = format!("-{}", component.to_filename());
    let mut asked = 0;

    for entry in entries.flatten() {
        let Some(filename) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(pid) = filename
            .strip_suffix(&suffix)
            .and_then(|p| p.parse::<u32>().ok())
        else {
            continue;
        };

        let sent = socket_name(pid, component)
            .to_ns_name::<GenericNamespaced>()
            .and_then(Stream::connect)
            .and_then(|mut conn| conn.write_all(&[REQ_SHUTDOWN]));

        match sent {
            Ok(()) => asked += 1,
            // The publisher is gone, clean up after it
            Err(_) => {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    asked
}

const RESCAN: Duration = Duration::from_secs(1);

pub struct StatsSubscriber {
    conns: Vec<(String, Stream)>,
    last_scan: Option<Instant>,
}

impl StatsSubscriber {
    pub fn new() -> Self {
        Self {
            conns: Vec::new(),
            last_scan: None,
        }
    }

    fn request(conn: &mut Stream) -> Option<MaestroStats> {
        conn.write_all(&[REQ_STATS]).ok()?;
        let mut frame = [0u8; FRAME_LEN];
        conn.read_exact(&mut frame).ok()?;
        MaestroStats::from_bytes(frame)
    }

    pub fn poll(&mut self) -> Vec<MaestroStats> {
        if self.last_scan.is_none_or(|at| at.elapsed() >= RESCAN) {
            self.scan();
            self.last_scan = Some(Instant::now());
        }

        let mut frames = Vec::with_capacity(self.conns.len());
        self.conns
            .retain_mut(|(_, conn)| match Self::request(conn) {
                Some(frame) => {
                    frames.push(frame);
                    true
                }
                None => false,
            });
        frames
    }

    pub fn reset(&mut self, pid: u32, component: &ConfigComponent) {
        let name = format!("{pid}-{}", component.to_filename());
        if let Some((_, conn)) = self.conns.iter_mut().find(|(known, _)| *known == name) {
            let _ = conn.write_all(&[REQ_RESET]);
        }
    }

    fn scan(&mut self) {
        let dir = runtime_dir();
        let Ok(entries) = fs::read_dir(&dir) else {
            return;
        };

        for entry in entries.flatten() {
            let Some(filename) = entry.file_name().to_str().map(|s| s.to_string()) else {
                continue;
            };

            let mut split = filename.split('-');
            let Some(pid) = split.next().and_then(|p| p.parse::<u32>().ok()) else {
                continue;
            };
            let Some(component) = split.next().and_then(ConfigComponent::from_name) else {
                continue;
            };

            if self.conns.iter().any(|(known, _)| *known == filename) {
                continue;
            }

            match socket_name(pid, &component)
                .to_ns_name::<GenericNamespaced>()
                .and_then(Stream::connect)
            {
                Ok(conn) => self.conns.push((filename, conn)),
                Err(_) => {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
}

impl Default for StatsSubscriber {
    fn default() -> Self {
        Self::new()
    }
}

pub fn try_lock_single_instance(name: &str) -> bool {
    static LOCK: OnceLock<File> = OnceLock::new();

    let dir = runtime_dir();
    if fs::create_dir_all(&dir).is_err() {
        return true;
    }

    let Ok(file) = File::create(dir.join(format!("{name}.lock"))) else {
        return true;
    };

    match file.try_lock() {
        Ok(()) => {
            let _ = LOCK.set(file);
            true
        }
        Err(TryLockError::WouldBlock) => false,
        // Locking is unsupported here (some network filesystems) so dont keep
        // the user from starting the application over that...
        Err(TryLockError::Error(_)) => true,
    }
}
