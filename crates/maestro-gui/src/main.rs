#![windows_subsystem = "windows"]

slint::include_modules!();

mod actions;
mod app;
mod config;
mod errors;
mod privileged;
mod slint_conv;
mod state;
mod sync;
#[cfg(feature = "update-check")]
mod update_check;
mod utils;
mod views;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    app::run(&args);
}
