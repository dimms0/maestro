use slint::{ComponentHandle, Global};

use crate::{AppState, MainWindow, config, errors, slint_conv::tab_to_key};

pub fn setup(ui: &MainWindow) {
    let state = AppState::get(ui);
    state.set_remember_last_tab(config::load_global_config().remember_last_tab);

    let weak = ui.as_weak();
    state.on_welcome_navigate(move |target| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let state = AppState::get(&ui);
        state.set_active_tab(target);

        if state.get_remember_last_tab() {
            let mut cfg = config::load_global_config();
            cfg.last_tab = tab_to_key(target).to_string();
            if let Err(e) = config::save_global_config(&cfg) {
                errors::report("Settings", format!("Failed to save the last tab: {e}"));
            }
        }
    });

    let weak = ui.as_weak();
    state.on_set_remember_last_tab(move |on| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let state = AppState::get(&ui);
        state.set_remember_last_tab(on);

        let mut cfg = config::load_global_config();
        cfg.remember_last_tab = on;
        if on {
            cfg.last_tab = tab_to_key(state.get_active_tab()).to_string();
        }
        if let Err(e) = config::save_global_config(&cfg) {
            errors::report("Settings", format!("Failed to save settings: {e}"));
        }
    });
}
