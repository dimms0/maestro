//! The types of `include/maestro.h`, which documents them. Keep the two in sync.

use std::ffi::{CStr, c_char, c_void};

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
#[allow(dead_code)]
pub const MAESTRO_ERR_FULL: MaestroResult = -6;
pub const MAESTRO_ERR_INTERNAL: MaestroResult = -7;

pub const MAESTRO_LOG_ERROR: u32 = 0;
pub const MAESTRO_LOG_WARN: u32 = 1;

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
pub struct MaestroInfo {
    pub struct_size: u32,
    pub sample_rate: u32,
    pub num_ports: u32,
    pub midi2: u32,
    pub render_block_us: u32,
}

#[repr(C)]
pub struct MaestroStats {
    pub struct_size: u32,
    pub voice_count: u64,
    pub render_time: f32,
    pub nps: u64,
    pub eps: u64,
    pub nps_limit: u64,
}

type SendShort = unsafe extern "C" fn(*mut MaestroInstance, u8, u32, MaestroTime) -> MaestroResult;
type SendLong =
    unsafe extern "C" fn(*mut MaestroInstance, u8, *const u8, usize, MaestroTime) -> MaestroResult;
type SendUmp =
    unsafe extern "C" fn(*mut MaestroInstance, *const u32, usize, MaestroTime) -> MaestroResult;
type Control = unsafe extern "C" fn(*mut MaestroInstance) -> MaestroResult;
type Get<T> = unsafe extern "C" fn(*mut MaestroInstance, *mut T) -> MaestroResult;

pub const MAESTRO_EXT_MIDI1: &CStr = c"maestro.midi1/1";
#[repr(C)]
pub struct MaestroExtMidi1 {
    pub send_short: SendShort,
    pub send_long: SendLong,
}

pub const MAESTRO_EXT_UMP: &CStr = c"maestro.ump/1";
#[repr(C)]
pub struct MaestroExtUmp {
    pub send: SendUmp,
}

pub const MAESTRO_EXT_CONTROL: &CStr = c"maestro.control/1";
#[repr(C)]
pub struct MaestroExtControl {
    pub reset: Control,
    pub show_settings: Control,
}

pub const MAESTRO_EXT_INFO: &CStr = c"maestro.info/1";
#[repr(C)]
pub struct MaestroExtInfo {
    pub get: Get<MaestroInfo>,
}

pub const MAESTRO_EXT_STATS: &CStr = c"maestro.stats/1";
#[repr(C)]
pub struct MaestroExtStats {
    pub get: Get<MaestroStats>,
}
