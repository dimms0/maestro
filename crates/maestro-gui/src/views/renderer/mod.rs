mod jobs;
mod worker;

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use slint::{ComponentHandle, Global, Model, ModelRc, VecModel};

use crate::{
    MainWindow, RenderStatus, RendererState, SlintArchiveEntry, SlintMidiRenderEntry,
    SlintRenderJob, actions,
    app::AppContext,
    errors,
    state::{AppData, ConfigComponent, index_of},
    sync::sync_renderer_to_slint,
};

use jobs::JobRegistry;
use worker::{ConverterRenderSettings, RenderRun};

pub const MIDI_FILTER: [&str; 17] = [
    "mid", "MID", "midi", "kar", "rmi", "midi2", "zip", "7z", "rar", "tar", "gz", "tgz", "xz",
    "txz", "bz2", "tbz2", "zst",
];

const ARCHIVE_ERROR: &str = "Could Not Open Archive";

pub fn clear_entry_statuses(ui: &MainWindow) {
    RendererState::get(ui)
        .set_render_statuses(ModelRc::new(VecModel::from(Vec::<RenderStatus>::new())));
}

fn queue_entries(ui: &MainWindow, data: &mut AppData, entries: Vec<(PathBuf, String)>) {
    let sflist = data.converter_default_sflist();
    let entries: Vec<_> = entries
        .iter()
        .filter_map(|(p, entry)| p.to_str().map(|p| (p, entry)))
        .map(|(p, entry)| SlintMidiRenderEntry {
            midi_path: p.into(),
            entry: entry.into(),
            sflist_name: sflist.as_str().into(),
        })
        .collect();
    if entries.is_empty() {
        return;
    }
    data.render_queue.extend(entries);
    clear_entry_statuses(ui);
    sync_renderer_to_slint(ui, data);
}

pub fn queue_midis(ui: &MainWindow, data: &mut AppData, paths: Vec<PathBuf>) {
    let mut entries = Vec::new();
    for path in paths {
        match midi_parser::archive::list(&path) {
            Ok(None) => entries.push((path, String::new())),
            Ok(Some(mut names)) if names.len() == 1 => entries.push((path, names.remove(0))),
            Ok(Some(names)) if names.is_empty() => {
                errors::report(ARCHIVE_ERROR, format!("\"{}\" holds no MIDI file.", path.display()));
            }
            Ok(Some(names)) => data.pending_archives.push((path, names)),
            Err(e) => errors::report(ARCHIVE_ERROR, format!("\"{}\":\n\n{e}", path.display())),
        }
    }
    queue_entries(ui, data, entries);
    if RendererState::get(ui).get_archive_path().is_empty() {
        show_next_archive(ui, data);
    }
}

fn show_next_archive(ui: &MainWindow, data: &mut AppData) {
    let state = RendererState::get(ui);
    let (path, names) = match data.pending_archives.first() {
        Some((path, names)) => (path.to_string_lossy().as_ref().into(), names.clone()),
        None => (Default::default(), Vec::new()),
    };
    let entries: Vec<_> =
        names.into_iter().map(|name| SlintArchiveEntry { name: name.into(), checked: true }).collect();
    state.set_archive_entries(ModelRc::new(VecModel::from(entries)));
    state.set_archive_path(path);
}

pub fn setup(ui: &MainWindow, cx: &AppContext) {
    let state = RendererState::get(ui);

    // Queue management

    let c = cx.clone();
    state.on_add_midis(move || {
        // The dialog is modal, so pick before borrowing the app data.
        let paths = actions::pick_files("MIDI", &MIDI_FILTER);
        c.with(|ui, data| queue_midis(ui, data, paths));
    });

    let c = cx.clone();
    state.on_close_archive_picker(move |queue| {
        c.with(|ui, data| {
            if data.pending_archives.is_empty() {
                return;
            }
            let (path, _) = data.pending_archives.remove(0);
            if queue {
                let entries = RendererState::get(ui)
                    .get_archive_entries()
                    .iter()
                    .filter(|e| e.checked)
                    .map(|e| (path.clone(), e.name.to_string()))
                    .collect();
                queue_entries(ui, data, entries);
            }
            show_next_archive(ui, data);
        });
    });

    let c = cx.clone();
    state.on_remove_midi_from_queue(move |idx| {
        c.with(|ui, data| {
            if let Some(i) = index_of(idx, data.render_queue.len()) {
                data.render_queue.remove(i);
            }
            clear_entry_statuses(ui);
            sync_renderer_to_slint(ui, data);
        });
    });

    let c = cx.clone();
    state.on_update_midi_sflist(move |idx, sflist| {
        c.with(|ui, data| {
            if let Some(i) = index_of(idx, data.render_queue.len()) {
                data.render_queue[i].sflist_name = sflist;
            }
            sync_renderer_to_slint(ui, data);
        });
    });

    // Render orchestration

    let cancel_flag = Arc::new(AtomicBool::new(false));

    let c = cx.clone();
    let flag = cancel_flag.clone();
    state.on_start_render(move || {
        let queue = match c.with(|_, data| data.render_queue.clone()) {
            Some(queue) if !queue.is_empty() => queue,
            _ => return,
        };
        flag.store(false, Ordering::SeqCst);
        if let Some(ui) = c.with_ui(|ui| ui.as_weak()) {
            spawn_render(ui, queue, flag.clone());
        }
    });

    let c = cx.clone();
    let flag = cancel_flag.clone();
    state.on_cancel_render(move || {
        flag.store(true, Ordering::SeqCst);
        c.with_ui(|ui| RendererState::get(ui).set_is_cancelling(true));
    });

    // Shortcuts into other views

    let c = cx.clone();
    state.on_open_converter_settings(move || {
        c.with(|ui, data| actions::open_component_settings(ui, data, ConfigComponent::Converter));
    });
}

fn spawn_render(
    ui: slint::Weak<MainWindow>,
    queue: Vec<SlintMidiRenderEntry>,
    cancel: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        set_render_state(&ui, true, queue.len());

        let settings = ConverterRenderSettings::load();
        let registry = JobRegistry::new(&ui);
        let run = RenderRun {
            settings: &settings,
            cancel: &cancel,
            ui: &ui,
            registry: &registry,
        };

        if settings.multithreaded {
            run.run_parallel(&queue);
        } else {
            run.run_sequential(&queue);
        }

        set_render_state(&ui, false, 0);
    });
}

fn set_render_state(ui_weak: &slint::Weak<MainWindow>, rendering: bool, queue_len: usize) {
    let ui = ui_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = ui.upgrade() else {
            return;
        };
        let state = RendererState::get(&ui);
        state.set_is_rendering(rendering);
        state.set_is_cancelling(false);
        state.set_render_jobs(ModelRc::new(VecModel::from(Vec::<SlintRenderJob>::new())));
        if rendering {
            state.set_render_statuses(ModelRc::new(VecModel::from(vec![
                RenderStatus::Pending;
                queue_len
            ])));
        }
    });
}
