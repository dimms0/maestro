//! * `MAESTRO_BENCH_SF2` — soundfont to load
//! * `MAESTRO_BENCH_SYNTH` — `bassmidi` or `fluidsynth`
//! * `MAESTRO_BENCH_MIDI` — a MIDI file to bench alongside the generated ones
//! * `MAESTRO_BENCH_SECONDS` — how much audio each generated file covers, default 5
//! * `MAESTRO_BENCH_TIME` — seconds criterion spends per benchmark, default 20
//! * `MAESTRO_BENCH_OUT` — where the rendered audio is written

use std::{
    env, fs,
    io::BufWriter,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use maestro_core::{
    audio_params::AudioParameters,
    file_renderer::{MaestroFileRenderer, OutputSettings, PortMode},
    renderer::{
        config::{
            EventProcessorConfig, RendererConfig, SynthConfig, bassmidi::BASSMIDIConfig,
            fluidsynth::FluidSynthConfig,
        },
        probe_libraries,
    },
    soundfont::SoundFontList,
};
use midi_parser::{Division, EventRef, Meta, MidiMessage, SmfWriter, meta};

const PPQ: u16 = 24_000;
const TEMPO_MICROS: u32 = 500_000;
const TICKS_PER_SEC: u32 = PPQ as u32 * 2;

fn seconds() -> u32 {
    env::var("MAESTRO_BENCH_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|s| *s > 0)
        .unwrap_or(5)
}

fn measurement_secs() -> u64 {
    env::var("MAESTRO_BENCH_TIME")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|s| *s > 0)
        .unwrap_or(20)
}

/// Deterministic PRNG, so every run and every revision benches the same notes
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u32) -> u32 {
        (self.next() >> 33) as u32 % n.max(1)
    }
}

struct Workload {
    name: &'static str,
    chords_per_sec: u32,
    chord_size: u32,
    ccs_per_sec: u32,
    ports: u8,
    chord_spread_samples: u32,
}

const WORKLOADS: &[Workload] = &[
    Workload {
        name: "sparse",
        chords_per_sec: 6,
        chord_size: 4,
        ccs_per_sec: 16,
        ports: 1,
        chord_spread_samples: 0,
    },
    Workload {
        name: "dense",
        chords_per_sec: 80,
        chord_size: 12,
        ccs_per_sec: 240,
        ports: 1,
        chord_spread_samples: 0,
    },
    Workload {
        name: "extreme",
        chords_per_sec: 300_000,
        chord_size: 16,
        ccs_per_sec: 100_000,
        ports: 1,
        chord_spread_samples: 0,
    },
    Workload {
        name: "burst",
        chords_per_sec: 500,
        chord_size: 10_000,
        ccs_per_sec: 0,
        ports: 1,
        chord_spread_samples: 1,
    },
    Workload {
        name: "controllers",
        chords_per_sec: 2,
        chord_size: 2,
        ccs_per_sec: 200_000,
        ports: 1,
        chord_spread_samples: 0,
    },
    Workload {
        name: "multi_port",
        chords_per_sec: 80,
        chord_size: 12,
        ccs_per_sec: 240,
        ports: 4,
        chord_spread_samples: 0,
    },
];

