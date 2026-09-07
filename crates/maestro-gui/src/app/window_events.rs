use std::path::Path;

use slint::{ComponentHandle, Global};

use crate::{
    AppState, DeviceManagerState, MainWindow, RendererState, SettingsEditorState,
    SlintMidiRenderEntry, SoundfontEditorState, StandaloneMode, Tab, actions,
    app::AppContext,
    errors,
    state::AppData,
    sync::{sync_renderer_to_slint, sync_sflist_to_slint},
    views::renderer,
};
use maestro_core::soundfont::SoundFont;

const MIDI_EXTENSIONS: [&str; 4] = ["mid", "midi", "kar", "rmi"];
const SOUNDFONT_EXTENSIONS: [&str; 3] = ["sf2", "sf3", "sfz"];

pub fn setup(ui: &MainWindow, cx: &AppContext) {
    use slint::winit_030::{EventResult, WinitWindowAccessor, winit};

    let cx = cx.clone();
    ui.window().on_winit_window_event(move |_window, event| {
        match event {
            winit::event::WindowEvent::DroppedFile(path) => {
                cx.with(|ui, data| handle_drop(ui, data, path));
            }
            winit::event::WindowEvent::CloseRequested => {
                let mut intercepted = false;
                cx.with_ui(|ui| intercepted = confirm_quit(ui));
                if intercepted {
                    return EventResult::PreventDefault;
                }
            }
            _ => {}
        }
        EventResult::Propagate
    });
}

fn confirm_quit(ui: &MainWindow) -> bool {
    let device = DeviceManagerState::get(ui);
    let app = AppState::get(ui);

    if app.get_quit_prompt_open() || !device.get_daemon_running() {
        return false;
    }

    let memory = device.get_daemon_memory();
    app.set_quit_prompt_detail(if memory.is_empty() {
        "Maestro will keep running in the background after this window closes.".into()
    } else {
        format!(
            "Maestro will keep running in the background after this window closes, \
             holding {memory}."
        )
        .into()
    });
    app.set_quit_prompt_open(true);
    true
}

fn handle_drop(ui: &MainWindow, data: &mut AppData, path: &Path) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    if MIDI_EXTENSIONS.contains(&ext.as_str()) {
        queue_midi(ui, data, path);
    } else if SOUNDFONT_EXTENSIONS.contains(&ext.as_str()) {
        add_soundfont(ui, data, path);
    }
}

fn queue_midi(ui: &MainWindow, data: &mut AppData, path: &Path) {
    // The render queue only exists in combined mode, and it must not
    // change under a running render
    if data.standalone_mode != StandaloneMode::Combined || RendererState::get(ui).get_is_rendering()
    {
        return;
    }
    let Some(path) = path.to_str() else {
        return;
    };

    let sflist = data
        .available_sflists
        .first()
        .cloned()
        .unwrap_or_else(|| actions::FALLBACK_SFLIST.to_string());
    data.render_queue.push(SlintMidiRenderEntry {
        midi_path: path.into(),
        sflist_name: sflist.into(),
    });

    renderer::clear_entry_statuses(ui);
    sync_renderer_to_slint(ui, data);
    AppState::get(ui).set_active_tab(Tab::Converter);
}

fn add_soundfont(ui: &MainWindow, data: &mut AppData, path: &Path) {
    if data.standalone_mode == StandaloneMode::EditConfig && SettingsEditorState::get(ui).get_open()
    {
        return;
    }
    let sub_idx = SoundfontEditorState::get(ui).get_selected_sublist_index();
    let combined = data.standalone_mode == StandaloneMode::Combined;

    let Some(file) = data.selected_sflist_mut() else {
        errors::report(
            "SoundFont Drop",
            "Select a SoundFont list in the SoundFont List Editor before dropping SoundFont files onto the window.",
        );
        return;
    };
    let Some(sublist) = file.sublist_name(sub_idx) else {
        return;
    };

    file.list.lists.entry(sublist).or_default().push(SoundFont {
        enabled: true,
        path: path.to_path_buf(),
        bank: 0,
        preset: -1,
    });
    file.dirty = true;

    sync_sflist_to_slint(ui, data);
    if combined {
        AppState::get(ui).set_active_tab(Tab::Soundfonts);
    }
}
