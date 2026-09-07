use std::ffi::OsStr;

use crate::{
    AudioDeviceState, DeviceManagerState, MainWindow, RendererState, SettingsEditorState,
    SflistState, SlintAudioParameters, SlintComponentConfig, SlintComponentSummary,
    SlintPortAssignment, SlintRealtimeConfig, SlintRendererConfig, SlintSoundFont,
    SoundfontEditorState,
    slint_conv::{
        converter_custom_to_slint, evproc_to_slint, postproc_to_slint, synth_to_slint,
        system_custom_to_slint,
    },
    state::{AppData, ConfigComponent},
};
use maestro_core::{
    audio_params::{COMMON_SAMPLE_RATES, ChannelCount},
    realtime::UMP_GROUPS,
};
use slint::{Global, Model, ModelRc, SharedString, VecModel};

const DEFAULT_BUFFER_SIZE: u32 = 512;

pub fn sync_all(ui: &MainWindow, state: &AppData) {
    sync_sflist_to_slint(ui, state);
    sync_config_to_slint(ui, state);
    sync_renderer_to_slint(ui, state);
    sync_device_to_slint(ui, state);
    sync_audio_devices_to_slint(ui, state);
}

fn model_matches(model: &ModelRc<SharedString>, names: &[SharedString]) -> bool {
    model.row_count() == names.len()
        && names
            .iter()
            .enumerate()
            .all(|(i, n)| model.row_data(i).as_ref() == Some(n))
}

pub fn sync_audio_devices_to_slint(ui: &MainWindow, state: &AppData) {
    let audio = AudioDeviceState::get(ui);

    let hosts = with_default("System default", &state.audio.host_labels);
    if !model_matches(&audio.get_hosts(), &hosts) {
        audio.set_hosts(ModelRc::new(VecModel::from(hosts)));
    }

    let devices = with_default("System default", &state.audio.device_labels);
    if !model_matches(&audio.get_devices(), &devices) {
        audio.set_devices(ModelRc::new(VecModel::from(devices)));
    }

    // Falling back to the presets keeps the box usable when a device cannot be
    // probed, rather than leaving the user with an empty dropdown.
    let rates: Vec<SharedString> = if state.audio.sample_rates.is_empty() {
        COMMON_SAMPLE_RATES.iter().map(rate_label).collect()
    } else {
        state.audio.sample_rates.iter().map(rate_label).collect()
    };

    if !model_matches(&audio.get_sample_rates(), &rates) {
        audio.set_sample_rates(ModelRc::new(VecModel::from(rates)));
    }

    let (min, max) = state.audio.buffer_range.unwrap_or((0, 0));
    audio.set_buffer_min(min as i32);
    audio.set_buffer_max(max as i32);

    let sizes = buffer_size_labels(state.audio.buffer_range);
    if !model_matches(&audio.get_buffer_sizes(), &sizes) {
        audio.set_buffer_sizes(ModelRc::new(VecModel::from(sizes)));
    }

    let host = state
        .selected_config()
        .and_then(|c| c.realtime.audio_host.clone());
    audio.set_fixed_by_host(state.audio.fixed_by_host(host.as_deref()));
}

fn rate_label(rate: &u32) -> SharedString {
    rate.to_string().into()
}

fn buffer_size_labels(range: Option<(u32, u32)>) -> Vec<SharedString> {
    const FALLBACK: (u32, u32) = (32, 16384);
    const SMALLEST_LISTED: u32 = 16;

    let (min, max) = match range {
        Some((min, max)) if min <= max && max > 0 => (min, max),
        _ => FALLBACK,
    };

    let mut sizes: Vec<u32> = (0..=14)
        .map(|e| 1u32 << e)
        .filter(|s| *s >= min.max(SMALLEST_LISTED) && *s <= max)
        .collect();

    if sizes.is_empty() {
        sizes.push(min);
        if max != min {
            sizes.push(max);
        }
    }

    sizes
        .iter()
        .map(|s| SharedString::from(s.to_string()))
        .collect()
}

fn with_default(default: &str, labels: &[String]) -> Vec<SharedString> {
    std::iter::once(SharedString::from(default))
        .chain(labels.iter().map(SharedString::from))
        .collect()
}

