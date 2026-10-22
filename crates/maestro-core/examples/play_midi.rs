use std::{
    path::PathBuf,
    process::exit,
    thread,
    time::{Duration, Instant},
};

use maestro_core::{
    realtime::{
        MaestroRealtimeEngine, RealtimeEngineOptions,
        config::{NpsLimit, RealtimeConfig},
        host_time,
    },
    renderer::config::{
        RendererConfig, SynthConfig,
        bassmidi::{BASSMIDIConfig, BASSMIDIThreading},
    },
    soundfont::SoundFontList,
};
use midi_parser::{EventRef, MidiFile, TempoClock};

const LOOKAHEAD: Duration = Duration::from_secs(1);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (midipath, sfpath) = match (args.get(1), args.get(2)) {
        (Some(midi), Some(sf)) => (PathBuf::from(midi), PathBuf::from(sf)),
        _ => {
            eprintln!(
                "Usage: {} <midi-file> <soundfont (.sf2/.sf3/.sfz)>\n\n\
                 Plays <midi-file> through the default audio output device using\n\
                 the default synthesizer configuration.",
                args.first().map(String::as_str).unwrap_or("play_midi")
            );
            exit(2);
        }
    };

    for (label, path) in [("MIDI file", &midipath), ("SoundFont", &sfpath)] {
        if !path.exists() {
            eprintln!("{label} not found: {}", path.display());
            exit(2);
        }
    }

    let options = RealtimeEngineOptions {
        ports: None,
        renderer: RendererConfig {
            synth: SynthConfig::BASSMIDI(BASSMIDIConfig {
                voice_limit: 3000,
                multithreading: Some(BASSMIDIThreading {
                    thread_count: 0,
                    divide_channels: false,
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        config: RealtimeConfig {
            max_nps: Some(NpsLimit {
                max: 200_000,
                load_limiter: false,
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut realtime = MaestroRealtimeEngine::new(options).unwrap();

    let stats = realtime.get_statistics();
    thread::spawn(move || {
        loop {
            println!(
                "Voice Count: {:3}\tRender time: {:.2}\tPeak: {:.0}",
                stats.get_renderer().read_voice_count(),
                stats.get_renderer().get_average_render_time(),
                stats.read_render_peak()
            );
            thread::sleep(Duration::from_millis(100));
        }
    });

    let sflist = SoundFontList::from(sfpath);
    realtime.set_soundfonts(sflist).unwrap();

    let midi = MidiFile::open(&midipath).unwrap();
    let mut tempo = TempoClock::new(midi.division(), 1_000_000_000);
    let mut merged = midi.merged().unwrap();

    let start = Instant::now() + Duration::from_millis(500);
    let start_ticks = host_time::host_now() + host_time::ns_to_ticks(500_000_000);
    let mut nanos = 0;
    let mut batch = Vec::new();
    let mut sysex = Vec::new();

    while let Some(Ok((delta, _, group))) = merged.next_batch(&mut batch) {
        nanos += tempo.advance(delta);
        let due = start + Duration::from_nanos(nanos);
        let at = start_ticks + host_time::ns_to_ticks(nanos);
        if let Some(wait) = due.checked_duration_since(Instant::now() + LOOKAHEAD) {
            thread::sleep(wait);
        }

        let mut set_tempo = None;
        for event in &batch {
            match *event {
                EventRef::Midi(m) => realtime.sender().process_short(group, m.as_short(), at),
                EventRef::SysEx(data) => {
                    sysex.clear();
                    sysex.push(0xF0);
                    sysex.extend_from_slice(data);
                    sysex.push(0xF7);
                    realtime.sender().process_long(group, &sysex, at);
                }
                EventRef::Meta(m) => set_tempo = m.tempo().or(set_tempo),
                EventRef::Ump(packet) => set_tempo = packet.tempo().or(set_tempo),
                EventRef::Escape(_) => {}
            }
        }

        if let Some(micros) = set_tempo {
            tempo.set_tempo(micros);
        }
    }

    thread::sleep(Duration::from_secs(5));
}
