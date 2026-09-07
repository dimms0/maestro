#![allow(non_snake_case)]

mod ffi;
mod register;
mod state;

use ffi::*;
use state::get_state;

use crate::{log_error, log_warn};

fn callback_flags(open_flags: u32) -> u32 {
    open_flags >> 16
}

#[unsafe(no_mangle)]
pub extern "system" fn DriverProc(
    dwDriverId: usize,
    hdrvr: usize,
    uMsg: u32,
    lParam1: isize,
    lParam2: isize,
) -> isize {
    match uMsg {
        DRV_LOAD | DRV_FREE | DRV_OPEN | DRV_CLOSE | DRV_ENABLE | DRV_DISABLE | DRV_INSTALL
        | DRV_REMOVE => 1,
        DRV_QUERYCONFIGURE => 0,
        _ => unsafe { DefDriverProc(dwDriverId, hdrvr, uMsg, lParam1, lParam2) },
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn modMessage(
    uDeviceID: u32,
    uMsg: u32,
    dwUser: usize,
    dwParam1: usize,
    dwParam2: usize,
) -> u32 {
    let state = get_state();

    match uMsg {
        MODM_DATA => {
            if dwUser == 0 {
                return MMSYSERR_INVALHANDLE;
            }
            let client = unsafe { &*(dwUser as *const state::OutClient) };
            let short = dwParam1 as u32;

            let guard = state.sender.load();
            if let Some(snd) = guard.as_ref() {
                snd.process_short(client.port, short, None);
            }
            MMSYSERR_NOERROR
        }

        MODM_LONGDATA => {
            if dwUser == 0 {
                return MMSYSERR_INVALHANDLE;
            }
            let header = dwParam1 as *mut MIDIHDR;
            if header.is_null() {
                return MMSYSERR_INVALPARAM;
            }
            let client = unsafe { &*(dwUser as *const state::OutClient) };

            unsafe {
                if (*header).dwFlags & MHDR_PREPARED == 0 {
                    return MIDIERR_UNPREPARED;
                }
                let data = std::slice::from_raw_parts(
                    (*header).lpData as *const u8,
                    (*header).dwBufferLength as usize,
                );

                let guard = state.sender.load();
                if let Some(snd) = guard.as_ref() {
                    snd.process_long(client.port, data, None);
                }

                (*header).dwFlags |= MHDR_DONE;
                (*header).dwFlags &= !MHDR_INQUEUE;
                DriverCallback(
                    client.callback,
                    callback_flags(client.flags),
                    client.hmidi,
                    MOM_DONE,
                    client.instance,
                    header as usize,
                    0,
                );
            }
            MMSYSERR_NOERROR
        }

        MODM_INIT | DRVM_EXIT => MMSYSERR_NOERROR,

        MODM_GETNUMDEVS => state.num_output_devices(),

        MODM_GETDEVCAPS => {
            if dwParam1 == 0 {
                return MMSYSERR_INVALPARAM;
            }
            let caps = state.output_caps(uDeviceID);
            let Some(caps) = caps else {
                return MMSYSERR_BADDEVICEID;
            };
            unsafe {
                copy_caps(&caps, dwParam1 as *mut u8, dwParam2);
            }
            MMSYSERR_NOERROR
        }

        MODM_OPEN => {
            let desc = dwParam1 as *const MIDIOPENDESC;
            if desc.is_null() || dwUser == 0 {
                return MMSYSERR_INVALPARAM;
            }
            let desc = unsafe { &*desc };

            let client = match state.open_output(uDeviceID, desc, dwParam2 as u32) {
                Ok(c) => c,
                Err(code) => return code,
            };

            let (callback, flags, hmidi, instance) =
                (client.callback, client.flags, client.hmidi, client.instance);
            unsafe {
                *(dwUser as *mut usize) = Box::into_raw(client) as usize;
                DriverCallback(
                    callback,
                    callback_flags(flags),
                    hmidi,
                    MOM_OPEN,
                    instance,
                    0,
                    0,
                );
            }
            MMSYSERR_NOERROR
        }

        MODM_CLOSE => {
            if dwUser == 0 {
                return MMSYSERR_INVALHANDLE;
            }
            let client = unsafe { Box::from_raw(dwUser as *mut state::OutClient) };
            state.close_output();
            unsafe {
                DriverCallback(
                    client.callback,
                    callback_flags(client.flags),
                    client.hmidi,
                    MOM_CLOSE,
                    client.instance,
                    0,
                    0,
                );
            }
            MMSYSERR_NOERROR
        }

        MODM_RESET => {
            if dwUser == 0 {
                return MMSYSERR_INVALHANDLE;
            }
            let client = unsafe { &*(dwUser as *const state::OutClient) };
            let guard = state.sender.load();
            if let Some(snd) = guard.as_ref() {
                snd.process_short(client.port, 0xFF, None);
            }
            MMSYSERR_NOERROR
        }

        // winmm performs the generic bookkeeping itself when the driver
        // reports these as unsupported
        MODM_PREPARE
        | MODM_UNPREPARE
        | MODM_CACHEPATCHES
        | MODM_CACHEDRUMPATCHES
        | MODM_GETVOLUME
        | MODM_SETVOLUME => MMSYSERR_NOTSUPPORTED,

        _ => MMSYSERR_NOTSUPPORTED,
    }
}

unsafe fn copy_caps<T>(caps: &T, dst: *mut u8, dst_size: usize) {
    let size = std::mem::size_of::<T>().min(dst_size);
    unsafe {
        std::ptr::copy_nonoverlapping(caps as *const T as *const u8, dst, size);
    }
}

pub(crate) fn log_driver_error(context: &str, err: impl std::fmt::Display) {
    log_error!("[driver] {context}: {err}");
}

pub(crate) fn log_driver_warn(context: &str, err: impl std::fmt::Display) {
    log_warn!("[driver] {context}: {err}");
}
