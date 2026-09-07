use std::path::Path;

use slint::{ComponentHandle, Global};

use crate::{
    MainWindow, SoundfontEditorState, actions,
    app::AppContext,
    state::{AppData, SoundfontFileCache, index_of},
    sync::sync_sflist_to_slint,
};
use maestro_core::{
    soundfont::{DEFAULT_SOUNDFONT_LIST_NAME, SoundFontList},
    system_cfg::MaestroConfigManager,
};

use super::show_error;

fn is_valid_list_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\') && name != "." && name != ".."
}

fn write_list(path: &Path, list: &SoundFontList) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create SoundFont list directory: {e}"))?;
    }
    let content = serde_json::to_string_pretty(list)
        .map_err(|e| format!("Failed to serialize SoundFont list: {e}"))?;
    std::fs::write(path, content).map_err(|e| format!("Failed to write SoundFont list file: {e}"))
}

pub fn setup(ui: &MainWindow, cx: &AppContext) {
    let state = SoundfontEditorState::get(ui);

    let c = cx.clone();
    state.on_select_file(move |idx| {
        c.with(|ui, data| {
            data.selected_sflist_idx = idx;
            SoundfontEditorState::get(ui).set_selected_sublist_index(0);
            sync_sflist_to_slint(ui, data);
        });
    });

    let c = cx.clone();
    state.on_save_current(move || {
        c.with(save_current);
    });

    let c = cx.clone();
    state.on_discard_current(move || {
        c.with(discard_current);
    });

    let c = cx.clone();
    state.on_create_sflist(move |name| {
        c.with(|ui, data| create_sflist(ui, data, name.as_str().trim()));
    });

    let c = cx.clone();
    state.on_delete_sflist(move |idx| {
        c.with(|ui, data| delete_sflist(ui, data, idx));
    });

    let c = cx.clone();
    state.on_rename_sflist(move |idx, new_name| {
        c.with(|ui, data| rename_sflist(ui, data, idx, new_name.as_str().trim()));
    });

    let c = cx.clone();
    state.on_exit_standalone(move || {
        c.with_ui(|ui| {
            let _ = ui.hide();
        });
    });
}

fn save_current(ui: &MainWindow, data: &mut AppData) {
    let Some(file) = data.selected_sflist() else {
        return;
    };
    if let Err(msg) = write_list(&file.path, &file.list) {
        show_error(msg);
        return;
    }

    if let Some(file) = data.selected_sflist_mut() {
        file.dirty = false;
    }
    data.refresh_available_sflists();
    actions::sync_after_sflist_change(ui, data);
}

fn discard_current(ui: &MainWindow, data: &mut AppData) {
    let Some(file) = data.selected_sflist_mut() else {
        return;
    };

    let reloaded = if file.path.exists() {
        let Some(list) = std::fs::read_to_string(&file.path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
        else {
            return;
        };
        list
    } else {
        SoundFontList::new_with_default_sublist()
    };

    file.list = reloaded;
    file.dirty = false;
    sync_sflist_to_slint(ui, data);
}

fn create_sflist(ui: &MainWindow, data: &mut AppData, name: &str) {
    if !is_valid_list_name(name) {
        show_error("Invalid SoundFont list name.");
        return;
    }
    if data.sflist_position(name).is_some() {
        show_error(format!("A SoundFont list named '{name}' already exists."));
        return;
    }

    let path = MaestroConfigManager::default()
        .sflists_default_dir()
        .join(format!("{name}.json"));
    let list = SoundFontList::new_with_default_sublist();
    if let Err(msg) = write_list(&path, &list) {
        show_error(msg);
        return;
    }

    data.sflist_files.push(SoundfontFileCache {
        path,
        name: name.to_string(),
        dirty: false,
        list,
    });
    data.selected_sflist_idx = (data.sflist_files.len() - 1) as i32;
    data.refresh_available_sflists();

    SoundfontEditorState::get(ui).set_selected_sublist_index(0);
    actions::sync_after_sflist_change(ui, data);
}

fn delete_sflist(ui: &MainWindow, data: &mut AppData, idx: i32) {
    let Some(idx) = index_of(idx, data.sflist_files.len()) else {
        return;
    };
    if data.sflist_files[idx].name == DEFAULT_SOUNDFONT_LIST_NAME {
        show_error("The Default SoundFont list cannot be deleted.");
        return;
    }

    let removed = data.sflist_files.remove(idx);

    if removed.path.exists()
        && let Err(err) = std::fs::remove_file(&removed.path)
    {
        show_error(format!(
            "Failed to delete '{}': {err}\nThe file was left in place and will reappear on the next start.",
            removed.path.display()
        ));
    }

    let summary = actions::repoint_sflist_references(data, &removed.name, actions::FALLBACK_SFLIST);
    actions::report_repointed(&summary, "SoundFont List Deleted", |what| {
        format!(
            "References to '{}' in {what} were switched to the '{}' list.",
            removed.name,
            actions::FALLBACK_SFLIST
        )
    });

    data.selected_sflist_idx = if data.sflist_files.is_empty() {
        -1
    } else if data.selected_sflist_idx >= 0 {
        let selected = data.selected_sflist_idx as usize;
        let shifted = if selected > idx {
            selected - 1
        } else {
            selected
        };
        shifted.min(data.sflist_files.len() - 1) as i32
    } else {
        data.selected_sflist_idx
    };
    data.refresh_available_sflists();

    SoundfontEditorState::get(ui).set_selected_sublist_index(0);
    actions::sync_after_sflist_change(ui, data);
}

fn rename_sflist(ui: &MainWindow, data: &mut AppData, idx: i32, new_name: &str) {
    let Some(idx) = index_of(idx, data.sflist_files.len()) else {
        return;
    };
    if !is_valid_list_name(new_name) {
        show_error("Invalid SoundFont list name.");
        return;
    }
    if new_name == data.sflist_files[idx].name {
        return;
    }
    if data.sflist_position(new_name).is_some() {
        show_error(format!(
            "A SoundFont list named '{new_name}' already exists."
        ));
        return;
    }

    let old_path = data.sflist_files[idx].path.clone();
    let new_path = old_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{new_name}.json"));

    if old_path.exists()
        && let Err(err) = std::fs::rename(&old_path, &new_path)
    {
        show_error(format!("Failed to rename SoundFont list file: {err}"));
        return;
    }

    let old_name = std::mem::replace(&mut data.sflist_files[idx].name, new_name.to_string());
    data.sflist_files[idx].path = new_path;
    data.refresh_available_sflists();

    let summary = actions::repoint_sflist_references(data, &old_name, new_name);
    actions::report_repointed(&summary, "SoundFont List Renamed", |what| {
        format!("References to '{old_name}' in {what} were updated to '{new_name}'.")
    });

    actions::sync_after_sflist_change(ui, data);
}
