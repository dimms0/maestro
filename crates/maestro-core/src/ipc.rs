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

use crate::{statistics::MaestroRenderStatistics, system_cfg::ConfigComponent};

pub type SharedStats = Arc<Mutex<Option<Arc<MaestroRenderStatistics>>>>;

const MAGIC: u32 = 0x4d53_5431; // "MST1"
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
    pub render_last: f32,
    pub render_avg: f32,
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
            render_last: 0.0,
            render_avg: 0.0,
            state,
            component,
            name_len: 0,
            name: [0; NAME_CAP],
        };

        if let Some(engine) = engine {
            frame.voices = engine.read_voice_count();
            frame.render_last = engine.get_last_render_time();
            frame.render_avg = engine.get_average_render_time();
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

pub type ShutdownHook = Option<Box<dyn Fn() + Send + Sync>>;

#[derive(Default)]
pub struct PublishOptions {
    pub state: Option<Arc<AtomicU32>>,
    pub on_shutdown: ShutdownHook,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn stats_with(voices: u64) -> SharedStats {
        let stats = MaestroRenderStatistics::new();
        stats.set_voices(voices);
        Arc::new(Mutex::new(Some(Arc::new(stats))))
    }

    fn frame(label: &str, stats: &SharedStats, state: u32) -> MaestroStats {
        MaestroStats::new(label, stats, ConfigComponent::System.code(), state)
    }

    #[test]
    fn frame_round_trips_through_bytes() {
        let frame = frame("Maestro Daemon", &stats_with(42), STATE_LIVE);
        let mut bytes = [0u8; FRAME_LEN];
        bytes.copy_from_slice(frame.as_bytes());

        let decoded = MaestroStats::from_bytes(bytes).expect("valid frame");
        assert_eq!(decoded.name(), "Maestro Daemon");
        assert_eq!(decoded.voices, 42);
        assert!(decoded.state == STATE_LIVE);
        assert_eq!(decoded.component(), Some(ConfigComponent::System));
        assert_eq!(decoded.pid, std::process::id());
        assert!(decoded.rss_bytes > 0);
    }

    #[test]
    fn foreign_bytes_are_rejected() {
        assert!(MaestroStats::from_bytes([0u8; FRAME_LEN]).is_none());
    }

    #[test]
    fn long_labels_are_truncated_on_a_character_boundary() {
        let label = "é".repeat(NAME_CAP);
        let frame = frame(&label, &stats_with(0), STATE_LIVE);
        assert!(frame.name().len() <= NAME_CAP);
        assert!(label.starts_with(frame.name()));
    }

    #[test]
    fn a_paused_component_reports_no_voices() {
        let frame = frame("Paused", &Arc::new(Mutex::new(None)), STATE_PAUSED);
        assert!(frame.state != STATE_LIVE);
        assert_eq!(frame.state, STATE_PAUSED);
        assert_eq!(frame.voices, 0);
    }

    #[test]
    fn subscriber_reads_a_published_frame() {
        let stats = stats_with(7);
        let state = Arc::new(AtomicU32::new(STATE_LIVE));
        publish(
            "Test Component",
            stats.clone(),
            &ConfigComponent::System,
            PublishOptions {
                state: Some(state.clone()),
                on_shutdown: None,
            },
        )
        .expect("publish");

        let mut subscriber = StatsSubscriber::new();
        let frames = subscriber.poll();

        let frame = frames
            .iter()
            .find(|f| f.pid == std::process::id())
            .expect("our own frame");
        assert_eq!(frame.name(), "Test Component");
        assert_eq!(frame.voices, 7);
        assert!(frame.state == STATE_LIVE);
        assert_eq!(frame.component(), Some(ConfigComponent::System));

        // The same connection is reused on the next poll, and the state cell is
        // read fresh each time rather than captured at publish.
        stats.lock().unwrap().as_ref().unwrap().set_voices(11);
        state.store(STATE_PAUSED, Ordering::Relaxed);
        let frames = subscriber.poll();
        let frame = frames
            .iter()
            .find(|f| f.pid == std::process::id())
            .expect("our own frame");
        assert_eq!(frame.voices, 11);
        assert_eq!(frame.state, STATE_PAUSED);
    }

    /// Asking a component nobody publishes for must be a no-op, not an error or
    /// a hang against a stale marker file.
    #[test]
    fn a_shutdown_request_with_no_publisher_asks_nobody() {
        assert_eq!(request_shutdown_all(&ConfigComponent::Converter), 0);
    }
}