fn generate(workload: &Workload, seconds: u32, path: &Path) -> u64 {
    let total_ticks = TICKS_PER_SEC * seconds;
    let tracks = workload.ports.max(1) as usize;

    let mut per_track: Vec<Vec<(u32, MidiMessage)>> = vec![Vec::new(); tracks];
    let mut rng = Rng(0x5EED_0F11_CE07_1234);

    let chords = workload.chords_per_sec * seconds;

    let hold = (TICKS_PER_SEC / workload.chords_per_sec.max(1)).max(1) * 3 / 2;
    let spread_span = workload.chord_spread_samples + 1;

    for i in 0..chords {
        let tick = (u64::from(i) * u64::from(total_ticks) / u64::from(chords.max(1))) as u32;
        let track = i as usize % tracks;

        for n in 0..workload.chord_size {
            let channel = rng.below(16) as u8;
            let key = 21 + rng.below(88) as u8;
            let vel = 40 + rng.below(80) as u8;
            let offset = n % spread_span;

            per_track[track].push((
                tick + offset,
                MidiMessage {
                    status: 0x90 | channel,
                    data1: key,
                    data2: vel,
                },
            ));
            per_track[track].push((
                tick + hold + offset,
                MidiMessage {
                    status: 0x80 | channel,
                    data1: key,
                    data2: 0,
                },
            ));
        }
    }

    let ccs = workload.ccs_per_sec * seconds;
    for i in 0..ccs {
        let tick = (u64::from(i) * u64::from(total_ticks) / u64::from(ccs.max(1))) as u32;
        let track = i as usize % tracks;
        let channel = (i % 16) as u8;

        let message = if i.is_multiple_of(2) {
            MidiMessage {
                status: 0xB0 | channel,
                data1: 11,
                data2: (i % 128) as u8,
            }
        } else {
            MidiMessage {
                status: 0xE0 | channel,
                data1: (i % 128) as u8,
                data2: 64,
            }
        };

        per_track[track].push((tick, message));
    }

    let file = fs::File::create(path).expect("could not write the generated MIDI file");
    let mut writer = SmfWriter::new(BufWriter::new(file), 1, Division::Ppq(PPQ), tracks as u16)
        .expect("could not write the MIDI header");

    let mut count = 0u64;

    for (index, events) in per_track.iter_mut().enumerate() {
        events.sort_by_key(|(tick, _)| *tick);

        writer.begin_track();

        if index == 0 {
            writer
                .event(
                    0,
                    EventRef::Meta(Meta {
                        kind: meta::SET_TEMPO,
                        data: &TEMPO_MICROS.to_be_bytes()[1..],
                    }),
                )
                .unwrap();
        }

        writer
            .event(
                0,
                EventRef::Meta(Meta {
                    kind: meta::MIDI_PORT,
                    data: &[index as u8],
                }),
            )
            .unwrap();

        let mut last = 0u32;
        for (tick, message) in events.iter() {
            writer.event(tick - last, EventRef::Midi(*message)).unwrap();
            last = *tick;
            count += 1;
        }

        writer
            .event(
                total_ticks.saturating_sub(last),
                EventRef::Meta(Meta {
                    kind: meta::END_OF_TRACK,
                    data: &[],
                }),
            )
            .unwrap();
        writer.end_track().unwrap();
    }

    writer.finish().unwrap();
    count
}

fn generate_empty(seconds: u32, path: &Path) {
    let empty = Workload {
        name: "startup",
        chords_per_sec: 0,
        chord_size: 0,
        ccs_per_sec: 0,
        ports: 1,
        chord_spread_samples: 0,
    };
    generate(&empty, seconds, path);
}

fn pick_synth() -> Option<SynthConfig> {
    let libraries = probe_libraries();
    let usable = |name: &str| {
        libraries
            .iter()
            .any(|lib| lib.name == name && lib.error.is_none())
    };

    let bassmidi = usable("BASS") && usable("BASSMIDI");
    let fluidsynth = usable("FluidSynth");

    let bass_cfg = || {
        eprintln!("synth: BASSMIDI");
        SynthConfig::BASSMIDI(BASSMIDIConfig {
            voice_limit: 1024,
            multithreading: Some(Default::default()),
            ..Default::default()
        })
    };
    let fluid_cfg = || {
        eprintln!("synth: FluidSynth");
        SynthConfig::FluidSynth(FluidSynthConfig::default())
    };

    match env::var("MAESTRO_BENCH_SYNTH")
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Ok("bassmidi") | Ok("bass") if bassmidi => Some(bass_cfg()),
        Ok("fluidsynth") | Ok("fluid") if fluidsynth => Some(fluid_cfg()),
        Ok(requested) => {
            eprintln!("skipped: MAESTRO_BENCH_SYNTH={requested} is not available");
            None
        }
        Err(_) if bassmidi => Some(bass_cfg()),
        Err(_) if fluidsynth => Some(fluid_cfg()),
        Err(_) => {
            eprintln!("skipped: neither BASSMIDI nor FluidSynth could be loaded");
            None
        }
    }
}

fn soundfonts() -> SoundFontList {
    match env::var_os("MAESTRO_BENCH_SF2") {
        Some(path) => SoundFontList::from(PathBuf::from(path)),
        None => {
            eprintln!(
                "warning: MAESTRO_BENCH_SF2 is not set, so no voices will sound. \
                 The event path is still measured; the synth's DSP is not."
            );
            SoundFontList::default()
        }
    }
}

struct Rig {
    synth: SynthConfig,
    soundfonts: SoundFontList,
    audio: AudioParameters,
    out_root: PathBuf,
    run: std::cell::Cell<u64>,
}

