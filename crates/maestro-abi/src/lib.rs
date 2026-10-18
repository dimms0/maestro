use std::ffi::{CStr, c_char, c_void};

use maestro_core::realtime::host_time;

mod abi;
mod error;
mod ext;
mod instance;
mod profile;
mod stats;

use abi::*;
use error::{Failure, err_guard};
use instance::Instance;

#[unsafe(no_mangle)]
pub extern "C" fn maestro_get_version() -> u32 {
    MAESTRO_API_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn maestro_last_error() -> *const c_char {
    error::last()
}

#[unsafe(no_mangle)]
pub extern "C" fn maestro_now() -> MaestroTime {
    host_time::host_now()
}

#[unsafe(no_mangle)]
pub extern "C" fn maestro_ns_to_ticks(ns: u64) -> MaestroTime {
    host_time::ns_to_ticks(ns)
}

/// # Safety
/// `params` must point at a `MaestroOpenParams` of at least its `struct_size`
/// bytes, and `out` at writable storage for a pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maestro_open(
    api_version: u32,
    params: *const MaestroOpenParams,
    out: *mut *mut MaestroInstance,
) -> MaestroResult {
    err_guard(|| {
        if out.is_null() {
            return Err(Failure::invalid("out is NULL"));
        }
        let instance = unsafe { Instance::open(api_version, params) }?;
        unsafe { *out = Box::into_raw(Box::new(instance)).cast() };
        Ok(())
    })
}

/// # Safety
/// `inst` must be NULL or an instance from `maestro_open` that is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maestro_close(inst: *mut MaestroInstance) {
    if !inst.is_null() {
        let _ = err_guard(|| {
            drop(unsafe { Box::from_raw(inst.cast::<Instance>()) });
            Ok(())
        });
    }
}

/// # Safety
/// `inst` must be an open instance and `id` NULL or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maestro_get_extension(
    inst: *mut MaestroInstance,
    id: *const c_char,
) -> *const c_void {
    if inst.is_null() || id.is_null() {
        return std::ptr::null();
    }
    ext::table(unsafe { CStr::from_ptr(id) })
}
