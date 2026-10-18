use std::ffi::{CString, c_void};

use maestro_core::system_cfg::{MaestroConfigManager, is_valid_app_id};

use crate::abi::*;
use crate::error::Failure;
use crate::profile::{self, ApiConfig};
use crate::sized;

pub struct Callbacks {
    log: MaestroLogFn,
    pub notify: MaestroNotifyFn,
    user: *mut c_void,
}

// Thread-safety of the callbacks is the host's promise, per the header.
unsafe impl Send for Callbacks {}
unsafe impl Sync for Callbacks {}

impl Callbacks {
    pub fn log(&self, level: MaestroLogLevel, message: &str) {
        if let Some(log) = self.log {
            let message = CString::new(message.replace('\0', "")).unwrap_or_default();
            unsafe { log(self.user, level, message.as_ptr()) };
        }
    }

    pub fn notify(&self, what: MaestroNotify) {
        if let Some(notify) = self.notify {
            unsafe { notify(self.user, what) };
        }
    }
}

/// The application behind an object, from its `MaestroAppInfo`.
pub struct App {
    pub id: String,
    name: Option<String>,
    pub callbacks: Callbacks,
}

impl App {
    /// # Safety
    /// `info` must be NULL or point at least `struct_size` readable bytes.
    pub unsafe fn read(info: *const MaestroAppInfo) -> Result<Self, Failure> {
        let info = unsafe { sized::read(info) };
        sized::reserved(info.reserved)?;
        let id = unsafe { sized::string(info.app_id) }
            .filter(|id| is_valid_app_id(id))
            .ok_or_else(|| {
                Failure::invalid(
                    "app_id is required: 1-128 characters of A-Z, a-z, 0-9, '.', '-' and '_'",
                )
            })?;
        Ok(Self {
            id,
            name: unsafe { sized::string(info.app_name) },
            callbacks: Callbacks {
                log: info.log,
                notify: None,
                user: info.user,
            },
        })
    }

    /// The app's profile, created from the defaults on first use, unless the
    /// user turned the API or the app off.
    pub fn profile(&self, manager: &MaestroConfigManager) -> Result<ApiConfig, Failure> {
        let global = profile::load_global(manager).unwrap_or_else(|e| {
            self.callbacks.log(
                MAESTRO_LOG_WARN,
                &format!("The Maestro API settings could not be read ({e}); using the defaults"),
            );
            ApiConfig::default()
        });
        if !global.enabled {
            return Err(Failure::new(
                MAESTRO_ERR_DISABLED,
                "The Maestro API is turned off in Maestro's settings",
            ));
        }

        if let Err(e) = profile::ensure_app(manager, &self.id, self.name.as_deref()) {
            self.callbacks.log(
                MAESTRO_LOG_WARN,
                &format!("This application's settings could not be created ({e})"),
            );
        }
        let config = profile::load_app(manager, &self.id).unwrap_or_else(|e| {
            self.callbacks.log(
                MAESTRO_LOG_WARN,
                &format!("This application's settings could not be read ({e}); using the defaults"),
            );
            ApiConfig {
                enabled: true,
                ..global
            }
        });
        if !config.enabled {
            return Err(Failure::new(
                MAESTRO_ERR_DISABLED,
                format!("Maestro is set not to serve this application ({})", self.id),
            ));
        }
        Ok(config)
    }
}
