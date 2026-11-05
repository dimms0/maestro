use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    host: u64,
    time: Duration,
}

impl Anchor {
    pub fn new(now: Duration) -> Self {
        Self {
            host: host_now(),
            time: now,
        }
    }

    pub fn at(&self, time: Duration) -> u64 {
        match time.checked_sub(self.time) {
            Some(ahead) => self.host + ns_to_ticks(ahead.as_nanos() as u64),
            None => self
                .host
                .saturating_sub(ns_to_ticks((self.time - time).as_nanos() as u64)),
        }
    }
}

pub fn ns_to_ticks(ns: u64) -> u64 {
    from_nanos(ns)
}

pub(crate) use tsc::{calibrate, now, resync};

#[cfg(not(all(
    any(target_arch = "x86", target_arch = "x86_64"),
    any(target_os = "linux", windows)
)))]
mod tsc {
    pub(crate) use super::{host_now as now, host_now as resync};

    pub(crate) fn calibrate() {}
}

#[cfg(all(
    any(target_arch = "x86", target_arch = "x86_64"),
    any(target_os = "linux", windows)
))]
mod tsc {
    #[cfg(target_arch = "x86")]
    use std::arch::x86 as arch;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64 as arch;
    use std::sync::{
        Once,
        atomic::{AtomicU64, Ordering},
    };

    const CALIBRATION_NS: u64 = 5_000_000;

    /// Host ticks per TSC tick in 32.32 fixed point, 0 while the TSC is unused
    static SCALE: AtomicU64 = AtomicU64::new(0);
    static BIAS: AtomicU64 = AtomicU64::new(0);

    #[inline]
    fn rdtsc() -> u64 {
        unsafe { arch::_rdtsc() }
    }

    #[inline]
    fn scaled(tsc: u64, scale: u64) -> u64 {
        ((tsc as u128 * scale as u128) >> 32) as u64
    }

    #[inline]
    pub(crate) fn now() -> u64 {
        match SCALE.load(Ordering::Acquire) {
            0 => super::host_now(),
            scale => scaled(rdtsc(), scale).wrapping_add(BIAS.load(Ordering::Relaxed)),
        }
    }

    /// A TSC and host clock reading taken as close together as possible
    fn sample() -> (u64, u64) {
        (0..8)
            .map(|_| {
                let before = rdtsc();
                let host = super::host_now();
                let span = rdtsc().wrapping_sub(before);
                (span, before.wrapping_add(span / 2), host)
            })
            .min()
            .map_or((0, 0), |(_, tsc, host)| (tsc, host))
    }

    pub(crate) fn resync() -> u64 {
        let scale = SCALE.load(Ordering::Relaxed);
        if scale == 0 {
            return super::host_now();
        }
        let (tsc, host) = sample();
        BIAS.store(host.wrapping_sub(scaled(tsc, scale)), Ordering::Relaxed);
        host
    }

    #[cfg(target_os = "linux")]
    fn reliable() -> bool {
        std::fs::read_to_string("/sys/devices/system/clocksource/clocksource0/current_clocksource")
            .is_ok_and(|source| source.trim() == "tsc")
    }

    #[cfg(windows)]
    fn reliable() -> bool {
        #[allow(unused_unsafe)]
        let invariant = unsafe {
            arch::__cpuid(0x8000_0000).eax >= 0x8000_0007
                && arch::__cpuid(0x8000_0007).edx & 1 << 8 != 0
        };
        invariant
    }

    pub(crate) fn calibrate() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            if !reliable() {
                return;
            }

            let (tsc, host) = sample();
            let span = super::ns_to_ticks(CALIBRATION_NS);
            while super::host_now().wrapping_sub(host) < span {
                std::hint::spin_loop();
            }
            let (end_tsc, end_host) = sample();

            let Some(ticks) = end_tsc.checked_sub(tsc).filter(|&t| t > 0) else {
                return;
            };
            let scale = ((u128::from(end_host - host) << 32) / u128::from(ticks)) as u64;
            BIAS.store(
                end_host.wrapping_sub(scaled(end_tsc, scale)),
                Ordering::Relaxed,
            );
            SCALE.store(scale, Ordering::Release);
        });
    }
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
pub(crate) fn ticks_per_sec() -> u64 {
    1_000_000_000
}

#[cfg(target_os = "linux")]
fn from_nanos(ns: u64) -> u64 {
    ns
}

#[cfg(target_os = "macos")]
pub fn host_now() -> u64 {
    unsafe { mach2::mach_time::mach_absolute_time() }
}

#[cfg(target_os = "macos")]
fn timebase() -> (u128, u128) {
    static TIMEBASE: std::sync::OnceLock<(u128, u128)> = std::sync::OnceLock::new();

    *TIMEBASE.get_or_init(|| {
        let mut info = mach2::mach_time::mach_timebase_info { numer: 1, denom: 1 };
        unsafe { mach2::mach_time::mach_timebase_info(&mut info) };
        (u128::from(info.numer.max(1)), u128::from(info.denom.max(1)))
    })
}

#[cfg(target_os = "macos")]
pub(crate) fn ticks_per_sec() -> u64 {
    let (numer, denom) = timebase();
    (1_000_000_000 * denom / numer) as u64
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
fn frequency() -> u64 {
    static FREQUENCY: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

    *FREQUENCY.get_or_init(|| {
        let mut frequency = 0;
        unsafe {
            windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut frequency)
        };
        (frequency as u64).max(1)
    })
}

#[cfg(windows)]
pub(crate) fn ticks_per_sec() -> u64 {
    frequency()
}

#[cfg(windows)]
fn from_nanos(ns: u64) -> u64 {
    (u128::from(ns) * u128::from(frequency()) / 1_000_000_000) as u64
}
