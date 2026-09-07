mod jobs;
mod worker;

pub use worker::is_cancel_panic;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use slint::{ComponentHandle, Global, ModelRc, VecModel};

use crate::{
    MainWindow, RenderStatus, RendererState, SlintMidiRenderEntry, SlintRenderJob, actions,
    app::AppContext,
    state::{ConfigComponent, index_of},
    sync::sync_renderer_to_slint,
};

use jobs::JobRegistry;
use worker::{ConverterRenderSettings, RenderRun};

const MIDI_FILTER: [&str; 5] = ["mid", "MID", "midi", "kar", "rmi"];

pub fn clear_entry_statuses(ui: &MainWindow) {
    RendererState::get(ui)
        .set_render_statuses(ModelRc::new(VecModel::from(Vec::<RenderStatus>::new())));
}

pub fn setup(ui: &MainWindow, cx: &AppContext) {
    let state = RendererState::get(ui);

    // Queue management

    let c = cx.clone();
    state.on_add_midi_to_queue(move |path, sflist| {
        c.with(|ui, data| {
            data.render_queue.push(SlintMidiRenderEntry {
                midi_path: path,
                sflist_name: sflist,
            });
            clear_entry_statuses(ui);
            sync_renderer_to_slint(ui, data);
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

    state.on_browse_midi(|| actions::pick_file("MIDI", &MIDI_FILTER));

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
        // Workers only observe the flag between render batches, which can take
        // a while on heavy files — show a disabled "Cancelling…" state instead
        // of pretending the render already stopped.
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
