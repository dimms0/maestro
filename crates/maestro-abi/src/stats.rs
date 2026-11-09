use std::sync::{Mutex, OnceLock, Weak};

use maestro_core::{
    ipc::{PublishOptions, SharedStats, process_name, publish},
    realtime::MaestroRealtimeStatistics,
    system_cfg::ConfigComponent,
};

use crate::realtime::Shared;

struct Publisher {
    shared: SharedStats,
    live: Mutex<Vec<(usize, MaestroRealtimeStatistics)>>,
    instances: Mutex<Vec<Weak<Shared>>>,
}

fn publisher() -> &'static Publisher {
    static PUBLISHER: OnceLock<Publisher> = OnceLock::new();
    PUBLISHER.get_or_init(|| {
        let shared = SharedStats::default();
        let _ = publish(
            &format!("Maestro API — {}", process_name()),
            shared.clone(),
            &ConfigComponent::Api,
            PublishOptions {
                on_reset: Some(Box::new(reset_all)),
                ..Default::default()
            },
        );
        Publisher {
            shared,
            live: Mutex::new(Vec::new()),
            instances: Mutex::new(Vec::new()),
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

pub fn register(instance: Weak<Shared>) {
    let mut instances = publisher().instances.lock().unwrap();
    instances.retain(|i| i.strong_count() > 0);
    instances.push(instance);
}

fn reset_all() {
    let instances = publisher().instances.lock().unwrap().clone();
    for instance in instances.iter().filter_map(Weak::upgrade) {
        let _ = instance.reset();
    }
}
