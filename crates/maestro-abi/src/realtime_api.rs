use std::path::PathBuf;
use std::process::Command;

use maestro_core::{
    paths,
    realtime::{MAX_SCHEDULE_SECS, host_time},
    system_cfg::MaestroConfigManager,
};

use crate::abi::*;
use crate::error::{Failure, err_guard};
use crate::realtime::{Realtime, Shared};
use crate::sized;

pub static REALTIME_V1: MaestroRealtimeV1 = MaestroRealtimeV1 {
    open,
    close,
    now,
    ns_to_ticks,
    send_short,
    send_long,
    send_ump,
    get_info,
    get_stats,
    reset,
    notes_off,
    show_settings,
};

unsafe extern "C" fn open(
    app: *const MaestroAppInfo,
    params: *const MaestroRealtimeParams,
    out: *mut *mut MaestroRealtime,
) -> MaestroResult {
    err_guard(|| {
        if out.is_null() {
            return Err(Failure::invalid("out is NULL"));
        }
        let realtime = unsafe { Realtime::open(app, params) }?;
        unsafe { *out = Box::into_raw(Box::new(realtime)).cast() };
        Ok(())
    })
}

unsafe extern "C" fn close(rt: *mut MaestroRealtime) {
    if !rt.is_null() {
        let _ = err_guard(|| {
            drop(unsafe { Box::from_raw(rt.cast::<Realtime>()) });
            Ok(())
        });
    }
}

unsafe extern "C" fn now() -> MaestroTime {
    host_time::host_now()
}

unsafe extern "C" fn ns_to_ticks(ns: u64) -> MaestroTime {
    host_time::ns_to_ticks(ns)
}

unsafe fn shared<'a>(rt: *mut MaestroRealtime) -> Result<&'a Shared, Failure> {
    unsafe { rt.cast::<Realtime>().as_ref() }
        .map(|instance| &*instance.shared)
        .ok_or_else(|| Failure::invalid("rt is NULL"))
}

unsafe extern "C" fn send_short(
    rt: *mut MaestroRealtime,
    port: u8,
    msg: u32,
    at: MaestroTime,
) -> MaestroResult {
    err_guard(|| {
        unsafe { shared(rt) }?.with_sender(Some(port), |sender| sender.process_short(port, msg, at))
    })
}

unsafe extern "C" fn send_long(
    rt: *mut MaestroRealtime,
    port: u8,
    data: *const u8,
    len: usize,
    at: MaestroTime,
) -> MaestroResult {
    err_guard(|| {
        if data.is_null() || len == 0 {
            return Err(Failure::invalid("data is empty"));
        }
        let data = unsafe { std::slice::from_raw_parts(data, len) };
        unsafe { shared(rt) }?.with_sender(Some(port), |sender| sender.process_long(port, data, at))
    })
}

unsafe extern "C" fn send_ump(
    rt: *mut MaestroRealtime,
    words: *const u32,
    count: usize,
    at: MaestroTime,
) -> MaestroResult {
    err_guard(|| {
        if words.is_null() || count == 0 {
            return Err(Failure::invalid("words is empty"));
        }
        let words = unsafe { std::slice::from_raw_parts(words, count) };
        unsafe { shared(rt) }?.with_sender(None, |sender| sender.process_ump(words, at))
    })
}

unsafe extern "C" fn reset(rt: *mut MaestroRealtime) -> MaestroResult {
    err_guard(|| {
        let shared = unsafe { shared(rt) }?;
        let mut engine = shared.engine.write().unwrap();
        let live = engine.as_mut().ok_or_else(Failure::not_running)?;
        live.engine.reset();
        if let Some(sender) = shared.sender() {
            sender.reset();
        }
        Ok(())
    })
}

unsafe extern "C" fn notes_off(rt: *mut MaestroRealtime) -> MaestroResult {
    err_guard(|| unsafe { shared(rt) }?.with_sender(None, |sender| sender.notes_off()))
}

fn gui_exe() -> PathBuf {
    let name = if cfg!(windows) {
        "maestro.exe"
    } else {
        "maestro"
    };
    paths::module_dir()
        .into_iter()
        .flat_map(|dir| [Some(dir.join(name)), dir.parent().map(|p| p.join(name))])
        .flatten()
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

unsafe extern "C" fn show_settings(rt: *mut MaestroRealtime) -> MaestroResult {
    err_guard(|| {
        let shared = unsafe { shared(rt) }?;
        let profile = MaestroConfigManager::default().get_app_config_path(&shared.app_id);
        let mut child = Command::new(gui_exe())
            .arg("edit-config")
            .arg(profile)
            .spawn()
            .map_err(|e| {
                Failure::new(
                    MAESTRO_ERR_UNAVAILABLE,
                    format!("Maestro's settings could not be opened: {e}"),
                )
            })?;
        std::thread::spawn(move || child.wait());
        Ok(())
    })
}

unsafe extern "C" fn get_info(
    rt: *mut MaestroRealtime,
    out: *mut MaestroRealtimeInfo,
) -> MaestroResult {
    err_guard(|| {
        let shared = unsafe { shared(rt) }?;
        let config = shared.config.lock().unwrap().clone();
        let sample_rate = shared
            .engine
            .read()
            .unwrap()
            .as_ref()
            .map_or(config.audio_params.sample_rate, |live| {
                live.engine.stream_params().sample_rate
            });
        let render_us = (config.realtime.render_buffer_ms * 1000.0) as u32;
        unsafe {
            sized::write(out, |struct_size| MaestroRealtimeInfo {
                struct_size,
                sample_rate,
                num_ports: u32::from(shared.ports),
                midi2: u32::from(shared.midi2),
                render_block_us: render_us,
                max_schedule_us: (MAX_SCHEDULE_SECS * 1_000_000) as u32,
            })
        }
    })
}

unsafe extern "C" fn get_stats(
    rt: *mut MaestroRealtime,
    out: *mut MaestroRealtimeStats,
) -> MaestroResult {
    err_guard(|| {
        let shared = unsafe { shared(rt) }?;
        let engine = shared.engine.read().unwrap();
        let live = engine.as_ref().ok_or_else(Failure::not_running)?;
        unsafe {
            sized::write(out, |struct_size| MaestroRealtimeStats {
                struct_size,
                render_time: live.stats.get_renderer().get_average_render_time(),
                latency_us: live.engine.latency().as_micros() as u32,
                reserved: 0,
                voice_count: live.stats.get_renderer().read_voice_count(),
                nps: live.stats.read_nps(),
                eps: live.stats.read_eps(),
                nps_limit: live.stats.read_nps_limit(),
            })
        }
    })
}
