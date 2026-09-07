mod files;
mod fonts;
mod sublists;

use slint::Global;

use crate::{
    MainWindow, SflistState, SoundfontEditorState, actions, app::AppContext, errors,
    state::AppData, state::SoundfontFileCache,
};
use maestro_core::soundfont::SoundFont;

pub(crate) const ERROR_TITLE: &str = "SoundFont List Editor";

pub fn setup(ui: &MainWindow, cx: &AppContext) {
    files::setup(ui, cx);
    sublists::setup(ui, cx);
    fonts::setup(ui, cx);

    let c = cx.clone();
    SflistState::get(ui).on_edit(move |name| {
        c.with(|ui, data| actions::open_sflist_editor(ui, data, name.as_str()));
    });
}

pub(crate) fn show_error(msg: impl Into<String>) {
    errors::report(ERROR_TITLE, msg);
}

pub(crate) fn edit_selected_file(
    data: &mut AppData,
    edit: impl FnOnce(&mut SoundfontFileCache),
) -> bool {
    match data.selected_sflist_mut() {
        Some(file) => {
            edit(file);
            file.dirty = true;
            true
        }
        None => false,
    }
}

pub(crate) fn edit_current_sublist(
    ui: &MainWindow,
    data: &mut AppData,
    edit: impl FnOnce(&mut Vec<SoundFont>) -> bool,
) -> bool {
    let sub_idx = SoundfontEditorState::get(ui).get_selected_sublist_index();
    let Some(file) = data.selected_sflist_mut() else {
        return false;
    };
    let Some(sublist) = file.sublist_name(sub_idx) else {
        return false;
    };

    let changed = edit(file.list.lists.entry(sublist).or_default());
    if changed {
        file.dirty = true;
    }
    changed
}
