use std::{
    env,
    hint::black_box,
    path::PathBuf,
    time::{Duration, Instant},
};

use criterion::{
    Bencher, Criterion, Throughput, criterion_group, criterion_main, measurement::WallTime,
};
use maestro_core::{
    realtime::{
        MaestroRealtimeEngine, RealtimeEngineOptions, RealtimeEventSender, config::RealtimeConfig,
    },
    renderer::config::EventProcessorConfig,
    soundfont::SoundFontList,
};

const CHUNK: u64 = 4096;

struct Rig {
    engine: MaestroRealtimeEngine,
    sender: RealtimeEventSender,
}

fn rig(config: RealtimeConfig, event_processor: Option<EventProcessorConfig>) -> Option<Rig> {
    let options = RealtimeEngineOptions {
        config,
        event_processor,
        ..Default::default()
    };

    let engine = match MaestroRealtimeEngine::new(options) {
        Ok(engine) => engine,
        Err(err) => {
            eprintln!("skipped: could not open an output stream ({err})");
            return None;
        }
    };

    if let Some(path) = env::var_os("MAESTRO_BENCH_SF2") {
        if let Err(err) = engine.set_soundfonts(SoundFontList::from(PathBuf::from(path))) {
            eprintln!("warning: soundfont failed to load ({err}), the render thread will idle");
        }
    }

    let sender = engine.get_event_sender();

    Some(Rig { engine, sender })
}

fn events() -> Vec<u32> {
    let mut events = Vec::new();

    for key in 36u32..96 {
        let channel = key % 4;

        events.push(0x90 | channel | key << 8 | 100 << 16);
        events.push(0x80 | channel | key << 8 | 64 << 16);

        if key % 16 == 0 {
            // Expression on channel 0: a message the NPS gate ignores.
            events.push(0xB0 | channel | 11 << 8 | (key % 128) << 16);
        }
    }

    events
}

fn drive(b: &mut Bencher<'_, WallTime>, sender: &RealtimeEventSender, events: &[u32]) {
    b.iter_custom(|iters| {
        let mut elapsed = Duration::ZERO;
        let mut sent = 0;

        while sent < iters {
            let count = CHUNK.min(iters - sent);

            let start = Instant::now();
            for i in 0..count {
                let event = events[((sent + i) % events.len() as u64) as usize];
                sender.process_short(0, black_box(event), None);
            }
            elapsed += start.elapsed();

            // Untimed: drains whatever the render thread has not taken yet.
            sender.reset();
            sent += count;
        }

        elapsed
    });
}

/// A ceiling no bench loop can reach, so the limiter runs but never rejects.
/// Kept well under `u64::MAX / 127`, which is where the limiter's own
/// `vel * max / 127` would overflow.
const NPS_UNREACHABLE: usize = 1_000_000_000;

fn send_path(c: &mut Criterion) {
    let events = events();

    let bare = RealtimeConfig {
        precision_playback: false,
        max_nps: None,
        ..RealtimeConfig::default()
    };

    let mut group = c.benchmark_group("process_short");
    group.throughput(Throughput::Elements(1));

    let cases: Vec<(&str, RealtimeConfig, Option<EventProcessorConfig>)> = vec![
        ("bare", bare.clone(), None),
        (
            "precision",
            RealtimeConfig {
                precision_playback: true,
                ..bare.clone()
            },
            None,
        ),
        (
            "nps_gate",
            RealtimeConfig {
                max_nps: Some(NPS_UNREACHABLE),
                ..bare.clone()
            },
            None,
        ),
        (
            "nps_limited",
            RealtimeConfig {
                max_nps: Some(100_000),
                ..bare.clone()
            },
            None,
        ),
        (
            "event_processor",
            bare.clone(),
            Some(EventProcessorConfig::default()),
        ),
        ("default", RealtimeConfig::default(), None),
    ];

    for (name, config, event_processor) in cases {
        let Some(rig) = rig(config, event_processor) else {
            return;
        };

        group.bench_function(name, |b| drive(b, &rig.sender, &events));
    }

    if let Some(mut rig) = rig(bare, None) {
        if rig.engine.pause().is_ok() {
            group.bench_function("bare_stream_paused", |b| drive(b, &rig.sender, &events));
        }
    }

    group.finish();
}

criterion_group!(benches, send_path);
criterion_main!(benches);