impl Rig {
    fn render_once(
        &self,
        midi: &Path,
        config: RendererConfig,
        evproc: Option<EventProcessorConfig>,
        ports: PortMode,
    ) -> Duration {
        let run = self.run.get();
        self.run.set(run + 1);
        let out_dir = self.out_root.join(format!("run-{run}"));

        let mut renderer =
            MaestroFileRenderer::new(config, self.audio, self.soundfonts.clone(), midi, &out_dir)
                .expect("the renderer rejected the generated MIDI file")
                .with_output_settings(OutputSettings::default())
                .with_port_mode(ports);

        if let Some(evproc) = evproc {
            renderer = renderer.with_event_processor(evproc);
        }

        let start = Instant::now();
        renderer.render().expect("render failed");
        let elapsed = start.elapsed();

        let _ = fs::remove_dir_all(&out_dir);
        elapsed
    }
}

fn file_render(c: &mut Criterion) {
    let Some(synth) = pick_synth() else {
        return;
    };

    let seconds = seconds();
    let midi_dir = env::temp_dir().join("maestro-bench-midi");
    fs::create_dir_all(&midi_dir).expect("could not create the MIDI scratch directory");

    let out_root = env::var_os("MAESTRO_BENCH_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("maestro-bench-out"));
    fs::create_dir_all(&out_root).expect("could not create the output directory");

    let rig = Rig {
        synth,
        soundfonts: soundfonts(),
        audio: AudioParameters::default(),
        out_root,
        run: std::cell::Cell::new(0),
    };

    let base = RendererConfig {
        synth: rig.synth,
        port_threads: None,
    };

    let generated: Vec<(&Workload, PathBuf, u64)> = WORKLOADS
        .iter()
        .map(|workload| {
            let path = midi_dir.join(format!("{}-{seconds}s.mid", workload.name));
            let events = generate(workload, seconds, &path);
            eprintln!(
                "workload {:<12} {events:>9} events over {seconds}s ({} port(s))",
                workload.name, workload.ports
            );
            (workload, path, events)
        })
        .collect();

    let mut group = c.benchmark_group("file_render");

    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(measurement_secs()));

    let empty = midi_dir.join(format!("startup-{seconds}s.mid"));
    generate_empty(seconds, &empty);
    group.throughput(Throughput::Elements(1));
    group.bench_function("startup", |b| {
        b.iter_custom(|iters| {
            (0..iters)
                .map(|_| rig.render_once(&empty, base, None, PortMode::Multi { max_ports: None }))
                .sum()
        })
    });

    for (workload, path, events) in &generated {
        let ports = if workload.ports > 1 {
            PortMode::Multi { max_ports: None }
        } else {
            PortMode::Single
        };

        group.throughput(Throughput::Elements(*events));
        group.bench_function(workload.name, |b| {
            b.iter_custom(|iters| {
                (0..iters)
                    .map(|_| rig.render_once(path, base, None, ports))
                    .sum()
            })
        });
    }

    if let Some((_, path, events)) = generated.iter().find(|(w, ..)| w.name == "multi_port") {
        let threaded = RendererConfig {
            port_threads: Some(4),
            ..base
        };

        group.throughput(Throughput::Elements(*events));
        group.bench_function("multi_port_threaded", |b| {
            b.iter_custom(|iters| {
                (0..iters)
                    .map(|_| {
                        rig.render_once(path, threaded, None, PortMode::Multi { max_ports: None })
                    })
                    .sum()
            })
        });
    }

    if let Some((_, path, events)) = generated.iter().find(|(w, ..)| w.name == "dense") {
        group.throughput(Throughput::Elements(*events));
        group.bench_function("dense_event_processor", |b| {
            b.iter_custom(|iters| {
                (0..iters)
                    .map(|_| {
                        rig.render_once(
                            path,
                            base,
                            Some(EventProcessorConfig::default()),
                            PortMode::Single,
                        )
                    })
                    .sum()
            })
        });
    }

    if let Some(path) = env::var_os("MAESTRO_BENCH_MIDI").map(PathBuf::from) {
        if path.is_file() {
            group.throughput(Throughput::Elements(1));
            group.bench_function("user_midi", |b| {
                b.iter_custom(|iters| {
                    (0..iters)
                        .map(|_| {
                            rig.render_once(&path, base, None, PortMode::Multi { max_ports: None })
                        })
                        .sum()
                })
            });
        } else {
            eprintln!("skipped user_midi: {} is not a file", path.display());
        }
    }

    group.finish();
}

criterion_group!(benches, file_render);
criterion_main!(benches);
