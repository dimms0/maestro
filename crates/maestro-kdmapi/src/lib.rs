#![allow(static_mut_refs)]
#![allow(non_snake_case)]

use native_dialog::DialogBuilder;
use std::{ffi::c_void, sync::atomic::Ordering};

use crate::config::MaestroKdmapiConfig;
use crate::state::get_state;
use maestro_core::{
    realtime::{MaestroRealtimeEngine, RealtimeEngineOptions},
    system_cfg::{ConfigComponent, MaestroConfigManager},
};

mod config;
mod platform;
mod state;
mod watcher;

fn log_error<T: std::fmt::Display>(err: T) {
    let errstr = format!("{err}");

    std::thread::spawn(|| {
        let _ = DialogBuilder::message()
            .set_level(native_dialog::MessageLevel::Error)
            .set_title("Maestro KDMAPI")
            .set_text(errstr)
            .alert()
            .show();
    });

    eprintln!("[Maestro KDMAPI] Error: {err}");
}

fn log_info<T: std::fmt::Display>(msg: T) {
    eprintln!("[Maestro KDMAPI] {msg}");
}

fn load_config() -> Result<MaestroKdmapiConfig, maestro_core::error::ConfigError> {
    let config_manager = MaestroConfigManager::default();
    let path = config_manager.get_component_config_path(&ConfigComponent::KDMAPI);
    config_manager.load_or_default(&path)
}

fn start_engine(config: &MaestroKdmapiConfig) -> bool {
    let config_manager = MaestroConfigManager::default();
    let sflist = match config_manager.get_soundfont_list(&config.sflist) {
        Ok(s) => s,
        Err(err) => {
            log_error(err);
            return false;
        }
    };

    let options = RealtimeEngineOptions {
        ports: None,
        config: config.realtime.clone(),
        renderer: config.renderer,
        audio_params: config.audio_params,
        event_processor: config.event_processor.clone(),
        post_processor: config.post_processor,
    };

    let engine = match MaestroRealtimeEngine::new(options) {
        Ok(e) => e,
        Err(err) => {
            log_error(err);
            return false;
        }
    };

    if let Err(err) = engine.set_soundfonts(sflist) {
        log_error(err);
    }

    let state = get_state();
    *state.statistics.lock().unwrap() = Some(engine.get_statistics());

    {
        let sender = engine.get_event_sender();
        let boxed = Box::new(sender);
        let ptr = Box::into_raw(boxed);
        let old = state.sender.swap(ptr, Ordering::AcqRel);
        if !old.is_null() {
            unsafe {
                let _ = Box::from_raw(old);
            }
        }
    }

    *state.realtime.lock().unwrap() = Some(engine);

    true
}

fn stop_engine() {
    let state = get_state();
    *state.realtime.lock().unwrap() = None;
    let old = state.sender.swap(std::ptr::null_mut(), Ordering::AcqRel);
    if !old.is_null() {
        unsafe {
            let _ = Box::from_raw(old);
        }
    }
    *state.statistics.lock().unwrap() = None;
}

pub(crate) fn send_long(data: &[u8]) -> bool {
    let state = get_state();
    let sender = state.sender.load(Ordering::Acquire);
    if sender.is_null() {
        return false;
    }

    unsafe {
        (*sender).process_long(0, data, None);
    }

    true
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn ReturnKDMAPIVer(
    Major: *mut u32,
    Minor: *mut u32,
    Build: *mut u32,
    Revision: *mut u32,
) -> u32 {
    unsafe {
        *Major = 4;
        *Minor = 1;
        *Build = 0;
        *Revision = 5;
    }

    1
}

#[unsafe(no_mangle)]
pub extern "C" fn IsKDMAPIAvailable() -> i32 {
    match load_config() {
        Ok(config) => {
            if config.enabled {
                1
            } else {
                0
            }
        }
        Err(err) => {
            log_error(err);
            1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn InitializeKDMAPIStream() -> i32 {
    let state = get_state();
    {
        let realtime = state.realtime.lock().unwrap();
        if realtime.is_some() {
            return 1;
        }
    }

    let config = match load_config() {
        Ok(c) => c,
        Err(err) => {
            log_error(err);
            Default::default()
        }
    };

    if !config.enabled {
        return 0;
    }

    if !start_engine(&config) {
        return 0;
    }

    *state.watcher.lock().unwrap() = watcher::spawn(config);

    1
}

#[unsafe(no_mangle)]
pub extern "C" fn TerminateKDMAPIStream() -> i32 {
    let state = get_state();

    // The watcher must go down first so the reload thread cannot resurrect
    // (or race the teardown of) the engine below.
    let handle = state.watcher.lock().unwrap().take();
    if let Some(handle) = handle {
        handle.shutdown();
    }

    if state.realtime.lock().unwrap().is_none() {
        return 1;
    }

    stop_engine();

    1
}

#[unsafe(no_mangle)]
pub extern "C" fn ResetKDMAPIStream() {
    let state = get_state();
    if let Some(synth) = state.realtime.lock().unwrap().as_mut() {
        synth.reset();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn SendCustomEvent(_eventtype: u32, _chan: u32, _param: u32) -> u32 {
    // Not supported
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn SendDirectData(short: u32) {
    let state = get_state();
    let ptr = state.sender.load(Ordering::Acquire);
    if !ptr.is_null() {
        unsafe {
            (*ptr).process_short(0, short, None);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn SendDirectDataNoBuf(short: u32) {
    SendDirectData(short);
}

#[unsafe(no_mangle)]
pub extern "C" fn GetVoiceCount() -> u64 {
    let state = get_state();
    if let Some(stats) = state.statistics.lock().unwrap().as_ref() {
        stats.read_voice_count()
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn GetRenderingTime() -> f32 {
    let state = get_state();
    if let Some(stats) = state.statistics.lock().unwrap().as_ref() {
        stats.get_average_render_time()
    } else {
        0.0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn GetDriverDebugInfo() -> *mut c_void {
    std::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn DriverSettings(
    _dwparam: u32,
    _dwcmd: u32,
    _lpvalue: *mut c_void,
    _cbsize: u32,
) -> u32 {
    // Not supported
    0
}

// Presence of this export is how the GUI tells a Maestro-installed OmniMIDI
// library apart from one belonging to the OmniMIDI project itself.
#[unsafe(no_mangle)]
pub extern "C" fn Maestro_KDMAPI_Version() -> u32 {
    let part = |s: Option<&str>| s.and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let mut it = env!("CARGO_PKG_VERSION").split('.');
    (part(it.next()) << 16) | (part(it.next()) << 8) | part(it.next())
}
