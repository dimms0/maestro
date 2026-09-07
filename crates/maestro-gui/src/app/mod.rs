mod panic;
mod startup;
mod window_events;

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global};

use crate::{AppState, MainWindow, errors, state::AppData, sync, views};

const SOUNDFONT_URL: &str = "https://musical-artifacts.com/artifacts?tags=soundfont";

#[derive(Clone)]
pub struct AppContext {
    ui: slint::Weak<MainWindow>,
    data: Arc<Mutex<AppData>>,
}

impl AppContext {
    fn new(ui: &MainWindow, data: Arc<Mutex<AppData>>) -> Self {
        Self {
            ui: ui.as_weak(),
            data,
        }
    }

    pub fn with<R>(&self, f: impl FnOnce(&MainWindow, &mut AppData) -> R) -> Option<R> {
        let ui = self.ui.upgrade()?;
        let mut data = self.data.lock().unwrap();
        Some(f(&ui, &mut data))
    }

    pub fn with_ui<R>(&self, f: impl FnOnce(&MainWindow) -> R) -> Option<R> {
        self.ui.upgrade().map(|ui| f(&ui))
    }
}

pub fn run(args: &[String]) {
    if let Some(code) = crate::privileged::handle(args) {
        std::process::exit(code);
    }

    if !maestro_core::ipc::try_lock_single_instance("gui") {
        return;
    }

    let _ = slint::set_xdg_app_id("gr.dimms.maestro");

    let ui = MainWindow::new().expect("Failed to create Slint UI");

    errors::init(&ui);

    maestro_core::realtime::set_stream_error_handler(|msg| {
        errors::report("Audio Stream", msg);
    });

    panic::install_hook(&ui);

    let data = Arc::new(Mutex::new(startup::load(args)));
    let cx = AppContext::new(&ui, data);

    {
        let data = cx.data.lock().unwrap();
        AppState::get(&ui).set_standalone_mode(data.standalone_mode);

        let global_cfg = crate::config::load_global_config();
        let initial_tab = if data.standalone_mode == crate::StandaloneMode::Combined
            && global_cfg.remember_last_tab
        {
            crate::slint_conv::tab_from_key(&global_cfg.last_tab).unwrap_or(crate::Tab::Welcome)
        } else {
            crate::Tab::Welcome
        };
        AppState::get(&ui).set_active_tab(initial_tab);
        AppState::get(&ui).set_version(
            format!(
                "Version {} | Build {}",
                maestro_core::VERSION,
                maestro_core::BUILD_ID
            )
            .into(),
        );
        let no_soundfonts = data
            .sflist_files
            .iter()
            .all(|f| f.list.lists.values().all(|v| v.is_empty()));
        AppState::get(&ui).set_no_soundfonts_prompt(no_soundfonts);
        // `edit-config` opens on the config; `open` then decides which of the
        // two editors that window shows (see app.slint).
        crate::SettingsEditorState::get(&ui)
            .set_open(data.standalone_mode == crate::StandaloneMode::EditConfig);
        sync::sync_all(&ui, &data);
    }

    AppState::get(&ui).on_open_soundfonts_page(|| {
        let _ = views::system_integration::open_path(SOUNDFONT_URL);
    });
    let ui_weak = ui.as_weak();
    AppState::get(&ui).on_open_update_page(move || {
        if let Some(ui) = ui_weak.upgrade() {
            let _ = views::system_integration::open_path(&AppState::get(&ui).get_update_url());
        }
    });

    #[cfg(feature = "update-check")]
    crate::update_check::spawn_check(ui.as_weak());

    views::soundfont_editor::setup(&ui, &cx);
    views::settings_editor::setup(&ui, &cx);
    views::renderer::setup(&ui, &cx);
    views::device_manager::setup(&ui, &cx);
    views::system_integration::setup(&ui);
    views::welcome::setup(&ui);
    window_events::setup(&ui, &cx);

    let ui_weak = ui.as_weak();
    ui.on_exit_app(move || {
        if let Some(ui) = ui_weak.upgrade() {
            let _ = ui.hide();
        }
    });

    let state = AppState::get(&ui);

    let ui_weak = ui.as_weak();
    state.on_quit_keep_running(move || {
        if let Some(ui) = ui_weak.upgrade() {
            let _ = ui.hide();
        }
    });

    let ui_weak = ui.as_weak();
    state.on_quit_and_stop(move || {
        let ui_weak = ui_weak.clone();
        std::thread::spawn(move || {
            let failure = views::system_integration::service::stop().err();

            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                match failure {
                    Some(message) => errors::report("Virtual MIDI Device", message),
                    None => {
                        let _ = ui.hide();
                    }
                }
            });
        });
    });

    ui.run().expect("Slint event loop failed");
}
