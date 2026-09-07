use std::{thread, time::Duration};

use maestro_core::ipc::{
    MaestroStats, STATE_LIVE, STATE_OFF, STATE_PAUSED, STATE_STARTING, StatsSubscriber,
};
use maestro_core::notify::format_bytes;
use maestro_core::system_cfg::ConfigComponent;
use slint::{ComponentHandle, Global, Model, ModelRc, VecModel};

use crate::{DeviceManagerState, MainWindow, SlintComponentStats, utils::fmt_count};

const REFRESH: Duration = Duration::from_millis(50);
const IDLE_REFRESH: Duration = Duration::from_millis(500);

struct DaemonStatus {
    running: bool,
    state: i32,
    memory: String,
}

pub fn spawn(ui: &MainWindow) {
    let ui = ui.as_weak();

    thread::Builder::new()
        .name("maestro-gui-stats".to_string())
        .spawn(move || {
            let mut subscriber = StatsSubscriber::new();
            let mut previous: Vec<MaestroStats> = Vec::new();

            loop {
                let frames = subscriber.poll();

                // Do not repaint if nothing changed
                if frames != previous {
                    let rows: Vec<SlintComponentStats> = frames.iter().map(to_slint).collect();
                    let daemon = daemon_status(&frames);
                    let ui = ui.clone();
                    let sent = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui.upgrade() {
                            publish_rows(&ui, rows);
                            publish_daemon(&ui, daemon);
                        }
                    });

                    if sent.is_err() {
                        return;
                    }
                    previous = frames;
                }

                thread::sleep(if previous.is_empty() {
                    IDLE_REFRESH
                } else {
                    REFRESH
                });
            }
        })
        .ok();
}

fn daemon_status(frames: &[MaestroStats]) -> DaemonStatus {
    let daemon = frames.iter().find(|f| {
        f.component() == Some(ConfigComponent::System)
            && f.name() == maestro_core::ipc::DAEMON_LABEL
    });

    match daemon {
        Some(frame) => DaemonStatus {
            running: true,
            state: frame.state as i32,
            memory: format_bytes(frame.rss_bytes),
        },
        None => DaemonStatus {
            running: false,
            state: STATE_OFF as i32,
            memory: String::new(),
        },
    }
}

fn publish_daemon(ui: &MainWindow, daemon: DaemonStatus) {
    let state = DeviceManagerState::get(ui);
    state.set_daemon_running(daemon.running);
    state.set_daemon_state(daemon.state);
    state.set_daemon_memory(daemon.memory.into());
    state.set_daemon_busy(false);
}

fn publish_rows(ui: &MainWindow, rows: Vec<SlintComponentStats>) {
    let state = DeviceManagerState::get(ui);
    let model = state.get_stats();

    if let Some(model) = model
        .as_any()
        .downcast_ref::<VecModel<SlintComponentStats>>()
        && model.row_count() == rows.len()
    {
        for (row, new) in rows.into_iter().enumerate() {
            if model.row_data(row).as_ref() != Some(&new) {
                model.set_row_data(row, new);
            }
        }
        return;
    }

    state.set_stats(ModelRc::new(VecModel::from(rows)));
}

fn state_text(state: u32) -> &'static str {
    match state {
        STATE_STARTING => "Loading soundfonts…",
        STATE_LIVE => "Ready",
        STATE_PAUSED => "Paused after a quiet spell",
        _ => "No engine loaded",
    }
}

fn to_slint(stats: &MaestroStats) -> SlintComponentStats {
    SlintComponentStats {
        name: stats.name().into(),
        state: stats.state as i32,
        state_text: state_text(stats.state).into(),
        memory: format_bytes(stats.rss_bytes).into(),
        voices: fmt_count(stats.voices).into(),
        render_load_txt: format!("{:.2}%", stats.render_last).into(),
        render_load: stats.render_avg,
    }
}
