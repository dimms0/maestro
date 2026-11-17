#![allow(non_snake_case)]

mod ffi;
mod register;
mod state;

use ffi::*;
use state::get_state;

use crate::{log_warn, ring};

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
    match uMsg {
        MODM_DATA => {
            if dwUser == 0 {
                return MMSYSERR_INVALHANDLE;
            }
            let client = unsafe { &*(dwUser as *const state::OutClient) };
            client.ring.push_short(dwParam1 as u32);
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
                if data.len() > ring::MAX_LONG {
                    return MMSYSERR_INVALPARAM;
                }
                client.ring.push_long(data);

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

        MODM_GETNUMDEVS => get_state().num_output_devices(),

        MODM_GETDEVCAPS => {
            if dwParam1 == 0 {
                return MMSYSERR_INVALPARAM;
            }
            let caps = get_state().output_caps(uDeviceID);
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

            let client = match get_state().open_output(uDeviceID, desc, dwParam2 as u32) {
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
            get_state().close_output(&client);
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
            client.ring.push_reset();
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

pub(crate) fn log_driver_warn(context: &str, err: impl std::fmt::Display) {
    log_warn!("[driver] {context}: {err}");
}
