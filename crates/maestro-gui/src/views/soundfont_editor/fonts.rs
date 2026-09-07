use std::path::{Path, PathBuf};

use slint::Global;

use crate::{
    MainWindow, SoundfontEditorState, actions,
    app::AppContext,
    sync::{sync_sflist_to_slint, sync_soundfont_rows_in_place},
};
use maestro_core::soundfont::{SoundFont, SoundFontType};

use super::{edit_current_sublist, show_error};

const SOUNDFONT_FILTER: [&str; 6] = ["sf2", "SF2", "sf3", "SF3", "sfz", "SFZ"];

pub fn setup(ui: &MainWindow, cx: &AppContext) {
    let state = SoundfontEditorState::get(ui);

    state.on_browse_soundfont(|| actions::pick_file("SoundFont", &SOUNDFONT_FILTER));

    state.on_soundfont_type(|path| {
        SoundFontType::from_path(Path::new(path.as_str()))
            .label()
            .into()
    });

    let c = cx.clone();
    state.on_add_soundfont(move |path, bank, preset| {
        if SoundFontType::from_path(Path::new(path.as_str())) == SoundFontType::Unknown {
            show_error(format!(
                "'{path}' is not a recognized SoundFont (.sf2, .sf3 or .sfz) and was not added."
            ));
            return;
        }

        c.with(|ui, data| {
            let added = edit_current_sublist(ui, data, |sfs| {
                sfs.push(SoundFont {
                    enabled: true,
                    path: PathBuf::from(path.as_str()),
                    bank: bank as i16,
                    preset: preset as i16,
                });
                true
            });
            if added {
                sync_sflist_to_slint(ui, data);
            }
        });
    });

    let c = cx.clone();
    state.on_remove_soundfont(move |sf_idx| {
        c.with(|ui, data| {
            let removed = edit_current_sublist(ui, data, |sfs| {
                match crate::state::index_of(sf_idx, sfs.len()) {
                    Some(i) => {
                        sfs.remove(i);
                        true
                    }
                    None => false,
                }
            });
            if removed {
                sync_sflist_to_slint(ui, data);
            }
        });
    });

    let c = cx.clone();
    state.on_update_soundfont(move |sf_idx, enabled, path, bank, preset| {
        c.with(|ui, data| {
            let updated = edit_current_sublist(ui, data, |sfs| {
                match crate::state::index_of(sf_idx, sfs.len()) {
                    Some(i) => {
                        sfs[i] = SoundFont {
                            enabled,
                            path: PathBuf::from(path.as_str()),
                            bank: bank as i16,
                            preset: preset as i16,
                        };
                        true
                    }
                    None => false,
                }
            });
            if updated {
                sync_sflist_to_slint(ui, data);
            }
        });
    });

    let c = cx.clone();
    state.on_move_soundfont(move |from_idx, to_idx| {
        c.with(|ui, data| {
            let moved = edit_current_sublist(ui, data, |sfs| {
                let (Some(from), Some(to)) = (
                    crate::state::index_of(from_idx, sfs.len()),
                    crate::state::index_of(to_idx, sfs.len()),
                ) else {
                    return false;
                };
                if from == to {
                    return false;
                }
                let sf = sfs.remove(from);
                sfs.insert(to, sf);
                true
            });

            if moved && !sync_soundfont_rows_in_place(ui, data) {
                sync_sflist_to_slint(ui, data);
            }
        });
    });
}
