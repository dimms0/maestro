use std::cell::RefCell;
use std::ffi::{CString, c_char};
use std::fmt::Display;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::abi::*;

thread_local! {
    static LAST: RefCell<CString> = RefCell::new(CString::default());
}

pub fn last() -> *const c_char {
    LAST.with(|last| last.borrow().as_ptr())
}

fn set(message: &str) {
    let message = CString::new(message.replace('\0', "")).unwrap_or_default();
    LAST.with(|last| *last.borrow_mut() = message);
}

pub struct Failure {
    code: MaestroResult,
    message: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl Failure {
    pub fn new(code: MaestroResult, message: impl Display) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }

    pub fn invalid(message: impl Display) -> Self {
        Self::new(MAESTRO_ERR_INVALID_ARG, message)
    }

    pub fn not_running() -> Self {
        Self::new(MAESTRO_ERR_NOT_RUNNING, "The engine is not running")
    }
}

pub fn err_guard(call: impl FnOnce() -> Result<(), Failure>) -> MaestroResult {
    match catch_unwind(AssertUnwindSafe(call)) {
        Ok(Ok(())) => MAESTRO_OK,
        Ok(Err(failure)) => {
            set(&failure.message);
            failure.code
        }
        Err(panic) => {
            let message = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            set(&format!("Internal error: {message}"));
            MAESTRO_ERR_INTERNAL
        }
    }
}
