use std::sync::{Mutex, OnceLock};

use maestro_core::{
    ipc::{SharedStats, process_name, publish},
    realtime::MaestroRealtimeStatistics,
    system_cfg::ConfigComponent,
};

struct Publisher {
    shared: SharedStats,
    live: Mutex<Vec<(usize, MaestroRealtimeStatistics)>>,
}

fn publisher() -> &'static Publisher {
    static PUBLISHER: OnceLock<Publisher> = OnceLock::new();
    PUBLISHER.get_or_init(|| {
        let shared = SharedStats::default();
        let _ = publish(
            &format!("Maestro API — {}", process_name()),
            shared.clone(),
            &ConfigComponent::Api,
            Default::default(),
        );
        Publisher {
            shared,
            live: Mutex::new(Vec::new()),
        }
    })
}

pub fn set(id: usize, stats: Option<MaestroRealtimeStatistics>) {
    let publisher = publisher();
    let mut live = publisher.live.lock().unwrap();
    live.retain(|(other, _)| *other != id);
    live.extend(stats.map(|s| (id, s)));
    *publisher.shared.lock().unwrap() = live.last().map(|(_, s)| s.clone());
}
