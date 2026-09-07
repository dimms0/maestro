pub fn rss_bytes() -> Option<u64> {
    imp::rss_bytes()
}

#[cfg(target_os = "linux")]
mod imp {
    pub fn rss_bytes() -> Option<u64> {
        // Field 2 of /proc/self/statm is the resident set in pages.
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        (page_size > 0).then(|| pages * page_size as u64)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    pub fn rss_bytes() -> Option<u64> {
        // proc_pidinfo(PROC_PIDTASKINFO) is the documented, entitlement-free
        // way to read this for one's own process; task_info would need a port.
        const PROC_PIDTASKINFO: libc::c_int = 4;

        let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;

        let written = unsafe {
            libc::proc_pidinfo(
                std::process::id() as libc::c_int,
                PROC_PIDTASKINFO,
                0,
                (&raw mut info).cast(),
                size,
            )
        };

        (written == size).then_some(info.pti_resident_size)
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    pub fn rss_bytes() -> Option<u64> {
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        counters.cb = size;

        let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &raw mut counters, size) };
        (ok != 0).then(|| counters.WorkingSetSize as u64)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod imp {
    pub fn rss_bytes() -> Option<u64> {
        None
    }
}
