use std::process::Command;
use std::thread;

use slint::{ComponentHandle, Global, ModelRc, VecModel};

mod kdmapi;
mod libraries;
pub mod service;

use crate::{MainWindow, SlintIntegrationItem, SlintLibraryItem, SystemIntegrationState, errors};

const TITLE: &str = "System Integration";

struct Snapshot {
    service: SlintIntegrationItem,
    service_installed: bool,
    autostart: bool,
    kdmapi: SlintIntegrationItem,
    kdmapi_blocked: bool,
    libraries: Vec<SlintLibraryItem>,
}

fn probe() -> Snapshot {
    let service = service::probe();
    let kdmapi = kdmapi::probe();

    Snapshot {
        service: service.item,
        service_installed: service.installed,
        autostart: service.autostart,
        kdmapi: kdmapi.item,
        kdmapi_blocked: kdmapi.blocked,
        libraries: libraries::probe(),
    }
}

fn publish(ui: &MainWindow, snapshot: Snapshot) {
    let state = SystemIntegrationState::get(ui);
    let issue = !snapshot.service_installed
        || snapshot
            .libraries
            .iter()
            .any(|l| l.status != crate::IntegrationStatus::Ok);
    state.set_service(snapshot.service);
    state.set_service_installed(snapshot.service_installed);
    state.set_autostart(snapshot.autostart);
    state.set_kdmapi(snapshot.kdmapi);
    state.set_kdmapi_blocked(snapshot.kdmapi_blocked);
    state.set_libraries(ModelRc::new(VecModel::from(snapshot.libraries)));
    state.set_startup_issue(issue);
}

fn run(
    weak: &slint::Weak<MainWindow>,
    action: impl FnOnce() -> Result<(), String> + Send + 'static,
) {
    let Some(ui) = weak.upgrade() else {
        return;
    };
    SystemIntegrationState::get(&ui).set_busy(true);

    let weak = weak.clone();
    thread::spawn(move || {
        let failure = action().err();
        let snapshot = probe();

        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                publish(&ui, snapshot);
                SystemIntegrationState::get(&ui).set_busy(false);
            }
            if let Some(message) = failure {
                errors::report(TITLE, message);
            }
        });
    });
}

pub fn open_path(target: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let opener = "explorer";
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let opener = "xdg-open";

    Command::new(opener)
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open {target}: {e}"))
}

pub fn setup(ui: &MainWindow) {
    let state = SystemIntegrationState::get(ui);
    state.set_windows(cfg!(windows));
    state.set_lib_dir(
        maestro_core::paths::user_lib_dir()
            .display()
            .to_string()
            .into(),
    );
    state.set_lib_arch_hint(
        format!(
            "Libraries for applications of another architecture go in a subfolder named after it, \
             such as {}.",
            maestro_core::paths::user_lib_dir()
                .join(if cfg!(target_pointer_width = "64") {
                    "x86"
                } else {
                    "x86_64"
                })
                .display()
        )
        .into(),
    );

    let weak = ui.as_weak();
    state.on_refresh({
        let weak = weak.clone();
        move || run(&weak, || Ok(()))
    });

    state.on_service_action({
        let weak = weak.clone();
        move |action| run(&weak, move || service::act(action))
    });

    state.on_set_autostart({
        let weak = weak.clone();
        move |on| run(&weak, move || service::set_autostart(on))
    });

    state.on_kdmapi_action({
        let weak = weak.clone();
        move |action| run(&weak, move || kdmapi::act(action))
    });

    state.on_library_action({
        let weak = weak.clone();
        move |index, action| run(&weak, move || libraries::act(index, action))
    });

    run(&weak, || Ok(()));
}
