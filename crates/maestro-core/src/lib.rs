#[macro_use]
mod macros;

pub mod audio_params;
pub mod error;
mod event;
pub mod file_renderer;
mod helpers;
pub mod ipc;
pub mod notify;
pub mod paths;
pub mod realtime;
pub mod renderer;
pub mod soundfont;
pub mod statistics;
pub mod sysinfo;
pub mod system_cfg;
mod tempo;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_ID: &str = env!("BUILD_ID");
