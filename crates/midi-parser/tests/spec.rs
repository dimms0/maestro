//! Tests built on the two example files printed in RP-001 (pages 12-14): the
//! same short excerpt written once as a format 0 file and once as a format 1
//! file. Between them they exercise headers, variable-length quantities,
//! running status, multi-byte delta times, meta events and merging.

use midi_parser::{
    ClipWriter, Division, EventRef, FileKind, Meta, MidiFile, MidiMessage, Options, SmfWriter, Ump,
    meta, ump,
};

/// RP-001 p.12: format 0, one track, 96 ticks per quarter note.
fn format_0() -> Vec<u8> {
    let track: &[u8] = &[
        0x00, 0xFF, 0x58, 0x04, 0x04, 0x02, 0x18, 0x08, // time signature
        0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, // tempo, 500000 us
        0x00, 0xC0, 0x05, //
        0x00, 0xC1, 0x2E, //
        0x00, 0xC2, 0x46, //
        0x00, 0x92, 0x30, 0x60, //
        0x00, 0x3C, 0x60, // running status
        0x60, 0x91, 0x43, 0x40, //
        0x60, 0x90, 0x4C, 0x20, //
        0x81, 0x40, 0x82, 0x30, 0x40, // two-byte delta time
        0x00, 0x3C, 0x40, // running status
        0x00, 0x81, 0x43, 0x40, //
        0x00, 0x80, 0x4C, 0x40, //
        0x00, 0xFF, 0x2F, 0x00, // end of track
    ];

    let mut file = header(0, 1, 96);
    chunk(&mut file, track);
    file
}

/// RP-001 p.13-14: the same music as format 1, in four tracks.
fn format_1() -> Vec<u8> {
    let tempo_track: &[u8] = &[
        0x00, 0xFF, 0x58, 0x04, 0x04, 0x02, 0x18, 0x08, //
        0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, //
        0x83, 0x00, 0xFF, 0x2F, 0x00,
    ];
    let first: &[u8] = &[
        0x00, 0xC0, 0x05, //
        0x81, 0x40, 0x90, 0x4C, 0x20, //
        0x81, 0x40, 0x4C, 0x00, // running status, note on with velocity 0
        0x00, 0xFF, 0x2F, 0x00,
    ];
    let second: &[u8] = &[
        0x00, 0xC1, 0x2E, //
        0x60, 0x91, 0x43, 0x40, //
        0x82, 0x20, 0x43, 0x00, //
        0x00, 0xFF, 0x2F, 0x00,
    ];
    let third: &[u8] = &[
        0x00, 0xC2, 0x46, //
        0x00, 0x92, 0x30, 0x60, //
        0x00, 0x3C, 0x60, //
        0x83, 0x00, 0x30, 0x00, //
        0x00, 0x3C, 0x00, //
        0x00, 0xFF, 0x2F, 0x00,
    ];

    let mut file = header(1, 4, 96);
    for track in [tempo_track, first, second, third] {
        chunk(&mut file, track);
    }
    file
}

fn header(format: u16, tracks: u16, division: u16) -> Vec<u8> {
    let mut out = b"MThd".to_vec();
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&format.to_be_bytes());
    out.extend_from_slice(&tracks.to_be_bytes());
    out.extend_from_slice(&division.to_be_bytes());
    out
}

fn chunk(out: &mut Vec<u8>, track: &[u8]) {
    out.extend_from_slice(b"MTrk");
    out.extend_from_slice(&(track.len() as u32).to_be_bytes());
    out.extend_from_slice(track);
}

