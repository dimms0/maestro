#![cfg_attr(windows, windows_subsystem = "windows")]

use maestrodrv::{log_error, logging, service::Service};

fn main() {
    if !maestro_core::ipc::try_lock_single_instance("maestrod") {
        return;
    }

    logging::use_notifications();

    let mut service = match Service::new() {
        Ok(s) => s,
        Err(err) => {
            log_error!("Failed to initialize: {err}");
            std::process::exit(1);
        }
    };

    #[cfg(unix)]
    handle_signals(service.event_sender());

    if let Err(err) = service.run() {
        log_error!("Fatal: {err}");
        std::process::exit(1);
    }
}

// Translate SIGTERM/SIGINT/SIGHUP into an orderly shutdown so devices and
// the audio stream are torn down cleanly.
#[cfg(unix)]
fn handle_signals(tx: std::sync::mpsc::Sender<maestrodrv::watcher::ServiceEvent>) {
    let signals = [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
    ];
    match signal_hook::iterator::Signals::new(signals) {
        Ok(mut signals) => {
            std::thread::spawn(move || {
                if signals.forever().next().is_some() {
                    let _ = tx.send(maestrodrv::watcher::ServiceEvent::Shutdown);
                }
            });
        }
        Err(err) => maestrodrv::log_warn!("Failed to install signal handlers: {err}"),
    }
}
