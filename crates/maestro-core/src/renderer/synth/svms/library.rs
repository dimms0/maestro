#![allow(non_snake_case)]

use std::ffi::{CString, c_char};
use std::sync::{Arc, OnceLock, RwLock};

use crate::{
    error::RendererError,
    helpers::Arena,
    renderer::{
        config::SynthConfig,
        synth::{SoundFontHandle, SynthLibrary},
    },
    soundfont::{SoundFont, SoundFontType},
};

#[cfg(target_os = "windows")]
pub(crate) const SVMS_LIB_FILENAME: &str = "SVMSAPI.dll";
#[cfg(target_os = "macos")]
pub(crate) const SVMS_LIB_FILENAME: &str = "libsvmsapi.dylib";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) const SVMS_LIB_FILENAME: &str = "libsvmsapi.so";

pub(crate) type SVMSSession = u64;
pub(crate) type SVMSResult = i32;

pub(crate) const SVMS_RESULT_OK: SVMSResult = 0;
const SVMS_ABI_VERSION_1: u32 = 1;
pub(crate) const SVMS_STRUCT_VERSION_1: u32 = 1;
const SVMS_CAP_ISOLATED_OFFLINE_SESSIONS: u64 = 1 << 10;
const SVMS_CAP_OFFLINE_SOUNDFONT_STACK: u64 = 1 << 23;
pub(crate) const SVMS_MAX_SOUNDFONT_STACK: usize = 16;
pub(crate) const SVMS_SESSION_OFFLINE_RENDER: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct SVMSOfflineSessionConfig {
    pub struct_size: u32,
    pub struct_version: u32,
    pub session_kind: u32,
    pub flags: u32,
    pub sample_rate: u32,
    pub max_voices: u32,
    pub render_threads: u32,
    pub max_block_frames: u32,
    pub render_backend: u32,
    // Limiter is disabled (limiter_enabled = 0) but the values are still validated
    pub limiter_enabled: u32,
    pub limiter_algorithm: u32,
    pub master_volume: f32,
    pub limiter_threshold: f32,
    pub limiter_lookahead_ms: f32,
    pub limiter_attack_ms: f32,
    pub limiter_release_ms: f32,
    // Key shards: 0 or 1 = one core; an older runtime rejects nonzero
    pub shards: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct SVMSOfflineEvent {
    pub frame_offset: u32,
    pub packed_message: u32,
    pub reserved: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct SVMSOfflineTelemetry {
    pub struct_size: u32,
    pub struct_version: u32,
    pub output_frame: u64,
    pub rendered_frames: u64,
    pub submitted_events: u64,
    pub active_voices: u32,
    pub free_voices: u32,
    pub voice_steals: u32,
    pub sample_rate: u32,
    pub max_block_frames: u32,
    pub session_kind: u32,
    pub reserved: [u32; 2],
}

#[repr(C)]
pub(crate) struct SVMSOfflineSoundFontEntry {
    pub struct_size: u32,
    pub struct_version: u32,
    pub path_utf8: *const c_char,
    pub bank: i32,
    pub program: i32,
    pub reserved: [u32; 4],
}

const _: () = assert!(size_of::<SVMSOfflineSessionConfig>() == 80);
const _: () = assert!(size_of::<SVMSOfflineEvent>() == 16);
const _: () = assert!(size_of::<SVMSOfflineTelemetry>() == 64);

type UnusedFn = Option<unsafe extern "C" fn()>;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct SVMSInterface {
    pub struct_size: u32,
    pub struct_version: u32,
    pub abi_version: u32,
    pub reserved0: u32,
    pub capabilities: u64,
    pub product_major: u32,
    pub product_minor: u32,
    pub product_patch: u32,
    pub build_number: u32,
    pub create_session: UnusedFn,
    pub destroy_session: Option<unsafe extern "C" fn(SVMSSession) -> SVMSResult>,
    pub send_short: UnusedFn,
    pub send_short_at_qpc: UnusedFn,
    pub send_short_batch: UnusedFn,
    pub send_system_exclusive: UnusedFn,
    pub reset: Option<unsafe extern "C" fn(SVMSSession) -> SVMSResult>,
    pub get_telemetry: UnusedFn,
    pub get_runtime_clock: UnusedFn,
    pub send_timed_short_batch: UnusedFn,
    pub get_output_clock: UnusedFn,
    pub get_monotonic_clock: UnusedFn,
    pub set_ingress_mode: UnusedFn,
    pub get_queue_info: UnusedFn,
    pub load_soundfont_utf8: UnusedFn,
    pub panic: UnusedFn,
    pub create_offline_session: Option<
        unsafe extern "C" fn(
            *const SVMSOfflineSessionConfig,
            *const c_char,
            *mut SVMSSession,
        ) -> SVMSResult,
    >,
    pub render_offline: Option<
        unsafe extern "C" fn(
            SVMSSession,
            *const SVMSOfflineEvent,
            u32,
            *mut f32,
            *mut f32,
            u32,
        ) -> SVMSResult,
    >,
    pub get_offline_telemetry:
        Option<unsafe extern "C" fn(SVMSSession, *mut SVMSOfflineTelemetry) -> SVMSResult>,
    pub get_config_json: UnusedFn,
    pub patch_config_json: UnusedFn,
    pub get_config_path_utf8: UnusedFn,
    pub cancel_session_submissions: UnusedFn,
    pub create_realtime_session: UnusedFn,
    pub start_session_audio: UnusedFn,
    pub stop_session_audio: UnusedFn,
    pub send_runtime_command: UnusedFn,
    pub get_telemetry_v2: UnusedFn,
    pub enable_callback_trace: UnusedFn,
    pub read_callback_trace: UnusedFn,
    pub send_midi_stream: UnusedFn,
    pub send_short_batch_direct: UnusedFn,
    pub get_telemetry_v3: UnusedFn,
    pub set_live_params: UnusedFn,
    pub get_live_params: UnusedFn,
    pub get_schema_json: UnusedFn,
    pub get_param_value: UnusedFn,
    pub set_param_value: UnusedFn,
    pub get_metric_value: UnusedFn,
    pub run_command: UnusedFn,
    pub set_offline_soundfonts: UnusedFn,
    pub set_offline_channel_soundfont: UnusedFn,
    pub set_offline_soundfont_entries: Option<
        unsafe extern "C" fn(SVMSSession, *const SVMSOfflineSoundFontEntry, u32) -> SVMSResult,
    >,
}

/// The function pointers a usable runtime is guaranteed to provide.
#[derive(Clone, Copy)]
pub(crate) struct SVMSApi {
    pub destroy_session: unsafe extern "C" fn(SVMSSession) -> SVMSResult,
    pub reset: unsafe extern "C" fn(SVMSSession) -> SVMSResult,
    pub create_offline_session: unsafe extern "C" fn(
        *const SVMSOfflineSessionConfig,
        *const c_char,
        *mut SVMSSession,
    ) -> SVMSResult,
    pub render_offline: unsafe extern "C" fn(
        SVMSSession,
        *const SVMSOfflineEvent,
        u32,
        *mut f32,
        *mut f32,
        u32,
    ) -> SVMSResult,
    pub get_offline_telemetry:
        unsafe extern "C" fn(SVMSSession, *mut SVMSOfflineTelemetry) -> SVMSResult,
    pub set_offline_soundfont_entries: Option<
        unsafe extern "C" fn(SVMSSession, *const SVMSOfflineSoundFontEntry, u32) -> SVMSResult,
    >,
}

define_lib_wrapper!(SVMSSharedLib, {
    SVMS_GetInterface: unsafe extern "C" fn(u32, u32, *mut SVMSInterface) -> SVMSResult,
});

impl SVMSSharedLib {
    /// Fetches the ABI-1 function table, or why this runtime cannot be used.
    pub(crate) fn interface(&self) -> Result<SVMSInterface, String> {
        let size = size_of::<SVMSInterface>() as u32;
        let mut table = std::mem::MaybeUninit::<SVMSInterface>::zeroed();
        let res = unsafe { (self.SVMS_GetInterface)(SVMS_ABI_VERSION_1, size, table.as_mut_ptr()) };
        if res != SVMS_RESULT_OK {
            return Err(format!("SVMS_GetInterface failed with code {res}"));
        }

        // All-zero is valid for every field (integers and Option<fn>)
        let table = unsafe { table.assume_init() };
        if table.abi_version != SVMS_ABI_VERSION_1 {
            return Err(format!(
                "Unsupported SVMS ABI version {}",
                table.abi_version
            ));
        }
        if table.capabilities & SVMS_CAP_ISOLATED_OFFLINE_SESSIONS == 0 {
            return Err(
                "This SuperVirtualMIDISynth runtime does not support offline \
                        sessions (V3 or newer required)"
                    .to_string(),
            );
        }

        Ok(table)
    }

    pub(crate) fn api(&self) -> Result<SVMSApi, String> {
        let table = self.interface()?;
        let missing =
            || "The SuperVirtualMIDISynth runtime is missing required functions".to_string();

        Ok(SVMSApi {
            destroy_session: table.destroy_session.ok_or_else(missing)?,
            reset: table.reset.ok_or_else(missing)?,
            create_offline_session: table.create_offline_session.ok_or_else(missing)?,
            render_offline: table.render_offline.ok_or_else(missing)?,
            get_offline_telemetry: table.get_offline_telemetry.ok_or_else(missing)?,
            set_offline_soundfont_entries: table
                .set_offline_soundfont_entries
                .filter(|_| table.capabilities & SVMS_CAP_OFFLINE_SOUNDFONT_STACK != 0),
        })
    }
}

impl SVMSInterface {
    pub(crate) fn version(&self) -> String {
        format!(
            "{}.{}.{}.{}",
            self.product_major, self.product_minor, self.product_patch, self.build_number
        )
    }
}

pub(crate) struct SVMSLib {
    _fns: SVMSSharedLib,
    pub api: SVMSApi,
    soundfonts: RwLock<Arena<SVMSFont>>,
}

#[derive(Clone)]
pub(crate) struct SVMSFont {
    pub path: CString,
    pub bank: i32,
    pub program: i32,
}

impl SVMSLib {
    // SVMS keeps render worker threads and process-wide session slots, so it
    // is never unloaded once used.
    fn pin_shared_object() {
        static PINNED: OnceLock<()> = OnceLock::new();

        PINNED.get_or_init(|| {
            if let Ok((lib, _)) = crate::paths::load_library(SVMS_LIB_FILENAME) {
                std::mem::forget(lib);
            }
        });
    }

    pub fn load() -> Result<Arc<Self>, RendererError> {
        Self::pin_shared_object();

        let fns = unsafe { SVMSSharedLib::new(SVMS_LIB_FILENAME) }.map_err(|e| {
            RendererError::SynthInit(format!(
                "The SuperVirtualMIDISynth synthesizer requires the '{SVMS_LIB_FILENAME}' \
                 shared library, which could not be loaded. \n\nDetails: {e}"
            ))
        })?;
        let api = fns.api().map_err(RendererError::SynthInit)?;

        Ok(Arc::new(Self {
            _fns: fns,
            api,
            soundfonts: RwLock::new(Arena::new()),
        }))
    }

    pub fn get_soundfont(&self, handle: &SoundFontHandle) -> Option<SVMSFont> {
        self.soundfonts.read().unwrap().get(*handle).cloned()
    }
}

impl SynthLibrary for SVMSLib {
    fn load_soundfont_handle(
        &self,
        config: &SynthConfig,
        soundfont: &SoundFont,
    ) -> Result<SoundFontHandle, RendererError> {
        if !matches!(config, SynthConfig::SVMS(_)) {
            return Err(RendererError::ConfigMismatch);
        }

        if !soundfont.path.is_file() {
            return Err(RendererError::SoundFont(format!(
                "SoundFont file not found: {:?}",
                soundfont.path
            )));
        }

        let path = soundfont
            .path
            .to_str()
            .and_then(|p| CString::new(p).ok())
            .ok_or_else(|| {
                RendererError::SoundFont(format!("Invalid SoundFont path: {:?}", soundfont.path))
            })?;

        // SVMS loads the fonts itself when a session is created, it has no
        // shared font pool. An entry only answers the bank/program it is
        // pinned to; the default bank 0 with no preset pins nothing.
        let pinned = soundfont.bank > 0 || soundfont.preset >= 0;
        let font = SVMSFont {
            path,
            bank: if pinned {
                soundfont.bank.max(0) as i32
            } else {
                -1
            },
            program: soundfont.preset.max(-1) as i32,
        };

        Ok(self.soundfonts.write().unwrap().insert(font))
    }

    fn free_soundfont_handle(&self, handle: SoundFontHandle) {
        let _ = self.soundfonts.write().unwrap().remove(handle);
    }

    fn supported_soundfont_types(&self) -> &'static [SoundFontType] {
        &[SoundFontType::Sf2, SoundFontType::Sfz]
    }
}
