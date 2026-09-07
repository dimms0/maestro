#[cfg(unix)]
fn main() {
    use maestrodrv::{log_error, log_warn, logging, service::Service, watcher::ServiceEvent};

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

    // Translate SIGTERM/SIGINT/SIGHUP into an orderly shutdown so devices and
    // the audio stream are torn down cleanly.
    let tx = service.event_sender();
    let signals = [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
    ];
    match signal_hook::iterator::Signals::new(signals) {
        Ok(mut signals) => {
            std::thread::spawn(move || {
                if signals.forever().next().is_some() {
                    let _ = tx.send(ServiceEvent::Shutdown);
                }
            });
        }
        Err(err) => log_warn!("Failed to install signal handlers: {err}"),
    }

    if let Err(err) = service.run() {
        log_error!("Fatal: {err}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn main() {
    use maestrodrv::log_error;

    log_error!(
        "maestrod is not used on Windows. Install maestrodrv.dll as a WinMM \
         MIDI driver instead (see the maestro-daemon documentation)."
    );
    std::process::exit(1);
}