/// Feeds the single model behind every `SflistPicker` in the app.
pub fn sync_available_sflists_to_slint(ui: &MainWindow, state: &AppData) {
    let names: Vec<SharedString> = state
        .available_sflists
        .iter()
        .map(|s| s.as_str().into())
        .collect();

    let sflists = SflistState::get(ui);
    if !model_matches(&sflists.get_available(), &names) {
        sflists.set_available(ModelRc::new(VecModel::from(names)));
    }
}

fn sync_converter_dirty_to_slint(ui: &MainWindow, state: &AppData) {
    let dirty = state
        .config(ConfigComponent::Converter)
        .is_some_and(|c| c.dirty);
    RendererState::get(ui).set_converter_settings_dirty(dirty);
}

// SoundFont list editor

fn current_slint_soundfonts(ui: &MainWindow, state: &AppData) -> Vec<SlintSoundFont> {
    let Some(file) = state.selected_sflist() else {
        return Vec::new();
    };
    let sub_idx = SoundfontEditorState::get(ui).get_selected_sublist_index();
    let Some(sublist) = file.sublist_name(sub_idx) else {
        return Vec::new();
    };

    file.list
        .lists
        .get(&sublist)
        .map(|sfs| sfs.iter().map(soundfont_to_slint).collect())
        .unwrap_or_default()
}

fn soundfont_to_slint(sf: &maestro_core::soundfont::SoundFont) -> SlintSoundFont {
    SlintSoundFont {
        enabled: sf.enabled,
        path: sf.path.to_string_lossy().to_string().into(),
        filename: sf
            .path
            .file_name()
            .unwrap_or(OsStr::new("N/A"))
            .to_string_lossy()
            .to_string()
            .into(),
        sf_type: sf.font_type().label().into(),
        bank: sf.bank as i32,
        preset: sf.preset as i32,
    }
}

/// Refreshes the SoundFont rows *without* replacing the model, so the repeated
/// row elements survive. Reordering by dragging depends on this: a new model
/// re-creates the rows, which cancels the drag handle's pointer grab after a
/// single move
pub fn sync_soundfont_rows_in_place(ui: &MainWindow, state: &AppData) -> bool {
    let rows = current_slint_soundfonts(ui, state);
    let model = SoundfontEditorState::get(ui).get_current_soundfonts();
    if model.row_count() != rows.len() {
        return false;
    }
    for (i, row) in rows.into_iter().enumerate() {
        model.set_row_data(i, row);
    }

    SoundfontEditorState::get(ui).set_file_dirty(dirty_flags_model(state));
    true
}

fn dirty_flags_model(state: &AppData) -> ModelRc<bool> {
    let dirty: Vec<bool> = state.sflist_files.iter().map(|f| f.dirty).collect();
    ModelRc::new(VecModel::from(dirty))
}

pub fn sync_sflist_to_slint(ui: &MainWindow, state: &AppData) {
    let editor = SoundfontEditorState::get(ui);

    let names: Vec<SharedString> = state
        .sflist_files
        .iter()
        .map(|f| f.name.as_str().into())
        .collect();
    editor.set_file_names(ModelRc::new(VecModel::from(names)));
    editor.set_file_dirty(dirty_flags_model(state));
    editor.set_selected_file_index(state.selected_sflist_idx);

    let Some(file) = state.selected_sflist() else {
        return;
    };

    let sublists = file.sorted_sublists();
    let sublist_names: Vec<SharedString> = sublists.iter().map(|s| s.as_str().into()).collect();
    editor.set_available_sublists(ModelRc::new(VecModel::from(sublist_names)));

    // Clamp a stale selection before reading the sub-list's SoundFonts, which
    // go through the (UI-selection-driven) helper below.
    let sel_sub_idx = editor.get_selected_sublist_index();
    if crate::state::index_of(sel_sub_idx, sublists.len()).is_none() {
        editor.set_selected_sublist_index(0);
    }

    let soundfonts = current_slint_soundfonts(ui, state);
    editor.set_current_soundfonts(ModelRc::new(VecModel::from(soundfonts)));

    let assignments: Vec<SlintPortAssignment> = (0..UMP_GROUPS)
        .map(|port| SlintPortAssignment {
            port: port as i32,
            list_name: file
                .list
                .port_assignments
                .get(&(port as usize))
                .cloned()
                .unwrap_or_default()
                .into(),
        })
        .collect();
    editor.set_current_port_assignments(ModelRc::new(VecModel::from(assignments)));
    editor.set_default_sublist(file.list.default_list.as_deref().unwrap_or("").into());
}

// Settings editor