/// Every note in a file as (tick, channel, key, sounding), with the note on at
/// velocity zero that means a note off folded into the note off it stands for.
fn notes(bytes: &[u8]) -> Vec<(u64, u8, u8, bool)> {
    let file = MidiFile::from_slice(bytes).unwrap();
    let mut out: Vec<_> = file
        .merged()
        .unwrap()
        .map(|event| event.unwrap())
        .filter_map(|event| match event.event {
            EventRef::Midi(m) if m.kind() == 0x90 => {
                Some((event.tick, m.channel(), m.data1, m.data2 > 0))
            }
            EventRef::Midi(m) if m.kind() == 0x80 => Some((event.tick, m.channel(), m.data1, false)),
            _ => None,
        })
        .collect();
    out.sort_unstable();
    out
}

#[test]
fn the_spec_examples_describe_the_same_music() {
    // The two files differ in layout, in how they spell a note off and in
    // where their delta times fall, so agreeing here means headers, running
    // status, multi-byte deltas and the merge all came out right.
    assert_eq!(notes(&format_0()), notes(&format_1()));
}

#[test]
fn headers_are_read() {
    let bytes = format_0();
    let zero = MidiFile::from_slice(&bytes).unwrap();
    assert_eq!(zero.kind(), FileKind::Smf);
    assert_eq!(zero.format(), 0);
    assert_eq!(zero.division(), Division::Ppq(96));
    assert_eq!(zero.track_count(), 1);

    let bytes = format_1();
    let one = MidiFile::from_slice(&bytes).unwrap();
    assert_eq!(one.format(), 1);
    assert_eq!(one.track_count(), 4);
}

#[test]
fn the_format_0_track_decodes_event_for_event() {
    let bytes = format_0();
    let file = MidiFile::from_slice(&bytes).unwrap();
    let events: Vec<_> = file
        .merged()
        .unwrap()
        .map(|event| event.unwrap())
        .map(|event| (event.delta, event.event))
        .collect();

    let midi = |status, data1, data2| {
        EventRef::Midi(MidiMessage {
            status,
            data1,
            data2,
        })
    };

    assert_eq!(events.len(), 14);
    assert_eq!(
        events[0],
        (
            0,
            EventRef::Meta(Meta {
                kind: meta::TIME_SIGNATURE,
                data: &[0x04, 0x02, 0x18, 0x08],
            })
        )
    );
    assert_eq!(events[1].1.tempo(), Some(500_000));
    assert_eq!(events[2], (0, midi(0xC0, 0x05, 0)));
    // Running status: the second note on carries no status byte of its own.
    assert_eq!(events[6], (0, midi(0x92, 0x3C, 0x60)));
    assert_eq!(events[7], (96, midi(0x91, 0x43, 0x40)));
    // 0x81 0x40 is 192, the two-byte delta time the spec calls out.
    assert_eq!(events[9], (192, midi(0x82, 0x30, 0x40)));
    assert!(events[13].1.is_end_of_track());
}

#[test]
fn simultaneous_events_come_out_in_track_order() {
    let bytes = format_1();
    let file = MidiFile::from_slice(&bytes).unwrap();
    let tracks: Vec<u32> = file
        .merged()
        .unwrap()
        .map(|event| event.unwrap())
        .filter(|event| event.tick == 0)
        .map(|event| event.track)
        .collect();

    assert!(
        tracks.windows(2).all(|pair| pair[0] <= pair[1]),
        "tracks {tracks:?} are out of order at tick 0"
    );
}

#[test]
fn a_scan_measures_the_file() {
    let bytes = format_1();
    let info = MidiFile::from_slice(&bytes).unwrap().scan().unwrap();

    assert_eq!(info.total_ticks, 384);
    assert_eq!(info.notes, 4);
    assert_eq!(info.tempo_changes, 1);
    assert_eq!(info.channels, 0b0111);
    // Four quarter notes at 96 ppq and 500000 microseconds each.
    assert!((info.duration - 2.0).abs() < 1e-9, "{}", info.duration);
}

