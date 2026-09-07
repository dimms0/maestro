use std::{
    path::PathBuf,
    process::exit,
    thread,
    time::{Duration, Instant},
};

use maestro_core::{
    realtime::{MaestroRealtimeEngine, RealtimeEngineOptions, config::RealtimeConfig},
    renderer::config::{
        RendererConfig,
        bassmidi::{BASSMIDIConfig, BASSMIDIThreading},
    },
    soundfont::SoundFontList,
};
use midi_parser::{EventRef, MidiFile};

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
        config: RealtimeConfig {
            render_buffer_ms: 10.0,
            device_audio_params: false,
            max_nps: None, // TODO: fix NPS limiting for this type of playback
            ..Default::default()
        },
        renderer: RendererConfig {
            synth: maestro_core::renderer::config::SynthConfig::BASSMIDI(BASSMIDIConfig {
                voice_limit: 1024,
                multithreading: Some(BASSMIDIThreading {
                    thread_count: None,
                    keyboard_divisions: 4,
                }),
                ..Default::default()
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
                "Voice Count: {:3}\tRender time: {:.2}",
                stats.read_voice_count(),
                stats.get_average_render_time()
            );
            thread::sleep(Duration::from_millis(100));
        }
    });

    let sflist = SoundFontList::from(sfpath);
    realtime.set_soundfonts(sflist).unwrap();

    let midi = MidiFile::open(&midipath).unwrap();
    realtime.sender().set_tick_division(midi.division());

    let stop_after = midi.scan().unwrap().duration + 5.0;
    let now = Instant::now();

    let (snd, rcv) = crossbeam_channel::bounded(100);

    thread::spawn(move || {
        let Ok(midi) = MidiFile::open(&midipath) else {
            return;
        };
        let Ok(mut merged) = midi.merged() else {
            return;
        };

        let mut batch = Vec::new();
        let mut pending = 0u64;

        while let Some(Ok((delta, _, group))) = merged.next_batch(&mut batch) {
            pending += delta;

            let mut out = Vec::with_capacity(batch.len());
            let mut set_tempo = None;

            for event in &batch {
                match *event {
                    EventRef::Midi(m) => out.push(Message::Short(m.as_short())),
                    EventRef::SysEx(data) => {
                        let mut bytes = Vec::with_capacity(data.len() + 2);
                        bytes.push(0xF0);
                        bytes.extend_from_slice(data);
                        bytes.push(0xF7);
                        out.push(Message::Long(bytes));
                    }
                    EventRef::Meta(m) => set_tempo = m.tempo().or(set_tempo),
                    EventRef::Ump(packet) => set_tempo = packet.tempo().or(set_tempo),
                    EventRef::Escape(_) => {}
                }
            }

            if snd.send((pending, group, set_tempo, out)).is_err() {
                break;
            }
            pending = 0;
        }
    });

    for (ticks, group, set_tempo, messages) in rcv {
        let mut ticks = ticks;
        for message in messages {
            match message {
                Message::Short(short) => realtime.sender().process_short(group, short, Some(ticks)),
                Message::Long(bytes) => realtime.sender().process_long(group, &bytes, Some(ticks)),
            }
            ticks = 0;
        }

        if ticks != 0 {
            realtime.sender().advance_ticks(ticks);
        }

        if let Some(micros) = set_tempo {
            realtime.sender().set_tick_tempo(micros);
        }
    }

    let elapsed = now.elapsed().as_secs_f64();
    let wait_secs = stop_after - elapsed;
    if wait_secs > 0.0 {
        thread::sleep(Duration::from_secs_f64(wait_secs));
    }
}

enum Message {
    Short(u32),
    Long(Vec<u8>),
}
