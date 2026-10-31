//! Sending a run of events as `process_short` calls vs packed into
//! `process_long` buffers of various sizes, the way KDMAPI's `SendDirectData`
//! and `SendDirectLongData` reach the engine.
//!
//! Criterion's run-to-run noise on a `powersave` machine is bigger than the
//! gaps this looks for, so this is a plain harness instead: every short/long
//! pair is measured back to back inside each round, rounds are repeated, and
//! the minimum is reported. Pin it to one core for stable numbers:
//!
//! ```sh
//! taskset -c 4 cargo bench -p maestro-core --bench long_vs_short [filter]
//! ```
//!
//! Environment:
//! - `MAESTRO_BENCH_ROUNDS`: rounds per case (default 7).
//! - `MAESTRO_BENCH_EVENTS`: events sent per measurement (default 1Mi).
//! - `MAESTRO_BENCH_LIVE=1`: keep the output stream running, so the render
//!   thread competes for the port queues. Paused by default to cut noise.
//! - `MAESTRO_BENCH_SF2`: soundfont to load (only matters when live).

use std::{
    env,
    hint::black_box,
    path::PathBuf,
    time::{Duration, Instant},
};

use maestro_core::{
    realtime::{
        MaestroRealtimeEngine, RealtimeEngineOptions, RealtimeEventSender,
        config::{NpsLimit, RealtimeConfig},
    },
    soundfont::SoundFontList,
};

/// Events per long buffer. 1 is the degenerate case of a long carrying a
/// single message; the top end is ~48 KB, near a MIDIHDR's practical limit.
const SIZES: &[usize] = &[1, 2, 4, 8, 16, 32, 64, 128, 256, 1024, 4096, 16384];

/// Ports are drained (untimed) after this many events in both paths, so queue
/// growth is identical whichever way the events arrive.
const DRAIN: usize = 16384;

/// Distinct events in a payload before it repeats.
const PAYLOAD_LEN: usize = 1 << 16;

struct Payload {
    name: &'static str,
    shorts: Vec<u32>,
    running_status: bool,
}

/// Packed long buffers for one chunk size, laid out back to back.
struct Longs {
    bytes: Vec<u8>,
    /// `bytes[ends[i - 1]..ends[i]]` holds chunk `i`.
    ends: Vec<usize>,
}

impl Longs {
    fn new(payload: &Payload, size: usize) -> Self {
        let mut bytes = Vec::with_capacity(payload.shorts.len() * 3);
        let mut ends = Vec::with_capacity(payload.shorts.len() / size);

        for chunk in payload.shorts.chunks_exact(size) {
            // Every buffer opens with a full status byte, since a drain also
            // clears the translator's running status.
            let mut last = 0u8;
            for &short in chunk {
                let status = short as u8;
                if !payload.running_status || status != last {
                    bytes.push(status);
                }
                last = status;

                bytes.push((short >> 8) as u8);
                if !matches!(status & 0xF0, 0xC0 | 0xD0) {
                    bytes.push((short >> 16) as u8);
                }
            }
            ends.push(bytes.len());
        }

        Self { bytes, ends }
    }