#[test]
fn a_scan_falls_back_to_the_tempo_the_spec_names() {
    // No Set Tempo event anywhere, so RP-001's 120 bpm applies: 384 ticks at
    // 96 ppq is four quarter notes, which is two seconds.
    let mut file = header(0, 1, 96);
    chunk(&mut file, &[0x00, 0x90, 0x40, 0x40, 0x83, 0x00, 0xFF, 0x2F, 0x00]);

    let info = MidiFile::from_slice(&file).unwrap().scan().unwrap();
    assert_eq!(info.tempo_changes, 0);
    assert!((info.duration - 2.0).abs() < 1e-9, "{}", info.duration);
}

#[test]
fn variable_length_quantities_round_trip() {
    // The boundary values RP-001 tabulates.
    for (value, encoded) in [
        (0x0000_0000u32, &[0x00u8][..]),
        (0x0000_007F, &[0x7F]),
        (0x0000_0080, &[0x81, 0x00]),
        (0x0000_3FFF, &[0xFF, 0x7F]),
        (0x0000_4000, &[0x81, 0x80, 0x00]),
        (0x001F_FFFF, &[0xFF, 0xFF, 0x7F]),
        (0x0020_0000, &[0x81, 0x80, 0x80, 0x00]),
        (0x0FFF_FFFF, &[0xFF, 0xFF, 0xFF, 0x7F]),
    ] {
        // A delta time is the only place a quantity appears on its own, so
        // that is what it gets written and read back as.
        let mut file = header(0, 1, 96);
        let mut track = encoded.to_vec();
        track.extend_from_slice(&[0x90, 0x40, 0x40, 0x00, 0xFF, 0x2F, 0x00]);
        chunk(&mut file, &track);

        let parsed = MidiFile::from_slice(&file).unwrap();
        let first = parsed.merged().unwrap().next().unwrap().unwrap();
        assert_eq!(first.delta, u64::from(value), "{encoded:02X?}");
    }
}

#[test]
fn a_truncated_track_stops_where_the_data_does() {
    let mut bytes = format_0();
    bytes.truncate(bytes.len() - 12);

    let file = MidiFile::from_slice(&bytes).unwrap();
    let events: Vec<_> = file.merged().unwrap().collect();
    assert!(
        events.iter().all(|event| event.is_ok()),
        "a short file should still read"
    );
    assert!(!events.is_empty());
}

#[test]
fn strict_mode_refuses_what_lenient_mode_recovers_from() {
    let mut bytes = format_0();
    bytes.truncate(bytes.len() - 12);

    // The cut lands inside a track, so the chunk length no longer matches what
    // is there and strict mode says so rather than reading on.
    let options = Options { strict: true };
    let failed = match MidiFile::from_slice_with(&bytes, options) {
        Err(_) => true,
        Ok(file) => file.merged().unwrap().any(|event| event.is_err()),
    };

    assert!(failed, "strict mode let a truncated track through");
}

#[test]
fn format_2_is_rejected() {
    let mut bytes = header(2, 1, 96);
    chunk(&mut bytes, &[0x00, 0xFF, 0x2F, 0x00]);
    assert!(MidiFile::from_slice(&bytes).is_err());
}

#[test]
fn a_file_that_is_not_midi_is_rejected() {
    assert!(MidiFile::from_slice(b"RIFF____WAVEfmt ").is_err());
}

#[test]
fn what_the_writer_produces_reads_back_the_same() {
    let source = format_1();
    let file = MidiFile::from_slice(&source).unwrap();

    let mut written = Vec::new();
    {
        let mut writer =
            SmfWriter::new(&mut written, 0, file.division(), 1).unwrap();
        writer.begin_track();

        // Four tracks collapse into one, so only the writer's own End of Track
        // belongs in the result. Skipping the originals means carrying their
        // delta onto the next event that is kept.
        let mut pending = 0u64;
        for event in file.merged().unwrap() {
            let event = event.unwrap();
            pending += event.delta;
            if event.event.is_end_of_track() {
                continue;
            }
            writer.event(pending as u32, event.event).unwrap();
            pending = 0;
        }

        writer.finish().unwrap();
    }

    assert_eq!(notes(&written), notes(&source));
}

