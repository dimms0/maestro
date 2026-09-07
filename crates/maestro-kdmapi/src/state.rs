use std::sync::{Mutex, OnceLock, atomic::AtomicPtr};

use maestro_core::{
    ipc::{SharedStats, process_name, publish},
    realtime::{MaestroRealtimeEngine, RealtimeEventSender},
    system_cfg::ConfigComponent,
};

use crate::watcher::WatcherHandle;

pub struct State {
    pub realtime: Mutex<Option<MaestroRealtimeEngine>>,
    pub sender: AtomicPtr<RealtimeEventSender>,
    pub statistics: SharedStats,
    pub watcher: Mutex<Option<WatcherHandle>>,
}

// the realtime engine owns a cpal, which is not Sync on all
// platforms; access to it is serialized through the realtime mutex.
unsafe impl Sync for State {}

pub(super) fn get_state() -> &'static State {
    static GLOBAL_STATE: OnceLock<State> = OnceLock::new();
    GLOBAL_STATE.get_or_init(|| {
        let statistics = SharedStats::default();

        if let Err(err) = publish(
            &format!("KDMAPI — {}", process_name()),
            statistics.clone(),
            &ConfigComponent::KDMAPI,
            Default::default(),
        ) {
            crate::log_info(format!("Statistics are unavailable: {err}"));
        }

        State {
            realtime: Mutex::new(None),
            sender: AtomicPtr::new(std::ptr::null_mut()),
            statistics,
            watcher: Mutex::new(None),
        }
    })
}
