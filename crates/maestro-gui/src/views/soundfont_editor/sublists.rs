use slint::Global;

use crate::{MainWindow, SoundfontEditorState, app::AppContext, sync::sync_sflist_to_slint};

use super::{edit_selected_file, show_error};

pub fn setup(ui: &MainWindow, cx: &AppContext) {
    let state = SoundfontEditorState::get(ui);

    let c = cx.clone();
    state.on_select_sublist(move |idx| {
        c.with(|ui, data| {
            SoundfontEditorState::get(ui).set_selected_sublist_index(idx);
            sync_sflist_to_slint(ui, data);
        });
    });

    let c = cx.clone();
    state.on_add_sublist(move |name| {
        c.with(|ui, data| {
            if edit_selected_file(data, |file| {
                file.list
                    .lists
                    .entry(name.as_str().to_string())
                    .or_default();
            }) {
                sync_sflist_to_slint(ui, data);
            }
        });
    });

    let c = cx.clone();
    state.on_remove_sublist(move |name| {
        c.with(|ui, data| {
            if edit_selected_file(data, |file| {
                file.list.lists.remove(name.as_str());
            }) {
                SoundfontEditorState::get(ui).set_selected_sublist_index(0);
                sync_sflist_to_slint(ui, data);
            }
        });
    });

    let c = cx.clone();
    state.on_rename_sublist(move |old_name, new_name| {
        c.with(|ui, data| {
            let old_name = old_name.as_str().to_string();
            let new_name = new_name.as_str().trim().to_string();
            if new_name.is_empty() || new_name == old_name {
                return;
            }

            let Some(file) = data.selected_sflist_mut() else {
                return;
            };
            if file.list.lists.contains_key(&new_name) {
                show_error(format!("A sub-list named '{new_name}' already exists."));
                return;
            }
            let Some(soundfonts) = file.list.lists.remove(&old_name) else {
                return;
            };
            file.list.lists.insert(new_name.clone(), soundfonts);

            if file.list.default_list.as_deref() == Some(old_name.as_str()) {
                file.list.default_list = Some(new_name.clone());
            }
            for target in file.list.port_assignments.values_mut() {
                if *target == old_name {
                    *target = new_name.clone();
                }
            }
            file.dirty = true;

            if let Some(idx) = file.sorted_sublists().iter().position(|s| s == &new_name) {
                SoundfontEditorState::get(ui).set_selected_sublist_index(idx as i32);
            }
            sync_sflist_to_slint(ui, data);
        });
    });

    let c = cx.clone();
    state.on_update_port_assignment(move |port, list_name| {
        c.with(|ui, data| {
            if edit_selected_file(data, |file| {
                let assignments = &mut file.list.port_assignments;
                match list_name.is_empty() {
                    true => {
                        assignments.remove(&(port as usize));
                    }
                    false => {
                        assignments.insert(port as usize, list_name.as_str().to_string());
                    }
                }
            }) {
                sync_sflist_to_slint(ui, data);
            }
        });
    });

    let c = cx.clone();
    state.on_set_default_sublist(move |name| {
        c.with(|ui, data| {
            if edit_selected_file(data, |file| {
                file.list.default_list = (!name.is_empty()).then(|| name.as_str().to_string());
            }) {
                sync_sflist_to_slint(ui, data);
            }
        });
    });
}
