use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use slint::{ComponentHandle, Global, Model, Timer, TimerMode};

use crate::{
    AppState, DeviceManagerState, MainWindow, QuitStage, RendererState, actions, app::AppContext,
    errors, state::AppData, views,
};

const RENDER_POLL: Duration = Duration::from_millis(150);
static DISCARD_UNSAVED: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Runs for as long as the render prompt is up. However the run ends —
    /// cancelled from the prompt, cancelled from the progress modal behind it,
    /// or simply finished — the close attempt carries on from where it stopped,
    /// rather than leaving a prompt on screen about a render that has ended.
    static RENDER_WATCH: Timer = Timer::default();
}

pub fn setup(ui: &MainWindow, cx: &AppContext) {
    let state = AppState::get(ui);

    let c = cx.clone();
    state.on_cancel_quit(move || {
        RENDER_WATCH.with(Timer::stop);
        DISCARD_UNSAVED.store(false, Ordering::SeqCst);
        c.with_ui(|ui| AppState::get(ui).set_quit_stage(QuitStage::Idle));
    });

    // Render stage

    let c = cx.clone();
    state.on_cancel_render_and_quit(move || {
        c.with_ui(|ui| RendererState::get(ui).invoke_cancel_render());
    });

    // Unsaved stage

    let c = cx.clone();
    state.on_save_and_quit(move || {
        let c2 = c.clone();
        c.with(|ui, data| {
            if !save_all(ui, data) {
                // Stand down, so the error dialog explaining the failed write
                // is not left hidden behind the prompt.
                AppState::get(ui).set_quit_stage(QuitStage::Idle);
                return;
            }
            if advance(&c2, ui, data) {
                finish(ui);
            }
        });
    });

    let c = cx.clone();
    state.on_discard_and_quit(move || {
        DISCARD_UNSAVED.store(true, Ordering::SeqCst);
        resume(&c);
    });

    // Daemon stage

    let c = cx.clone();
    state.on_quit_keep_running(move || {
        c.with_ui(finish);
    });

    let c = cx.clone();
    state.on_quit_and_stop(move || {
        let Some(ui) = c.with_ui(|ui| {
            AppState::get(ui).set_quit_busy(true);
            ui.as_weak()
        }) else {
            return;
        };
        std::thread::spawn(move || {
            let failure = views::system_integration::service::stop().err();

            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = ui.upgrade() else {
                    return;
                };
                AppState::get(&ui).set_quit_busy(false);
                match failure {
                    Some(message) => {
                        AppState::get(&ui).set_quit_stage(QuitStage::Idle);
                        errors::report("Virtual MIDI Device", message);
                    }
                    None => finish(&ui),
                }
            });
        });
    });
}

pub fn intercept_close(cx: &AppContext) -> bool {
    cx.with(|ui, data| match AppState::get(ui).get_quit_stage() {
        QuitStage::Render | QuitStage::Unsaved => true,
        QuitStage::Daemon => false,
        QuitStage::Idle => !advance(cx, ui, data),
    })
    .unwrap_or(false)
}

fn advance(cx: &AppContext, ui: &MainWindow, data: &AppData) -> bool {
    let app = AppState::get(ui);
    let renderer = RendererState::get(ui);

    if renderer.get_is_rendering() {
        app.set_quit_prompt_detail(render_detail(renderer.get_render_jobs().row_count()).into());
        app.set_quit_stage(QuitStage::Render);
        watch_render(cx);
        return false;
    }

    if !DISCARD_UNSAVED.load(Ordering::SeqCst) {
        let unsaved = data.unsaved_labels();
        if !unsaved.is_empty() {
            app.set_quit_prompt_detail(unsaved_detail(&unsaved).into());
            app.set_quit_stage(QuitStage::Unsaved);
            return false;
        }
    }

    let device = DeviceManagerState::get(ui);
    if device.get_daemon_running() {
        app.set_quit_prompt_detail(daemon_detail(&device.get_daemon_memory()).into());
        app.set_quit_stage(QuitStage::Daemon);
        return false;
    }

    app.set_quit_stage(QuitStage::Idle);
    true
}

fn render_detail(files: usize) -> String {
    let what = match files {
        // The count is unknown for the moment between starting a run and the
        // first worker reporting in.
        0 => "A conversion is still in progress".to_string(),
        1 => "A file is still being converted".to_string(),
        n => format!("{n} files are still being converted"),
    };
    format!("{what}. Cancelling now leaves the output unfinished.")
}

fn unsaved_detail(labels: &[String]) -> String {
    let list = labels
        .iter()
        .map(|label| format!("  •  {label}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("These have changes that were never written to disk:\n\n{list}")
}

fn daemon_detail(memory: &str) -> String {
    if memory.is_empty() {
        "Maestro will keep running in the background after this window closes.".to_string()
    } else {
        format!(
            "Maestro will keep running in the background after this window closes, \
             holding {memory}."
        )
    }
}

fn watch_render(cx: &AppContext) {
    let cx = cx.clone();
    RENDER_WATCH.with(|timer| {
        if timer.running() {
            return;
        }
        timer.start(TimerMode::Repeated, RENDER_POLL, move || {
            if cx.with_ui(|ui| RendererState::get(ui).get_is_rendering()) == Some(true) {
                return;
            }
            RENDER_WATCH.with(Timer::stop);
            resume(&cx);
        });
    });
}

fn resume(cx: &AppContext) {
    cx.with(|ui, data| {
        if advance(cx, ui, data) {
            finish(ui);
        }
    });
}

fn finish(ui: &MainWindow) {
    RENDER_WATCH.with(Timer::stop);
    DISCARD_UNSAVED.store(false, Ordering::SeqCst);
    let app = AppState::get(ui);
    app.set_quit_busy(false);
    app.set_quit_stage(QuitStage::Idle);
    let _ = ui.hide();
}

fn save_all(ui: &MainWindow, data: &mut AppData) -> bool {
    let (lists, configs) = data.unsaved_indices();
    let mut saved = true;

    for idx in lists {
        if let Err(msg) = views::soundfont_editor::save_file_at(data, idx) {
            errors::report(views::soundfont_editor::ERROR_TITLE, msg);
            saved = false;
        }
    }
    data.refresh_available_sflists();
    actions::sync_after_sflist_change(ui, data);

    for idx in configs {
        saved &= actions::save_config_at(ui, data, idx, views::settings_editor::ERROR_TITLE);
    }

    saved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_render_prompt_counts_the_files_it_knows_about() {
        assert!(render_detail(1).starts_with("A file is still being converted."));
        assert!(render_detail(4).starts_with("4 files are still being converted."));
        // Before the first worker reports in there is no count to give.
        assert!(render_detail(0).starts_with("A conversion is still in progress."));
    }

    #[test]
    fn the_unsaved_prompt_lists_every_pending_edit() {
        let detail = unsaved_detail(&[
            "SoundFont list 'Default'".to_string(),
            "Converter settings".to_string(),
        ]);

        assert!(detail.contains("•  SoundFont list 'Default'"));
        assert!(detail.contains("•  Converter settings"));
    }

    #[test]
    fn the_daemon_prompt_mentions_the_memory_only_when_it_is_known() {
        assert!(daemon_detail("1.2 GB").contains("holding 1.2 GB"));
        assert!(!daemon_detail("").contains("holding"));
    }
}
