use std::ffi::{CStr, c_char, c_void};

mod abi;
mod app;
mod error;
mod file_render;
mod profile;
mod realtime;
mod realtime_api;
mod sized;
mod stats;

use abi::*;

static CORE_V1: MaestroCoreV1 = MaestroCoreV1 {
    get_version,
    last_error,
};

const fn parse(digits: &str) -> u32 {
    let digits = digits.as_bytes();
    let mut value = 0;
    let mut i = 0;
    while i < digits.len() {
        value = value * 10 + (digits[i] - b'0') as u32;
        i += 1;
    }
    value
}

unsafe extern "C" fn get_version() -> u32 {
    const VERSION: u32 =
        parse(env!("CARGO_PKG_VERSION_MAJOR")) << 16 | parse(env!("CARGO_PKG_VERSION_MINOR"));
    VERSION
}

unsafe extern "C" fn last_error() -> *const c_char {
    error::last()
}

/// # Safety
/// `id` must be NULL or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maestro_get_interface(id: *const c_char) -> *const c_void {
    fn erase<T>(table: &'static T) -> *const c_void {
        (table as *const T).cast()
    }

    if id.is_null() {
        return std::ptr::null();
    }
    match unsafe { CStr::from_ptr(id) } {
        id if id == MAESTRO_CORE_V1 => erase(&CORE_V1),
        id if id == MAESTRO_REALTIME_V1 => erase(&realtime_api::REALTIME_V1),
        id if id == MAESTRO_FILE_RENDER_V1 => erase(&file_render::FILE_RENDER_V1),
        _ => std::ptr::null(),
    }
}
