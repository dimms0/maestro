use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_UNLIMITED_INSTANCES,
    PeekNamedPipe, WaitNamedPipeW,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, OpenEventW, OpenProcess, PROCESS_SYNCHRONIZE, SetEvent,
    WaitForSingleObject,
};

use crate::ring::{Region, SIZE};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn handle(h: HANDLE) -> io::Result<HANDLE> {
    if h.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(h)
    }
}

pub fn pipe_name() -> String {
    let mut session = 0;
    unsafe { ProcessIdToSessionId(std::process::id(), &mut session) };
    format!(r"\\.\pipe\maestro-winmm-{session}")
}

// Blocks until a client connects to a fresh instance of the byte-mode pipe
pub fn accept_pipe(name: &str) -> io::Result<File> {
    let h = unsafe {
        CreateNamedPipeW(
            wide(name).as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            4096,
            4096,
            0,
            null(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let pipe = unsafe { File::from_raw_handle(h) };
    if unsafe { ConnectNamedPipe(h, null_mut()) } == 0
        && unsafe { GetLastError() } != ERROR_PIPE_CONNECTED
    {
        return Err(io::Error::last_os_error());
    }
    Ok(pipe)
}

pub fn connect_pipe(name: &str) -> io::Result<File> {
    let open = || OpenOptions::new().read(true).write(true).open(name);
    match open() {
        // Every instance is between a connect and the next accept
        Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
            unsafe { WaitNamedPipeW(wide(name).as_ptr(), 1000) };
            open()
        }
        result => result,
    }
}

pub fn pipe_available(pipe: &File) -> io::Result<usize> {
    let mut avail = 0u32;
    let ok = unsafe {
        PeekNamedPipe(
            pipe.as_raw_handle(),
            null_mut(),
            0,
            null_mut(),
            &mut avail,
            null_mut(),
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(avail as usize)
    }
}

// All names live in the session's Local\ namespace so each user has their own
pub fn ring_name(pid: u32, id: u32) -> String {
    format!("Local\\maestro-ring-{pid}-{id}")
}

pub fn wake_name(pid: u32) -> String {
    format!("Local\\maestro-wake-{pid}")
}

/// A pagefile-backed ring mapping
pub struct Section {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

unsafe impl Send for Section {}
unsafe impl Sync for Section {}

impl Section {
    pub fn create(name: &str) -> io::Result<Self> {
        let h = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                null(),
                PAGE_READWRITE,
                0,
                SIZE as u32,
                wide(name).as_ptr(),
            )
        };
        Self::map(handle(h)?)
    }

    pub fn open(name: &str) -> io::Result<Self> {
        Self::map(handle(unsafe {
            OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, wide(name).as_ptr())
        })?)
    }

    fn map(handle: HANDLE) -> io::Result<Self> {
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, SIZE) };
        if view.Value.is_null() {
            let err = io::Error::last_os_error();
            unsafe { CloseHandle(handle) };
            return Err(err);
        }
        Ok(Self { handle, view })
    }
}

unsafe impl Region for Section {
    fn base(&self) -> *mut u8 {
        self.view.Value as *mut u8
    }
}

impl Drop for Section {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.view);
            CloseHandle(self.handle);
        }
    }
}

/// An auto-reset event
pub struct Event(HANDLE);

unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl Event {
    pub fn create(name: &str) -> io::Result<Self> {
        handle(unsafe { CreateEventW(null(), 0, 0, wide(name).as_ptr()) }).map(Self)
    }

    pub fn open(name: &str) -> io::Result<Self> {
        handle(unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, wide(name).as_ptr()) }).map(Self)
    }

    pub fn set(&self) {
        unsafe { SetEvent(self.0) };
    }

    pub fn wait(&self, timeout_ms: u32) {
        unsafe { WaitForSingleObject(self.0, timeout_ms) };
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub struct Process(HANDLE);

unsafe impl Send for Process {}
unsafe impl Sync for Process {}

impl Process {
    pub fn open(pid: u32) -> io::Result<Self> {
        handle(unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) }).map(Self)
    }

    pub fn alive(&self) -> bool {
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_TIMEOUT }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}
