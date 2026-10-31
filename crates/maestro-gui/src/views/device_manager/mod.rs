use std::time::Duration;

use slint::{ComponentHandle, Global, Timer, TimerMode};

mod stats;

use crate::{
    DeviceComponent, DeviceManagerState, MainWindow, SlintSystemCustom, StandaloneMode, actions,
    app::AppContext,
    config, errors,
    slint_conv::slint_to_system_custom,
    state::{AppData, ConfigComponent, app_id},
    sync::sync_device_to_slint,
    views::system_integration::service,
};

pub const WINDOWS_COMPAT_TEXT: &str = "MIDI 2.0 functionality is not yet\
supported on Windows as it requires Windows MIDI Services which are not\
officially released yet.\n\n\
For more information about Windows MIDI Services visit: https://aka.ms/midi";

impl From<DeviceComponent> for ConfigComponent {
    fn from(component: DeviceComponent) -> Self {
        match component {
            DeviceComponent::System => ConfigComponent::System,
            DeviceComponent::Kdmapi => ConfigComponent::KDMAPI,
        }
    }
}

fn error_title(component: DeviceComponent) -> &'static str {
    match component {
        DeviceComponent::System => "Virtual MIDI Device",
        DeviceComponent::Kdmapi => "KDMAPI",
    }
}

fn with_position(
    data: &mut AppData,
    component: DeviceComponent,
    f: impl FnOnce(&mut AppData, usize),
) {
    if let Some(pos) = data.config_position(component.into()) {
        f(data, pos);
    }
}

fn spawn_running_change(weak: slint::Weak<MainWindow>, running: bool) {
    let Some(ui) = weak.upgrade() else {
        return;
    };
    DeviceManagerState::get(&ui).set_daemon_busy(true);

    std::thread::spawn(move || {
        let failure = if running {
            service::start().err()
        } else {
            service::stop().err()
        };

        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade()
                && let Some(message) = failure
            {
                DeviceManagerState::get(&ui).set_daemon_busy(false);
                crate::errors::report("Virtual MIDI Device", message);
            }
        });
    });
}

fn remove_app_config(ui: &MainWindow, data: &mut AppData, name: &str) {
    let Some(pos) = data.config_files.iter().position(|c| c.name == name) else {
        return;
    };
    let removed = data.config_files.remove(pos);

    if removed.path.exists()
        && let Err(err) = std::fs::remove_file(&removed.path)
    {
        errors::report(
            "Maestro API",
            format!(
                "Failed to delete '{}': {err}\nThe file was left in place and will reappear on the next start.",
                removed.path.display()
            ),
        );
    }

    data.selected_config_idx = match data.selected_config_idx {
        i if i == pos as i32 => -1,
        i if i > pos as i32 => i - 1,
        i => i,
    };
    sync_device_to_slint(ui, data);
}

thread_local! {
    static APP_WATCH: Timer = Timer::default();
}

fn pick_up_new_apps(ui: &MainWindow, data: &mut AppData) {
    let new: Vec<_> = config::get_component_files()
        .into_iter()
        .filter(|(name, _)| {
            app_id(name).is_some() && !data.config_files.iter().any(|c| &c.name == name)
        })
        .filter_map(|(name, path)| config::load_config_cache(name, path))
        .collect();

    if !new.is_empty() {
        data.config_files.extend(new);
        sync_device_to_slint(ui, data);
    }
}

pub fn setup(ui: &MainWindow, cx: &AppContext) {
    let state = DeviceManagerState::get(ui);

    let global_cfg = config::load_global_config();
    state.set_start_on_launch(global_cfg.start_virtual_device_on_launch);

    // System's inline device fields
    let c = cx.clone();
    state.on_update_system_custom(move |custom: SlintSystemCustom| {
        c.with(|ui, data| {
            // TODO remove when WMS are implemented
            actions::edit_config(ui, data, ConfigComponent::System, |f| {
                f.system_custom = slint_to_system_custom(&custom, &mut f.remembered);
                if f.system_custom.midi2_enabled && cfg!(windows) {
                    crate::errors::report("Not supported", WINDOWS_COMPAT_TEXT);
                    f.system_custom.midi2_enabled = false;
                }
            });

            // in case MIDI 2.0 was selected on windows
            sync_device_to_slint(ui, data);
        });
    });

    let weak = ui.as_weak();
    state.on_set_running(move |running| spawn_running_change(weak.clone(), running));

    let weak = ui.as_weak();
    state.on_set_start_on_launch(move |on| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        DeviceManagerState::get(&ui).set_start_on_launch(on);

        let mut cfg = config::load_global_config();
        cfg.start_virtual_device_on_launch = on;
        if let Err(e) = config::save_global_config(&cfg) {
            errors::report(
                "Virtual MIDI Device",
                format!("Failed to save settings: {e}"),
            );
        }
    });

    // Per-component actions
    let c = cx.clone();
    state.on_set_enabled(move |component, enabled| {
        c.with(|ui, data| {
            actions::edit_config(ui, data, component.into(), |f| f.enabled = enabled)
        });
    });

    let c = cx.clone();
    state.on_set_sflist(move |component, name| {
        c.with(|ui, data| {
            actions::edit_config(ui, data, component.into(), |f| {
                f.sflist = name.as_str().to_string()
            })
        });
    });

    let c = cx.clone();
    state.on_save(move |component| {
        c.with(|ui, data| {
            with_position(data, component, |data, pos| {
                actions::save_config_at(ui, data, pos, error_title(component));
            })
        });
    });

    let c = cx.clone();
    state.on_discard(move |component| {
        c.with(|ui, data| {
            with_position(data, component, |data, pos| {
                actions::discard_config_at(ui, data, pos);
            })
        });
    });

    let c = cx.clone();
    state.on_open_settings(move |component| {
        c.with(|ui, data| actions::open_component_settings(ui, data, component.into()));
    });

    let c = cx.clone();
    state.on_remove_app_config(move |name| {
        c.with(|ui, data| remove_app_config(ui, data, &name));
    });

    stats::spawn(ui);

    if cx.with(|_, data| data.standalone_mode == StandaloneMode::Combined) == Some(true) {
        let c = cx.clone();
        APP_WATCH.with(|timer| {
            timer.start(TimerMode::Repeated, Duration::from_secs(2), move || {
                c.with(pick_up_new_apps);
            })
        });
    }

    if global_cfg.start_virtual_device_on_launch {
        spawn_running_change(ui.as_weak(), true);
    }
}
