use std::ffi::{CStr, c_char};
use std::mem::MaybeUninit;

use crate::error::Failure;

/// Reads a caller's struct as far as its `struct_size` reaches; the rest, or
/// all of it for NULL, reads as 0.
///
/// # Safety
/// `ptr` must be NULL or point at least `struct_size` readable bytes, and `T`
/// must be valid when zeroed.
pub unsafe fn read<T>(ptr: *const T) -> T {
    let mut value = MaybeUninit::<T>::zeroed();
    if !ptr.is_null() {
        let size = unsafe { ptr.cast::<u32>().read_unaligned() } as usize;
        unsafe {
            std::ptr::copy_nonoverlapping(
                ptr.cast::<u8>(),
                value.as_mut_ptr().cast::<u8>(),
                size.min(size_of::<T>()),
            )
        };
    }
    unsafe { value.assume_init() }
}

/// # Safety
/// `out` must be NULL or point at least `struct_size` writable bytes.
pub unsafe fn write<T>(out: *mut T, value: impl FnOnce(u32) -> T) -> Result<(), Failure> {
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

pub fn reserved(value: u32) -> Result<(), Failure> {
    match value {
        0 => Ok(()),
        _ => Err(Failure::unsupported("a reserved field is not 0")),
    }
}

/// # Safety
/// `ptr` must be NULL or a NUL-terminated string.
pub unsafe fn string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .ok()
        .map(str::to_string)
}
