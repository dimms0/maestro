//! The types of `include/maestro.h`, which documents them. Keep the two in sync.

use std::ffi::{CStr, c_char, c_void};

pub type MaestroResult = i32;
pub const MAESTRO_OK: MaestroResult = 0;
pub const MAESTRO_ERR_INVALID_ARG: MaestroResult = -1;
pub const MAESTRO_ERR_UNSUPPORTED: MaestroResult = -2;
pub const MAESTRO_ERR_DISABLED: MaestroResult = -3;
pub const MAESTRO_ERR_INTERNAL: MaestroResult = -4;
pub const MAESTRO_ERR_UNAVAILABLE: MaestroResult = -5;
pub const MAESTRO_ERR_NOT_RUNNING: MaestroResult = -100;
pub const MAESTRO_ERR_IO: MaestroResult = -200;
pub const MAESTRO_ERR_BAD_MIDI: MaestroResult = -201;
pub const MAESTRO_ERR_ENCODER: MaestroResult = -202;
pub const MAESTRO_ERR_CANCELLED: MaestroResult = -203;
pub const MAESTRO_ERR_BUSY: MaestroResult = -204;

pub type MaestroLogLevel = u32;
pub const MAESTRO_LOG_ERROR: MaestroLogLevel = 0;
pub const MAESTRO_LOG_WARN: MaestroLogLevel = 1;

pub type MaestroLogFn =
    Option<unsafe extern "C" fn(user: *mut c_void, level: MaestroLogLevel, message: *const c_char)>;

#[repr(C)]
pub struct MaestroAppInfo {
    pub struct_size: u32,
    pub reserved: u32,
    pub app_id: *const c_char,
    pub app_name: *const c_char,
    pub log: MaestroLogFn,
    pub user: *mut c_void,
}

pub const MAESTRO_CORE_V1: &CStr = c"maestro.core/1";
#[repr(C)]
pub struct MaestroCoreV1 {
    pub get_version: unsafe extern "C" fn() -> u32,
    pub last_error: unsafe extern "C" fn() -> *const c_char,
}

// ---- Realtime ----

pub type MaestroTime = u64;

#[repr(C)]
pub struct MaestroRealtime {
    _private: [u8; 0],
}

pub type MaestroNotify = u32;
pub const MAESTRO_NOTIFY_SETTINGS_CHANGED: MaestroNotify = 0;
pub const MAESTRO_NOTIFY_ENGINE_STOPPED: MaestroNotify = 1;
pub const MAESTRO_NOTIFY_ENGINE_STARTED: MaestroNotify = 2;
pub const MAESTRO_NOTIFY_ENGINE_DISABLED: MaestroNotify = 3;

pub type MaestroNotifyFn = Option<unsafe extern "C" fn(user: *mut c_void, what: MaestroNotify)>;

#[repr(C)]
pub struct MaestroRealtimeParams {
    pub struct_size: u32,
    pub num_ports: u32,
    pub midi2: u32,
    pub reserved: u32,
    pub notify: MaestroNotifyFn,
}

#[repr(C)]
pub struct MaestroRealtimeInfo {
    pub struct_size: u32,
    pub sample_rate: u32,
    pub num_ports: u32,
    pub midi2: u32,
    pub render_block_us: u32,
    pub max_schedule_us: u32,
}

#[repr(C)]
pub struct MaestroRealtimeStats {
    pub struct_size: u32,
    pub render_time: f32,
    pub latency_us: u32,
    pub reserved: u32,
    pub voice_count: u64,
    pub nps: u64,
    pub eps: u64,
    pub nps_limit: u64,
}

type Rt = *mut MaestroRealtime;

pub const MAESTRO_REALTIME_V1: &CStr = c"maestro.realtime/1";
#[repr(C)]
pub struct MaestroRealtimeV1 {
    pub open: unsafe extern "C" fn(
        *const MaestroAppInfo,
        *const MaestroRealtimeParams,
        *mut Rt,
    ) -> MaestroResult,
    pub close: unsafe extern "C" fn(Rt),
    pub now: unsafe extern "C" fn() -> MaestroTime,
    pub ns_to_ticks: unsafe extern "C" fn(u64) -> MaestroTime,
    pub send_short: unsafe extern "C" fn(Rt, u8, u32, MaestroTime) -> MaestroResult,
    pub send_long: unsafe extern "C" fn(Rt, u8, *const u8, usize, MaestroTime) -> MaestroResult,
    pub send_ump: unsafe extern "C" fn(Rt, *const u32, usize, MaestroTime) -> MaestroResult,
    pub get_info: unsafe extern "C" fn(Rt, *mut MaestroRealtimeInfo) -> MaestroResult,
    pub get_stats: unsafe extern "C" fn(Rt, *mut MaestroRealtimeStats) -> MaestroResult,
    pub reset: unsafe extern "C" fn(Rt) -> MaestroResult,
    pub notes_off: unsafe extern "C" fn(Rt) -> MaestroResult,
    pub show_settings: unsafe extern "C" fn(Rt) -> MaestroResult,
}

// ---- File rendering ----

#[repr(C)]
pub struct MaestroFileRender {
    _private: [u8; 0],
}

pub type MaestroFormat = u32;
pub const MAESTRO_FORMAT_WAV: MaestroFormat = 0;
pub const MAESTRO_FORMAT_FLAC: MaestroFormat = 1;
pub const MAESTRO_FORMAT_MP3: MaestroFormat = 2;
pub const MAESTRO_FORMAT_OGG: MaestroFormat = 3;

pub type MaestroPortMode = u32;
pub const MAESTRO_PORTS_MULTI: MaestroPortMode = 0;
pub const MAESTRO_PORTS_SINGLE: MaestroPortMode = 1;

#[repr(C)]
pub struct MaestroFileRenderParams {
    pub struct_size: u32,
    pub reserved: u32,
    pub midi_path: *const c_char,
    pub output_path: *const c_char,
    pub format: MaestroFormat,
    pub wav_bit_depth: u32,
    pub bitrate_kbps: u32,
    pub sample_rate: u32,
    pub channels: u32,
    pub port_mode: MaestroPortMode,
    pub max_ports: u32,
}

#[repr(C)]
pub struct MaestroFileRenderProgress {
    pub struct_size: u32,
    pub speed: f32,
    pub position_s: f64,
    pub length_s: f64,
    pub notes: u64,
    pub events: u64,
    pub voice_count: u64,
}

type Job = *mut MaestroFileRender;

pub const MAESTRO_FILE_RENDER_V1: &CStr = c"maestro.file_render/1";
#[repr(C)]
pub struct MaestroFileRenderV1 {
    pub open: unsafe extern "C" fn(
        *const MaestroAppInfo,
        *const MaestroFileRenderParams,
        *mut Job,
    ) -> MaestroResult,
    pub close: unsafe extern "C" fn(Job),
    pub run: unsafe extern "C" fn(Job) -> MaestroResult,
    pub cancel: unsafe extern "C" fn(Job),
    pub progress: unsafe extern "C" fn(Job, *mut MaestroFileRenderProgress) -> MaestroResult,
}
