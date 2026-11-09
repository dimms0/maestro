use std::time::Duration;

use maestro_core::system_cfg::MaestroConfigManager;
use slint::{ComponentHandle, Global, Timer, TimerMode};

mod stats;

use crate::{
    DeviceComponent, DeviceManagerState, MainWindow, SlintSystemCustom, StandaloneMode, actions,
    app::AppContext,
    config, errors,
    slint_conv::slint_to_system_custom,
    state::{AppData, ComponentConfigCache, ConfigComponent, app_id},
    sync::sync_device_to_slint,
    views::system_integration::service,
};

pub const WINDOWS_COMPAT_TEXT: &str = "MIDI 2.0 functionality is not yet \
supported on Windows as it requires Windows MIDI Services which are not \
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

fn edit_and_save(
    ui: &MainWindow,
    data: &mut AppData,
    name: &str,
    error_title: &str,
    mutate: impl FnOnce(&mut ComponentConfigCache),
) {
    if let Some(pos) = data.config_files.iter().position(|c| c.name == name) {
        actions::edit_config_at(ui, data, pos, mutate);
        actions::save_config_at(ui, data, pos, error_title);
    }
}

fn set_use_shared(ui: &MainWindow, data: &mut AppData, name: &str, use_shared: bool) {
    let saved_shared = config::load_config_cache(
        ConfigComponent::Shared.to_filename(),
        MaestroConfigManager::default().get_component_config_path(&ConfigComponent::Shared),
    );
    edit_and_save(ui, data, name, "Shared Settings", |cfg| {
        cfg.use_shared = use_shared;
        if let (false, Some(shared)) = (use_shared, &saved_shared) {
            cfg.copy_shared_settings(shared);
        }
    });
}

thread_local! {
    static CONFIG_WATCH: Timer = Timer::default();
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
    state.on_set_use_shared(move |name, use_shared| {
        c.with(|ui, data| set_use_shared(ui, data, &name, use_shared));
    });

    let c = cx.clone();
    state.on_set_app_enabled(move |name, enabled| {
        c.with(|ui, data| {
            edit_and_save(ui, data, &name, "Maestro API", |cfg| cfg.enabled = enabled)
        });
    });

    let c = cx.clone();
    state.on_set_app_sflist(move |name, sflist| {
        c.with(|ui, data| {
            edit_and_save(ui, data, &name, "Maestro API", |cfg| {
                cfg.sflist = sflist.as_str().to_string()
            })
        });
    });

    let c = cx.clone();
    state.on_remove_app_config(move |name| {
        c.with(|ui, data| remove_app_config(ui, data, &name));
    });

    let resets = stats::spawn(ui);
    state.on_panic(move |pid, component| {
        if let Some(component) = ConfigComponent::from_code(component as u32) {
            let _ = resets.send((pid as u32, component));
        }
    });

    if cx.with(|_, data| data.standalone_mode != StandaloneMode::EditList) == Some(true) {
        let c = cx.clone();
        CONFIG_WATCH.with(|timer| {
            timer.start(TimerMode::Repeated, Duration::from_secs(2), move || {
                c.with(|ui, data| {
                    pick_up_new_apps(ui, data);
                    actions::reload_changed_configs(ui, data);
                });
            })
        });
    }

    if global_cfg.start_virtual_device_on_launch {
        spawn_running_change(ui.as_weak(), true);
    }
}
