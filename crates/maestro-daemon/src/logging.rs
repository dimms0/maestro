//! Logging for both things this crate builds: `maestrod`, a background service
//! with nobody watching it, and `maestrodrv.dll`, which is loaded into a host
//! application's process where stderr usually goes nowhere.
//!
//! Those two want opposite things from a warning. The DLL has no other channel,
//! so a modal alert is the only way to reach the user. The daemon has a log the
//! supervisor captures, and popping an unthrottled modal dialog from a service
//! is wrong — a warning that repeats would stack windows over whatever the user
//! is actually doing. So the daemon opts into notification mode at startup:
//! errors become a single desktop notification, warnings go to the log only.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use native_dialog::MessageLevel;

const MODE_DIALOG: u8 = 0;
const MODE_NOTIFY: u8 = 1;

static MODE: AtomicU8 = AtomicU8::new(MODE_DIALOG);

pub fn use_notifications() {
    MODE.store(MODE_NOTIFY, Ordering::Relaxed);
}

#[doc(hidden)]
pub fn write_log(level: MessageLevel, args: std::fmt::Arguments) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    eprintln!("[maestrod {secs} {level:?}] {args}");

    match (MODE.load(Ordering::Relaxed), level) {
        (MODE_NOTIFY, MessageLevel::Error) => {
            maestro_core::notify::send("Maestro", &format!("{args}"));
        }
        (MODE_NOTIFY, _) => {}
        (_, MessageLevel::Error | MessageLevel::Warning) => {
            let args = format!("{args}");
            std::thread::spawn(move || {
                let _ = native_dialog::DialogBuilder::message()
                    .set_level(level)
                    .set_title("Maestro")
                    .set_text(args)
                    .alert()
                    .show();
            });
        }
        _ => {}
    }
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::logging::write_log(native_dialog::MessageLevel::Info, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => { $crate::logging::write_log(native_dialog::MessageLevel::Warning, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => { $crate::logging::write_log(native_dialog::MessageLevel::Error, format_args!($($arg)*)) };
}
