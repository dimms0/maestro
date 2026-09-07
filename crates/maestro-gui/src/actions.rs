//! Operations shared by more than one view

use crate::{
    AppState, MainWindow, SettingsEditorState, SoundfontEditorState, Tab,
    config::{load_config_cache, save_config_cache},
    errors,
    state::{AppData, ComponentConfigCache, ConfigComponent, index_of},
    sync::{
        sync_audio_devices_to_slint, sync_config_to_slint, sync_device_to_slint,
        sync_renderer_to_slint, sync_sflist_to_slint,
    },
};
use maestro_core::soundfont::DEFAULT_SOUNDFONT_LIST_NAME;
use slint::{Global, SharedString};

// Navigation

pub fn open_component_settings(ui: &MainWindow, data: &mut AppData, kind: ConfigComponent) {
    if let Some(pos) = data.config_position(kind) {
        open_config_at(ui, data, pos);
    }
}

pub fn open_component_settings_by_name(ui: &MainWindow, data: &mut AppData, name: &str) {
    if let Some(pos) = data.config_files.iter().position(|c| c.name == name) {
        open_config_at(ui, data, pos);
    }
}

fn open_config_at(ui: &MainWindow, data: &mut AppData, pos: usize) {
    data.selected_config_idx = pos as i32;
    sync_config_to_slint(ui, data);
    SettingsEditorState::get(ui).set_open(true);
}

/// Shows the SoundFont list editor on `name`. Every `SflistPicker`'s Edit
/// button lands here, from any view and in any launch mode.
pub fn open_sflist_editor(ui: &MainWindow, data: &mut AppData, name: &str) {
    // Fall back to the first list rather than leaving nothing selected: in the
    // standalone `edit-config` window the editor pane is all there is, so an
    // empty one would be a dead end.
    let target = data
        .sflist_position(name)
        .or_else(|| (!data.sflist_files.is_empty()).then_some(0));
    if let Some(pos) = target {
        data.selected_sflist_idx = pos as i32;
        SoundfontEditorState::get(ui).set_selected_sublist_index(0);
        sync_sflist_to_slint(ui, data);
    }
    // Closing the settings editor both dismisses the modal (combined mode) and
    // hands the standalone `edit-config` window over to the list editor.
    SettingsEditorState::get(ui).set_open(false);
    AppState::get(ui).set_active_tab(Tab::Soundfonts);
}

// Config editing

pub fn edit_config_at(
    ui: &MainWindow,
    data: &mut AppData,
    pos: usize,
    mutate: impl FnOnce(&mut ComponentConfigCache),
) {
    let Some(cfg) = data.config_files.get_mut(pos) else {
        return;
    };
    mutate(cfg);
    cfg.dirty = true;
    sync_config_to_slint(ui, data);
    sync_device_to_slint(ui, data);
    sync_audio_devices_to_slint(ui, data);
}

pub fn edit_selected_config(
    ui: &MainWindow,
    data: &mut AppData,
    mutate: impl FnOnce(&mut ComponentConfigCache),
) {
    if let Some(pos) = index_of(data.selected_config_idx, data.config_files.len()) {
        edit_config_at(ui, data, pos, mutate);
    }
}

pub fn edit_config(
    ui: &MainWindow,
    data: &mut AppData,
    kind: ConfigComponent,
    mutate: impl FnOnce(&mut ComponentConfigCache),
) {
    if let Some(pos) = data.config_position(kind) {
        edit_config_at(ui, data, pos, mutate);
    }
}

pub fn save_config_at(ui: &MainWindow, data: &mut AppData, pos: usize, error_title: &str) {
    let Some(cfg) = data.config_files.get(pos) else {
        return;
    };
    match save_config_cache(cfg) {
        Ok(()) => {
            data.config_files[pos].dirty = false;
            sync_config_to_slint(ui, data);
            sync_device_to_slint(ui, data);
        }
        Err(e) => errors::report(error_title, format!("Failed to save configuration: {e}")),
    }
}

pub fn discard_config_at(ui: &MainWindow, data: &mut AppData, pos: usize) {
    let Some((name, path)) = data
        .config_files
        .get(pos)
        .map(|cfg| (cfg.name.clone(), cfg.path.clone()))
    else {
        return;
    };

    // Unwrapping should be safe since we already check the names at startup,
    // so it should match here too
    data.config_files[pos] = load_config_cache(name, path).unwrap();
    sync_config_to_slint(ui, data);
    sync_device_to_slint(ui, data);
}

// SoundFont list references

pub struct RepointSummary {
    /// Names of the component configs whose `sflist` key was rewritten.
    pub configs: Vec<String>,
    /// How many conversion queue entries were repointed.
    pub queue_entries: usize,
}

impl RepointSummary {
    pub fn is_empty(&self) -> bool {
        self.configs.is_empty() && self.queue_entries == 0
    }

    /// Human-readable "component config(s) [a, b] and N render queue entrie(s)".
    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.configs.is_empty() {
            parts.push(format!("component config(s) [{}]", self.configs.join(", ")));
        }
        if self.queue_entries > 0 {
            parts.push(format!("{} render queue entrie(s)", self.queue_entries));
        }
        parts.join(" and ")
    }
}

/// Repoints everything that referenced the SoundFont list `old_name` at
/// `new_name`: the in-memory configs, their files on disk, and the conversion
/// queue.
pub fn repoint_sflist_references(
    data: &mut AppData,
    old_name: &str,
    new_name: &str,
) -> RepointSummary {
    let mut configs = Vec::new();
    for cfg in data.config_files.iter_mut() {
        if cfg.profile.has_sflist && cfg.sflist == old_name {
            cfg.sflist = new_name.to_string();
            if let Some(obj) = cfg.val.as_object_mut() {
                obj.insert(
                    "sflist".to_string(),
                    serde_json::Value::String(new_name.to_string()),
                );
            }
            patch_sflist_on_disk(&cfg.path, new_name);
            configs.push(cfg.name.clone());
        }
    }

    let mut queue_entries = 0;
    for entry in data.render_queue.iter_mut() {
        if entry.sflist_name.as_str() == old_name {
            entry.sflist_name = SharedString::from(new_name);
            queue_entries += 1;
        }
    }

    RepointSummary {
        configs,
        queue_entries,
    }
}

pub fn report_repointed(summary: &RepointSummary, title: &str, message: impl Fn(&str) -> String) {
    if !summary.is_empty() {
        errors::report(title, message(&summary.describe()));
    }
}

/// Rewrites just the `sflist` key of a component config file on disk
fn patch_sflist_on_disk(path: &std::path::Path, new_name: &str) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };
    let Some(obj) = val.as_object_mut() else {
        return;
    };
    obj.insert(
        "sflist".to_string(),
        serde_json::Value::String(new_name.to_string()),
    );
    if let Ok(content) = serde_json::to_string_pretty(&val) {
        let _ = std::fs::write(path, content);
    }
}

pub const FALLBACK_SFLIST: &str = DEFAULT_SOUNDFONT_LIST_NAME;

// Misc

pub fn pick_file(filter_name: &str, extensions: &[&str]) -> SharedString {
    rfd::FileDialog::new()
        .add_filter(filter_name, extensions)
        .pick_file()
        .and_then(|p| p.to_str().map(SharedString::from))
        .unwrap_or_default()
}

pub fn sync_after_sflist_change(ui: &MainWindow, data: &AppData) {
    sync_sflist_to_slint(ui, data);
    sync_renderer_to_slint(ui, data);
    sync_device_to_slint(ui, data);
    sync_config_to_slint(ui, data);
}
