//! The raw types of `maestro.h`, which documents them. Keep the two in sync.
//!
//! Most programs want the safe wrappers at the crate root instead; these are
//! here for anything they don't cover yet.

use std::ffi::{c_char, c_void};

pub const MAESTRO_VERSION_MAJOR: u32 = 1;
pub const MAESTRO_VERSION_MINOR: u32 = 0;
pub const MAESTRO_API_VERSION: u32 = (MAESTRO_VERSION_MAJOR << 16) | MAESTRO_VERSION_MINOR;

pub type MaestroTime = u64;
pub const MAESTRO_TIME_NOW: MaestroTime = 0;

#[repr(C)]
pub struct MaestroInstance {
    _private: [u8; 0],
}

pub type MaestroResult = i32;
pub const MAESTRO_OK: MaestroResult = 0;
pub const MAESTRO_ERR_INVALID_ARG: MaestroResult = -1;
pub const MAESTRO_ERR_UNSUPPORTED: MaestroResult = -2;
pub const MAESTRO_ERR_DISABLED: MaestroResult = -3;
pub const MAESTRO_ERR_UNAVAILABLE: MaestroResult = -4;
pub const MAESTRO_ERR_NOT_RUNNING: MaestroResult = -5;
pub const MAESTRO_ERR_FULL: MaestroResult = -6;
pub const MAESTRO_ERR_INTERNAL: MaestroResult = -7;

pub const MAESTRO_LOG_ERROR: u32 = 0;
pub const MAESTRO_LOG_WARN: u32 = 1;
pub const MAESTRO_LOG_INFO: u32 = 2;
pub const MAESTRO_LOG_DEBUG: u32 = 3;

pub const MAESTRO_NOTIFY_SETTINGS_CHANGED: u32 = 0;
pub const MAESTRO_NOTIFY_ENGINE_STOPPED: u32 = 1;
pub const MAESTRO_NOTIFY_ENGINE_STARTED: u32 = 2;

pub type MaestroLogFn =
    Option<unsafe extern "C" fn(user: *mut c_void, level: u32, message: *const c_char)>;
pub type MaestroNotifyFn = Option<unsafe extern "C" fn(user: *mut c_void, what: u32)>;

#[repr(C)]
pub struct MaestroHints {
    pub struct_size: u32,
    pub num_ports: u32,
    pub midi2: u32,
}

#[repr(C)]
pub struct MaestroOpenParams {
    pub struct_size: u32,
    pub app_id: *const c_char,
    pub app_name: *const c_char,
    pub hints: *const MaestroHints,
    pub log: MaestroLogFn,
    pub notify: MaestroNotifyFn,
    pub user: *mut c_void,
}

#[repr(C)]
#[derive(Default)]
pub struct MaestroInfo {
    pub struct_size: u32,
    pub sample_rate: u32,
    pub num_ports: u32,
    pub midi2: u32,
    pub render_block_us: u32,
}

#[repr(C)]
#[derive(Default)]
pub struct MaestroStats {
    pub struct_size: u32,
    pub voice_count: u64,
    pub render_time: f32,
    pub nps: u64,
    pub eps: u64,
    pub nps_limit: u64,
}

// ---- Core functions, as resolved from the library ----

pub type GetVersionFn = unsafe extern "C" fn() -> u32;
pub type LastErrorFn = unsafe extern "C" fn() -> *const c_char;
pub type NowFn = unsafe extern "C" fn() -> MaestroTime;
pub type NsToTicksFn = unsafe extern "C" fn(ns: u64) -> MaestroTime;
pub type OpenFn = unsafe extern "C" fn(
    api_version: u32,
    params: *const MaestroOpenParams,
    out: *mut *mut MaestroInstance,
) -> MaestroResult;
pub type CloseFn = unsafe extern "C" fn(inst: *mut MaestroInstance);
pub type GetExtensionFn =
    unsafe extern "C" fn(inst: *mut MaestroInstance, id: *const c_char) -> *const c_void;

// ---- Extensions ----

pub const MAESTRO_EXT_MIDI1: &std::ffi::CStr = c"maestro.midi1/1";
#[repr(C)]
pub struct MaestroExtMidi1 {
    pub send_short: unsafe extern "C" fn(
        inst: *mut MaestroInstance,
        port: u8,
        msg: u32,
        at: MaestroTime,
    ) -> MaestroResult,
    pub send_long: unsafe extern "C" fn(
        inst: *mut MaestroInstance,
        port: u8,
        data: *const u8,
        len: usize,
        at: MaestroTime,
    ) -> MaestroResult,
}

pub const MAESTRO_EXT_UMP: &std::ffi::CStr = c"maestro.ump/1";
#[repr(C)]
pub struct MaestroExtUmp {
    pub send: unsafe extern "C" fn(
        inst: *mut MaestroInstance,
        words: *const u32,
        count: usize,
        at: MaestroTime,
    ) -> MaestroResult,
}

pub const MAESTRO_EXT_CONTROL: &std::ffi::CStr = c"maestro.control/1";
#[repr(C)]
pub struct MaestroExtControl {
    pub reset: unsafe extern "C" fn(inst: *mut MaestroInstance) -> MaestroResult,
    pub show_settings: unsafe extern "C" fn(inst: *mut MaestroInstance) -> MaestroResult,
}

pub const MAESTRO_EXT_INFO: &std::ffi::CStr = c"maestro.info/1";
#[repr(C)]
pub struct MaestroExtInfo {
    pub get:
        unsafe extern "C" fn(inst: *mut MaestroInstance, out: *mut MaestroInfo) -> MaestroResult,
}

pub const MAESTRO_EXT_STATS: &std::ffi::CStr = c"maestro.stats/1";
#[repr(C)]
pub struct MaestroExtStats {
    pub get:
        unsafe extern "C" fn(inst: *mut MaestroInstance, out: *mut MaestroStats) -> MaestroResult,
}
