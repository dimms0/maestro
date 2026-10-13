use std::ffi::{CStr, c_void};
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use maestro_core::{paths, realtime::host_time, system_cfg::MaestroConfigManager};

use crate::abi::*;
use crate::error::{Failure, err_guard};
use crate::instance::{Instance, Shared};

static MIDI1: MaestroExtMidi1 = MaestroExtMidi1 {
    send_short: midi1_send_short,
    send_long: midi1_send_long,
};

static UMP: MaestroExtUmp = MaestroExtUmp { send: ump_send };

static CONTROL: MaestroExtControl = MaestroExtControl {
    reset: control_reset,
    show_settings: control_show_settings,
};

static INFO: MaestroExtInfo = MaestroExtInfo { get: info_get };

static STATS: MaestroExtStats = MaestroExtStats { get: stats_get };

pub fn table(id: &CStr) -> *const c_void {
    fn erase<T>(table: &'static T) -> *const c_void {
        (table as *const T).cast()
    }

    match id {
        id if id == MAESTRO_EXT_MIDI1 => erase(&MIDI1),
        id if id == MAESTRO_EXT_UMP => erase(&UMP),
        id if id == MAESTRO_EXT_CONTROL => erase(&CONTROL),
        id if id == MAESTRO_EXT_INFO => erase(&INFO),
        id if id == MAESTRO_EXT_STATS => erase(&STATS),
        _ => std::ptr::null(),
    }
}

unsafe fn shared<'a>(inst: *mut MaestroInstance) -> Result<&'a Shared, Failure> {
    unsafe { inst.cast::<Instance>().as_ref() }
        .map(|instance| &*instance.shared)
        .ok_or_else(|| Failure::invalid("inst is NULL"))
}

fn timestamp(at: MaestroTime) -> Option<Instant> {
    (at != MAESTRO_TIME_NOW).then(|| host_time::from_host_ticks(at))
}

/// Copies `value` into a caller's struct, as far as its `struct_size` allows.
unsafe fn write_sized<T>(out: *mut T, value: impl FnOnce(u32) -> T) -> Result<(), Failure> {
    if out.is_null() {
        return Err(Failure::invalid("out is NULL"));
    }
    let size = unsafe { out.cast::<u32>().read_unaligned() };
    if (size as usize) < size_of::<u32>() {
        return Err(Failure::invalid("out->struct_size is not set"));
    }
    let value = value(size);
    let len = (size as usize).min(size_of::<T>());
    unsafe {
        std::ptr::copy_nonoverlapping((&value as *const T).cast::<u8>(), out.cast::<u8>(), len)
    };
    Ok(())
}

unsafe extern "C" fn midi1_send_short(
    inst: *mut MaestroInstance,
    port: u8,
    msg: u32,
    at: MaestroTime,
) -> MaestroResult {
    err_guard(|| {
        unsafe { shared(inst) }?.with_sender(Some(port), |sender| {
            sender.process_short(port, msg, timestamp(at))
        })
    })
}

unsafe extern "C" fn midi1_send_long(
    inst: *mut MaestroInstance,
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
        unsafe { shared(inst) }?.with_sender(Some(port), |sender| {
            sender.process_long(port, data, timestamp(at))
        })
    })
}

unsafe extern "C" fn ump_send(
    inst: *mut MaestroInstance,
    words: *const u32,
    count: usize,
    at: MaestroTime,
) -> MaestroResult {
    err_guard(|| {
        if words.is_null() || count == 0 {
            return Err(Failure::invalid("words is empty"));
        }
        let words = unsafe { std::slice::from_raw_parts(words, count) };
        unsafe { shared(inst) }?
            .with_sender(None, |sender| sender.process_ump(words, timestamp(at)))
    })
}

unsafe extern "C" fn control_reset(inst: *mut MaestroInstance) -> MaestroResult {
    err_guard(|| {
        let shared = unsafe { shared(inst) }?;
        let mut engine = shared.engine.write().unwrap();
        let live = engine.as_mut().ok_or_else(Failure::not_running)?;
        live.engine.reset();
        if let Some(sender) = shared.sender.load().as_deref() {
            sender.reset();
        }
        Ok(())
    })
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

unsafe extern "C" fn control_show_settings(inst: *mut MaestroInstance) -> MaestroResult {
    err_guard(|| {
        let shared = unsafe { shared(inst) }?;
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

unsafe extern "C" fn info_get(inst: *mut MaestroInstance, out: *mut MaestroInfo) -> MaestroResult {
    err_guard(|| {
        let shared = unsafe { shared(inst) }?;
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
            write_sized(out, |struct_size| MaestroInfo {
                struct_size,
                sample_rate,
                num_ports: u32::from(shared.ports),
                midi2: u32::from(shared.midi2),
                render_block_us: render_us,
            })
        }
    })
}

unsafe extern "C" fn stats_get(
    inst: *mut MaestroInstance,
    out: *mut MaestroStats,
) -> MaestroResult {
    err_guard(|| {
        let shared = unsafe { shared(inst) }?;
        let engine = shared.engine.read().unwrap();
        let live = engine.as_ref().ok_or_else(Failure::not_running)?;
        unsafe {
            write_sized(out, |struct_size| MaestroStats {
                struct_size,
                voice_count: live.stats.get_renderer().read_voice_count(),
                render_time: live.stats.get_renderer().get_average_render_time(),
                nps: live.stats.read_nps(),
                eps: live.stats.read_eps(),
                nps_limit: live.stats.read_nps_limit(),
            })
        }
    })
}