    fn chunk(&self, i: usize) -> &[u8] {
        let i = i % self.ends.len();
        let start = if i == 0 { 0 } else { self.ends[i - 1] };
        &self.bytes[start..self.ends[i]]
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

fn black_midi(channels: u32, controllers: bool, vel0_off: bool) -> Vec<u32> {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut held = std::collections::VecDeque::new();
    let mut out = Vec::with_capacity(PAYLOAD_LEN);

    while out.len() < PAYLOAD_LEN {
        let channel = rng.below(channels);

        if controllers && rng.below(100) < 4 {
            out.push(match rng.below(3) {
                0 => 0xB0 | channel | 11 << 8 | rng.below(128) << 16,
                1 => 0xE0 | channel | rng.below(128) << 8 | rng.below(128) << 16,
                _ => 0xB0 | channel | 64 << 8 | (rng.below(2) * 127) << 16,
            });
            continue;
        }

        // Keep ~256 notes sounding: start one, or end the oldest.
        if held.len() < 64 || (held.len() < 512 && rng.below(2) == 0) {
            let key = 21 + rng.below(88);
            out.push(0x90 | channel | key << 8 | (1 + rng.below(127)) << 16);
            held.push_back((channel, key));
        } else {
            let (channel, key) = held.pop_front().unwrap();
            out.push(if vel0_off {
                0x90 | channel | key << 8
            } else {
                0x80 | channel | key << 8 | 64 << 16
            });
        }
    }

    out
}

fn payloads() -> Vec<Payload> {
    vec![
        // Events merged from many tracks: the channel changes on nearly every
        // message, so running status would rarely apply even if used.
        Payload {
            name: "16ch",
            shorts: black_midi(16, true, false),
            running_status: false,
        },
        // A single spam track with note-on velocity 0 releases, packed with
        // running status, so most events cost 2 bytes in the long buffer.
        Payload {
            name: "1ch_rs",
            shorts: black_midi(1, false, true),
            running_status: true,
        },
    ]
}

fn configs() -> Vec<(&'static str, RealtimeConfig)> {
    vec![
        // Every event is admitted and queued.
        (
            "no_limit",
            RealtimeConfig {
                auto_timestamp: false,
                max_nps: None,
                ..RealtimeConfig::default()
            },
        ),
        // The limiter is on but never rejects, so its bookkeeping is paid on
        // every note while all of them still get stamped and queued.
        (
            "nps_open",
            RealtimeConfig {
                max_nps: Some(NpsLimit {
                    max: 1_000_000_000,
                    load_limiter: false,
                }),
                ..RealtimeConfig::default()
            },
        ),
        // Shipping defaults. An unpaced sender saturates the 400k limit, so
        // most notes are rejected, which is what black MIDI hits in practice.
        ("default", RealtimeConfig::default()),
    ]
}

fn engine(config: RealtimeConfig, live: bool) -> Option<MaestroRealtimeEngine> {
    let options = RealtimeEngineOptions {
        config,
        ..Default::default()
    };

    let mut engine = match MaestroRealtimeEngine::new(options) {
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

    if !live && let Err(err) = engine.pause() {
        eprintln!("warning: could not pause the stream ({err}), measuring it live");
    }

    Some(engine)
}

/// Nanoseconds per event sending `total` events as shorts.
fn time_shorts(sender: &RealtimeEventSender, shorts: &[u32], total: usize) -> f64 {
    let mut elapsed = Duration::ZERO;
    let mut sent = 0;

    sender.reset();
    while sent < total {
        let start = Instant::now();
        for i in sent..sent + DRAIN {
            sender.process_short(0, black_box(shorts[i % shorts.len()]), 0);
        }
        elapsed += start.elapsed();

        sender.reset();
        sent += DRAIN;
    }

    elapsed.as_nanos() as f64 / sent as f64
}

/// Nanoseconds per event sending `total` events as longs of `size` events.
fn time_longs(sender: &RealtimeEventSender, longs: &Longs, size: usize, total: usize) -> f64 {
    let calls_per_drain = (DRAIN / size).max(1);
    let mut elapsed = Duration::ZERO;
    let mut call = 0;
    let mut sent = 0;

    sender.reset();
    while sent < total {
        let start = Instant::now();
        for i in call..call + calls_per_drain {
            sender.process_long(0, black_box(longs.chunk(i)), 0);
        }
        elapsed += start.elapsed();

        sender.reset();
        call += calls_per_drain;
        sent += calls_per_drain * size;
    }

    elapsed.as_nanos() as f64 / sent as f64
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    // `cargo bench` passes `--bench`; anything else is a substring filter on
    // the `config/payload` case name.
    let filter: Vec<String> = env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();

    let rounds = env_usize("MAESTRO_BENCH_ROUNDS", 7).max(1);
    let total = env_usize("MAESTRO_BENCH_EVENTS", 1 << 20).max(DRAIN);
    let live = env::var_os("MAESTRO_BENCH_LIVE").is_some_and(|v| v == "1");

    println!(
        "long_vs_short: {rounds} rounds x {total} events, min ns/event, stream {}",
        if live { "live" } else { "paused" }
    );

    let payloads = payloads();

    for (config_name, config) in configs() {
        let cases: Vec<&Payload> = payloads
            .iter()
            .filter(|p| {
                let name = format!("{config_name}/{}", p.name);
                filter.is_empty() || filter.iter().any(|f| name.contains(f.as_str()))
            })
            .collect();
        if cases.is_empty() {
            continue;
        }

        let Some(engine) = engine(config, live) else {
            return;
        };
        let sender = engine.get_event_sender();

        for payload in cases {
            let longs: Vec<Longs> = SIZES.iter().map(|&n| Longs::new(payload, n)).collect();
            let largest = &longs[longs.len() - 1];
            let bytes_per_event = largest.bytes.len() as f64 / payload.shorts.len() as f64;

            let mut short_best = f64::INFINITY;
            let mut long_best = vec![f64::INFINITY; SIZES.len()];

            // Warm up caches, the allocator, and the CPU clock.
            time_shorts(&sender, &payload.shorts, total);

            for round in 0..rounds {
                // Alternate the order so neither path always runs first.
                for (i, (&size, longs)) in SIZES.iter().zip(&longs).enumerate() {
                    let long = |best: &mut f64| {
                        *best = best.min(time_longs(&sender, longs, size, total));
                    };

                    if (round + i) % 2 == 0 {
                        short_best = short_best.min(time_shorts(&sender, &payload.shorts, total));
                        long(&mut long_best[i]);
                    } else {
                        long(&mut long_best[i]);
                        short_best = short_best.min(time_shorts(&sender, &payload.shorts, total));
                    }
                }
            }

            println!();
            println!(
                "{config_name}/{} ({bytes_per_event:.2} bytes/event in longs)",
                payload.name
            );
            println!(
                "  {:>13}  {:>10}  {:>10}  {:>9}  {:>10}",
                "events/long", "short", "long", "long/short", "ns/call"
            );
            for (i, &size) in SIZES.iter().enumerate() {
                println!(
                    "  {:>13}  {:>10.2}  {:>10.2}  {:>9.2}x  {:>10.1}",
                    size,
                    short_best,
                    long_best[i],
                    long_best[i] / short_best,
                    long_best[i] * size as f64,
                );
            }
        }

        drop(sender);
        drop(engine);
    }
}