#[test]
fn a_clip_file_round_trips() {
    let mut written = Vec::new();
    {
        let mut writer = ClipWriter::new(&mut written, 480).unwrap();
        writer.start_clip(0).unwrap();
        writer.packet(Ump::set_tempo(0, 500_000)).unwrap();
        writer
            .event(
                0,
                3,
                EventRef::Midi(MidiMessage {
                    status: 0x91,
                    data1: 0x40,
                    data2: 0x64,
                }),
            )
            .unwrap();
        // Past the 0xFFFFF ticks a single Delta Clockstamp can carry, so this
        // has to come out as a chain of them.
        writer
            .event(
                0x20_0000,
                3,
                EventRef::Midi(MidiMessage {
                    status: 0x81,
                    data1: 0x40,
                    data2: 0x40,
                }),
            )
            .unwrap();
        writer.end_clip(0).unwrap();
        writer.finish().unwrap();
    }

    let file = MidiFile::from_slice(&written).unwrap();
    assert_eq!(file.kind(), FileKind::Clip);
    assert_eq!(file.division(), Division::Ppq(480));

    let events: Vec<_> = file
        .merged()
        .unwrap()
        .map(|event| event.unwrap())
        .filter(|event| matches!(event.event, EventRef::Midi(_)))
        .collect();

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].group, 3);
    assert_eq!(events[1].tick, 0x20_0000);
}

#[test]
fn a_clip_file_carries_its_tempo() {
    let mut written = Vec::new();
    let mut writer = ClipWriter::new(&mut written, 96).unwrap();
    writer.start_clip(0).unwrap();
    writer.packet(Ump::set_tempo(0, 400_000)).unwrap();
    writer.end_clip(384).unwrap();
    writer.finish().unwrap();

    let info = MidiFile::from_slice(&written).unwrap().scan().unwrap();
    assert_eq!(info.tempo_changes, 1);
    assert!((info.duration - 1.6).abs() < 1e-9, "{}", info.duration);
}

#[test]
fn an_smf_converted_to_a_clip_and_back_keeps_its_music() {
    let source = format_1();
    let file = MidiFile::from_slice(&source).unwrap();

    let mut clip = Vec::new();
    midi_parser::convert::to_clip(&file, &mut clip).unwrap();

    let clip_file = MidiFile::from_slice(&clip).unwrap();
    assert_eq!(clip_file.kind(), FileKind::Clip);
    assert_eq!(clip_file.scan().unwrap().tempo_changes, 1);

    let mut round_tripped = Vec::new();
    midi_parser::convert::to_smf(&clip_file, &mut round_tripped).unwrap();

    let back = MidiFile::from_slice(&round_tripped).unwrap();
    assert_eq!(back.kind(), FileKind::Smf);
    assert_eq!(notes(&round_tripped), notes(&source));

    let info = back.scan().unwrap();
    assert_eq!(info.tempo_changes, 1);
    assert!((info.duration - 2.0).abs() < 1e-9, "{}", info.duration);
}

#[test]
fn sysex_survives_the_trip_through_a_clip_file() {
    // Long enough to need a start, a continue and an end packet.
    let payload: Vec<u8> = (1u8..=20).collect();

    let mut packets = Vec::new();
    midi_parser::convert::event_to_ump(0, EventRef::SysEx(&payload), &mut packets);
    assert_eq!(packets.len(), 4);
    assert_eq!(packets[0].message_type(), ump::mt::SYSEX7);

    let mut written = Vec::new();
    let mut writer = ClipWriter::new(&mut written, 96).unwrap();
    writer.start_clip(0).unwrap();
    writer.event(0, 0, EventRef::SysEx(&payload)).unwrap();
    writer.end_clip(0).unwrap();
    writer.finish().unwrap();

    let clip = MidiFile::from_slice(&written).unwrap();
    let mut smf = Vec::new();
    midi_parser::convert::to_smf(&clip, &mut smf).unwrap();

    let file = MidiFile::from_slice(&smf).unwrap();
    let recovered: Vec<Vec<u8>> = file
        .merged()
        .unwrap()
        .map(|event| event.unwrap())
        .filter_map(|event| match event.event {
            EventRef::SysEx(data) => Some(data.to_vec()),
            _ => None,
        })
        .collect();

    assert_eq!(recovered, vec![payload]);
}

