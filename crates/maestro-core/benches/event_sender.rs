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
        MaestroRealtimeEngine, RealtimeEngineOptions, RealtimeEventSender,
        config::{NpsLimit, RealtimeConfig},
        host_time,
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

    if let Some(path) = env::var_os("MAESTRO_BENCH_SF2")
        && let Err(err) = engine.set_soundfonts(SoundFontList::from(PathBuf::from(path)))
    {
        eprintln!("warning: soundfont failed to load ({err}), the render thread will idle");
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

/// The same events as MIDI bytes, `per_call` messages to a buffer.
fn long_buffers(events: &[u32], per_call: usize) -> Vec<Vec<u8>> {
    // Repeated `per_call` times so every buffer comes out full
    let events = events.repeat(per_call);
    events
        .chunks(per_call)
        .map(|chunk| {
            chunk
                .iter()
                .flat_map(|e| e.to_le_bytes().into_iter().take(3))
                .collect()
        })
        .collect()
}

fn drive<T>(
    b: &mut Bencher<'_, WallTime>,
    sender: &RealtimeEventSender,
    items: &[T],
    scheduled: bool,
    send: impl Fn(&RealtimeEventSender, &T, u64),
) {
    b.iter_custom(|iters| {
        let mut elapsed = Duration::ZERO;
        let mut sent = 0;

        while sent < iters {
            let count = CHUNK.min(iters - sent);

            let start = Instant::now();
            let at = if scheduled { host_time::host_now() } else { 0 };
            for i in 0..count {
                let item = &items[((sent + i) % items.len() as u64) as usize];
                send(sender, black_box(item), at);
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

fn short(sender: &RealtimeEventSender, event: &u32, at: u64) {
    sender.process_short(0, *event, at);
}

fn send_path(c: &mut Criterion) {
    let events = events();

    let bare = RealtimeConfig {
        auto_timestamp: false,
        max_nps: None,
        ..RealtimeConfig::default()
    };

    let mut group = c.benchmark_group("process_short");
    group.throughput(Throughput::Elements(1));

    let cases: Vec<(&str, RealtimeConfig, Option<EventProcessorConfig>)> = vec![
        ("bare", bare.clone(), None),
        (
            "auto_timestamp",
            RealtimeConfig {
                auto_timestamp: true,
                ..bare.clone()
            },
            None,
        ),
        (
            "nps_gate",
            RealtimeConfig {
                max_nps: Some(NpsLimit {
                    max: NPS_UNREACHABLE,
                    load_limiter: false,
                }),
                ..bare.clone()
            },
            None,
        ),
        (
            "nps_limited",
            RealtimeConfig {
                max_nps: Some(NpsLimit::default()),
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

        group.bench_function(name, |b| drive(b, &rig.sender, &events, false, short));
    }

    if let Some(rig) = rig(bare.clone(), None) {
        group.bench_function("scheduled", |b| drive(b, &rig.sender, &events, true, short));
    }

    if let Some(mut rig) = rig(bare.clone(), None)
        && rig.engine.pause().is_ok()
    {
        group.bench_function("bare_stream_paused", |b| {
            drive(b, &rig.sender, &events, false, short)
        });
    }

    group.finish();

    const PER_CALL: usize = 64;
    let buffers = long_buffers(&events, PER_CALL);

    let long =
        |sender: &RealtimeEventSender, bytes: &Vec<u8>, at| sender.process_long(0, bytes, at);

    let mut group = c.benchmark_group("process_long");
    group.throughput(Throughput::Elements(PER_CALL as u64));

    for (name, config) in [
        ("bare", bare.clone()),
        ("default", RealtimeConfig::default()),
    ] {
        if let Some(rig) = rig(config, None) {
            group.bench_function(name, |b| drive(b, &rig.sender, &buffers, false, long));
        }
    }

    if let Some(rig) = rig(bare, None) {
        group.bench_function("scheduled", |b| drive(b, &rig.sender, &buffers, true, long));
    }

    group.finish();
}

criterion_group!(benches, send_path);
criterion_main!(benches);
