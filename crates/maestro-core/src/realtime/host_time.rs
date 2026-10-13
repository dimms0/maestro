use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    instant: Instant,
    time: Duration,
}

impl Anchor {
    pub fn new(now: Duration) -> Self {
        Self {
            instant: Instant::now(),
            time: now,
        }
    }

    pub fn at(&self, time: Duration) -> Instant {
        match time.checked_sub(self.time) {
            Some(ahead) => self.instant + ahead,
            None => self
                .instant
                .checked_sub(self.time - time)
                .unwrap_or(self.instant),
        }
    }
}

pub fn from_host_ticks(ticks: u64) -> Instant {
    Anchor::new(to_duration(host_now())).at(to_duration(ticks))
}

pub fn ns_to_ticks(ns: u64) -> u64 {
    from_nanos(ns)
}

#[cfg(target_os = "linux")]
pub fn host_now() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

#[cfg(target_os = "linux")]
fn to_duration(ticks: u64) -> Duration {
    Duration::from_nanos(ticks)
}

#[cfg(target_os = "linux")]
fn from_nanos(ns: u64) -> u64 {
    ns
}

#[cfg(target_os = "macos")]
pub fn host_now() -> u64 {
    unsafe { libc::mach_absolute_time() }
}

#[cfg(target_os = "macos")]
fn timebase() -> (u128, u128) {
    static TIMEBASE: std::sync::OnceLock<(u128, u128)> = std::sync::OnceLock::new();

    *TIMEBASE.get_or_init(|| {
        let mut info = libc::mach_timebase_info { numer: 1, denom: 1 };
        #[allow(deprecated)]
        unsafe {
            libc::mach_timebase_info(&mut info)
        };
        (u128::from(info.numer.max(1)), u128::from(info.denom.max(1)))
    })
}

#[cfg(target_os = "macos")]
fn to_duration(ticks: u64) -> Duration {
    let (numer, denom) = timebase();
    Duration::from_nanos((u128::from(ticks) * numer / denom) as u64)
}

#[cfg(target_os = "macos")]
fn from_nanos(ns: u64) -> u64 {
    let (numer, denom) = timebase();
    (u128::from(ns) * denom / numer) as u64
}

#[cfg(windows)]
pub fn host_now() -> u64 {
    let mut count = 0;
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut count) };
    count as u64
}

#[cfg(windows)]
fn frequency() -> u128 {
    static FREQUENCY: std::sync::OnceLock<u128> = std::sync::OnceLock::new();

    *FREQUENCY.get_or_init(|| {
        let mut frequency = 0;
        unsafe {
            windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut frequency)
        };
        (frequency as u128).max(1)
    })
}

#[cfg(windows)]
fn to_duration(ticks: u64) -> Duration {
    Duration::from_nanos((u128::from(ticks) * 1_000_000_000 / frequency()) as u64)
}

#[cfg(windows)]
fn from_nanos(ns: u64) -> u64 {
    (u128::from(ns) * frequency() / 1_000_000_000) as u64
}
