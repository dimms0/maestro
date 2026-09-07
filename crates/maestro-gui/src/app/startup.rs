use std::path::{Path, PathBuf};

use maestro_core::{soundfont::SoundFontList, system_cfg::MaestroConfigManager};

use crate::{
    StandaloneMode,
    config::{get_component_files, get_soundfont_list_files, load_config_cache},
    errors,
    state::{AppData, SoundfontFileCache},
};

const EDIT_LIST_ARG: &str = "edit-list";
const EDIT_CONFIG_ARG: &str = "edit-config";

pub fn load(args: &[String]) -> AppData {
    let mut data = match args {
        [_, mode, path, ..] if mode == EDIT_LIST_ARG => single_list(PathBuf::from(path)),
        [_, mode, path, ..] if mode == EDIT_CONFIG_ARG => single_config(PathBuf::from(path)),
        _ => combined(),
    };

    probe_audio_devices(&mut data);
    data
}

fn probe_audio_devices(data: &mut AppData) {
    let (host, device) = data
        .config_files
        .iter()
        .find(|c| c.profile.has_realtime)
        .map(|c| {
            (
                c.realtime.audio_host.clone(),
                c.realtime.output_device.clone(),
            )
        })
        .unwrap_or_default();

    data.audio.refresh(host.as_deref(), device.as_deref());
}

fn stem_or(path: &Path, fallback: &str) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(fallback)
        .to_string()
}

fn empty(standalone_mode: StandaloneMode) -> AppData {
    AppData {
        sflist_files: Vec::new(),
        selected_sflist_idx: -1,
        config_files: Vec::new(),
        selected_config_idx: -1,
        render_queue: Vec::new(),
        available_sflists: Vec::new(),
        audio: Default::default(),
        standalone_mode,
    }
}

/// `edit-list`: a single SoundFont list read straight from the given path.
fn single_list(path: PathBuf) -> AppData {
    let list = std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default();

    let mut data = empty(StandaloneMode::EditList);
    data.sflist_files.push(SoundfontFileCache {
        name: stem_or(&path, "list"),
        path,
        dirty: false,
        list,
    });
    data.selected_sflist_idx = 0;
    data.refresh_available_sflists();
    data
}

/// `edit-config`: a single component config read straight from the given path.
fn single_config(path: PathBuf) -> AppData {
    let name = stem_or(&path, "config");
    let mut data = empty(StandaloneMode::EditConfig);
    let Some(cache) = load_config_cache(name, path) else {
        errors::report(
            "Invalid config",
            "The configuration file that was loaded does not correspond to a Maestro component.",
        );
        return combined();
    };
    data.config_files.push(cache);
    data.selected_config_idx = 0;
    load_sflist_files(&mut data);
    data
}

fn load_sflist_files(data: &mut AppData) {
    let manager = MaestroConfigManager::default();

    data.sflist_files = get_soundfont_list_files()
        .into_iter()
        .map(|(name, path)| {
            let list = manager
                .get_soundfont_list(&name)
                .unwrap_or_else(|_| SoundFontList::new_with_default_sublist());
            SoundfontFileCache {
                path,
                name,
                dirty: false,
                list,
            }
        })
        .collect();
    data.refresh_available_sflists();
}

/// The normal application: every SoundFont list and component config
fn combined() -> AppData {
    let mut data = empty(StandaloneMode::Combined);

    load_sflist_files(&mut data);

    data.config_files = get_component_files()
        .into_iter()
        .filter_map(|(name, path)| load_config_cache(name, path))
        .collect();

    data
}