pub fn build_slint_component_config(state: &AppData) -> Option<SlintComponentConfig> {
    let file = state.selected_config()?;
    let p = &file.profile;

    Some(SlintComponentConfig {
        name: ConfigComponent::from_name(&file.name)?
            .display_name()
            .into(),
        dirty: file.dirty,
        has_enabled: p.has_enabled,
        enabled: file.enabled,
        has_sflist: p.has_sflist,
        sflist: file.sflist.as_str().into(),
        // Resolved to a position so the ComboBox can be driven by
        // `current-index` (robust against the widget dropping its
        // `current-value` binding after the first user selection).
        sflist_index: state.available_sflist_index(&file.sflist),

        has_audio_params: p.has_audio_params,
        audio_params: SlintAudioParameters {
            channels: match file.audio_params.channels {
                ChannelCount::Mono => 1,
                ChannelCount::Stereo => 2,
            },
            sample_rate: file.audio_params.sample_rate as i32,
        },

        has_realtime: p.has_realtime,
        realtime: SlintRealtimeConfig {
            device_audio_params: file.realtime.device_audio_params,
            render_buffer_ms: file.realtime.render_buffer_ms,
            precision_playback: file.realtime.precision_playback,
            max_nps: file.realtime.max_nps.unwrap_or(100_000) as i32,
            has_max_nps: file.realtime.max_nps.is_some(),

            audio_host_index: state.audio.host_index(file.realtime.audio_host.as_deref()),
            output_device_index: state
                .audio
                .device_index(file.realtime.output_device.as_deref()),
            buffer_size: file.realtime.buffer_size.unwrap_or(DEFAULT_BUFFER_SIZE) as i32,
            has_buffer_size: file.realtime.buffer_size.is_some(),
        },

        has_renderer: p.has_renderer,
        renderer: SlintRendererConfig {
            port_threads: file.renderer.port_threads.unwrap_or(4) as i32,
            has_port_threads: file.renderer.port_threads.is_some(),
            render_fps: file.renderer.render_fps.unwrap_or(0.0) as f32,
            has_render_fps: file.renderer.render_fps.is_some(),
            render_fps_variation: file.renderer.render_fps_variation,
        },

        synth: synth_to_slint(&file.renderer.synth),
        has_event_processor: file.has_event_processor,
        event_processor: evproc_to_slint(&file.event_processor),
        has_post_processor: file.has_post_processor,
        post_processor: postproc_to_slint(&file.post_processor),

        has_port_threads: p.has_port_threads,
        has_converter_options: p.has_converter_options(),
        has_device_options: p.has_device_options(),
        converter_custom: converter_custom_to_slint(&file.converter_custom),
        system_custom: system_custom_to_slint(&file.system_custom),
    })
}

pub fn sync_config_to_slint(ui: &MainWindow, state: &AppData) {
    let editor = SettingsEditorState::get(ui);
    if let Some(cfg) = build_slint_component_config(state) {
        editor.set_current_config(cfg);
    }
    if editor.get_bitrates().row_count() == 0 {
        editor.set_bitrates(crate::slint_conv::bitrate_labels());
    }
    sync_available_sflists_to_slint(ui, state);
    sync_converter_dirty_to_slint(ui, state);
}

// Converter

pub fn sync_renderer_to_slint(ui: &MainWindow, state: &AppData) {
    let queue = ModelRc::new(VecModel::from(state.render_queue.clone()));
    RendererState::get(ui).set_render_queue(queue);
    sync_available_sflists_to_slint(ui, state);
    sync_converter_dirty_to_slint(ui, state);
}

// Virtual MIDI device

pub fn sync_device_to_slint(ui: &MainWindow, state: &AppData) {
    let dev = DeviceManagerState::get(ui);

    if let Some(sys) = state.config(ConfigComponent::System) {
        dev.set_system_custom(system_custom_to_slint(&sys.system_custom));
        dev.set_system(summary(state, sys));
    }
    if let Some(kd) = state.config(ConfigComponent::KDMAPI) {
        dev.set_kdmapi(summary(state, kd));
    }

    sync_available_sflists_to_slint(ui, state);
}

fn summary(state: &AppData, cfg: &crate::state::ComponentConfigCache) -> SlintComponentSummary {
    SlintComponentSummary {
        enabled: cfg.enabled,
        dirty: cfg.dirty,
        sflist_index: state.available_sflist_index(&cfg.sflist),
    }
}