#[test]
fn many_tracks_still_merge_in_order() {
    // Enough tracks to be sure the merge holds up past the handful a hand
    // written file has, with every track landing a note on every tick.
    const TRACKS: u16 = 64;
    const NOTES: u32 = 50;

    let mut bytes = header(1, TRACKS, 96);
    for track in 0..TRACKS {
        let mut events = Vec::new();
        for note in 0..NOTES {
            events.extend_from_slice(&[if note == 0 { 0x00 } else { 0x01 }, 0x90]);
            events.extend_from_slice(&[(note % 128) as u8, 0x40]);
        }
        events.extend_from_slice(&[0x00, 0xFF, meta::MIDI_PORT, 0x01, (track % 4) as u8]);
        events.extend_from_slice(&[0x00, 0xFF, meta::END_OF_TRACK, 0x00]);
        chunk(&mut bytes, &events);
    }

    let file = MidiFile::from_slice(&bytes).unwrap();
    let mut last = 0;
    let mut count = 0;
    for event in file.merged().unwrap() {
        let event = event.unwrap();
        assert!(event.tick >= last, "tick {} went backwards", event.tick);
        last = event.tick;
        count += 1;
    }

    assert_eq!(count, TRACKS as u64 * (NOTES as u64 + 2));

    let info = file.scan().unwrap();
    assert_eq!(info.notes, TRACKS as u64 * NOTES as u64);
    assert_eq!(info.track_ports.len(), TRACKS as usize);
    assert_eq!(info.ports, 0b1111);
}

#[test]
fn a_mapped_file_reads_the_same_as_the_bytes_do() {
    let bytes = format_1();
    let path = std::env::temp_dir().join("midi-parser-mapped.mid");
    std::fs::write(&path, &bytes).unwrap();

    let mapped = MidiFile::open(&path).unwrap();
    let loaded = MidiFile::load(&path).unwrap();

    let events = |file: &MidiFile<'_>| -> Vec<(u64, u32)> {
        file.merged()
            .unwrap()
            .map(|event| event.unwrap())
            .map(|event| (event.tick, event.track))
            .collect()
    };

    assert_eq!(events(&mapped), events(&loaded));
    assert_eq!(mapped.scan().unwrap().notes, 4);

    std::fs::remove_file(&path).ok();
}

#[test]
fn midi_2_notes_scale_down_to_midi_1() {
    let mut written = Vec::new();
    let mut writer = ClipWriter::new(&mut written, 96).unwrap();
    writer.start_clip(0).unwrap();

    // MIDI 2.0 Note On, channel 1, key 60, full 16-bit velocity.
    let note_on = Ump::new(&[0x4091_3C00, 0xFFFF_0000]).unwrap();
    writer.packet(Ump::delta_clockstamp(0)).unwrap();
    writer.packet(note_on).unwrap();
    writer.end_clip(96).unwrap();
    writer.finish().unwrap();

    let clip = MidiFile::from_slice(&written).unwrap();
    let mut smf = Vec::new();
    midi_parser::convert::to_smf(&clip, &mut smf).unwrap();

    let file = MidiFile::from_slice(&smf).unwrap();
    let notes: Vec<MidiMessage> = file
        .merged()
        .unwrap()
        .map(|event| event.unwrap())
        .filter_map(|event| match event.event {
            EventRef::Midi(m) if m.kind() == 0x90 => Some(m),
            _ => None,
        })
        .collect();

    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].data1, 60);
    assert_eq!(notes[0].data2, 127);
}
