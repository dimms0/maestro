//! Conversion between the two file formats, following the concordance in
//! M2-116-U Appendix A: what SMF version 1 expressed as meta events, a clip
//! file expresses as Flex Data messages.

use std::io::Write;

use crate::{
    error::Result,
    event::{EventRef, Meta, MidiMessage, meta},
    file::{Division, MidiFile},
    ump::{Ump, flex, mt},
    writer::{ClipWriter, SmfWriter},
};

/// Turns one event into the packets a clip file stores it as. Events with no
/// equivalent produce nothing.
pub fn event_to_ump(group: u8, event: EventRef<'_>, out: &mut Vec<Ump>) {
    out.clear();

    match event {
        EventRef::Midi(message) => out.push(Ump::midi1_message(group, message)),
        EventRef::SysEx(payload) => sysex7(group, payload, out),
        EventRef::Meta(event) => meta_to_flex(group, event, out),
        EventRef::Ump(packet) => out.push(packet),
        // An escape is raw bytes for a MIDI 1.0 wire, which UMP has no room for.
        EventRef::Escape(_) => {}
    }
}

/// Splits a SysEx payload across SysEx7 packets, six 7-bit bytes at a time.
fn sysex7(group: u8, payload: &[u8], out: &mut Vec<Ump>) {
    if payload.len() <= 6 {
        out.push(Ump::sysex7_packet(group, 0x0, payload));
        return;
    }

    let last = (payload.len() - 1) / 6;
    for (index, chunk) in payload.chunks(6).enumerate() {
        let status = match index {
            0 => 0x1,
            index if index == last => 0x3,
            _ => 0x2,
        };
        out.push(Ump::sysex7_packet(group, status, chunk));
    }
}

fn meta_to_flex(group: u8, event: Meta<'_>, out: &mut Vec<Ump>) {
    match (event.kind, event.data) {
        (meta::SET_TEMPO, _) => {
            if let Some(micros) = event.tempo() {
                out.push(Ump::set_tempo(group, micros));
            }
        }

        // SMF stores the denominator as a power of two; Flex Data stores it
        // outright.
        (meta::TIME_SIGNATURE, [numerator, denominator, _clocks, thirty_seconds]) => {
            let denominator = 1u32 << (*denominator).min(31);
            out.push(Ump::flex_data(
                group,
                0,
                flex::BANK_SETUP,
                flex::SET_TIME_SIGNATURE,
                [
                    u32::from(*numerator) << 24
                        | (denominator & 0xFF) << 16
                        | u32::from(*thirty_seconds) << 8,
                    0,
                    0,
                ],
            ));
        }

        (meta::KEY_SIGNATURE, [sharps_flats, _mode]) => {
            out.push(Ump::flex_data(
                group,
                0,
                flex::BANK_SETUP,
                flex::SET_KEY_SIGNATURE,
                [u32::from(*sharps_flats & 0xF) << 28, 0, 0],
            ));
        }

        (meta::COPYRIGHT, data) => text(group, flex::BANK_METADATA, flex::COPYRIGHT, data, out),
        (meta::TRACK_NAME, data) => text(group, flex::BANK_METADATA, flex::CLIP_NAME, data, out),
        (meta::LYRIC, data) => text(group, flex::BANK_PERFORMANCE, flex::LYRICS, data, out),

        _ => {}
    }
}

/// Flex Data text, twelve bytes per packet, with the form field marking where
/// each packet falls in the string.
fn text(group: u8, bank: u8, status: u8, data: &[u8], out: &mut Vec<Ump>) {
    if data.is_empty() {
        return;
    }

    let last = (data.len() - 1) / 12;
    for (index, chunk) in data.chunks(12).enumerate() {
        let form = match (last, index) {
            (0, _) => 0,
            (_, 0) => 1,
            (last, index) if index == last => 3,
            _ => 2,
        };

        let mut words = [0u32; 3];
        for (at, byte) in chunk.iter().enumerate() {
            words[at / 4] |= u32::from(*byte) << (24 - 8 * (at % 4));
        }

        out.push(Ump::flex_data(group, form, bank, status, words));
    }
}

/// Writes every event of `file` out as a MIDI Clip File.
///
/// Tracks collapse into the single sequence a clip file is, with each track's
/// MIDI Port meta event picking the UMP Group its events are addressed to.
pub fn to_clip<W: Write>(file: &MidiFile<'_>, out: W) -> Result<()> {
    let ppq = match file.division() {
        Division::Ppq(ppq) => ppq,
        // A clip file counts in ticks per quarter note and nothing else, so
        // SMPTE timing is folded onto its nominal tick rate.
        Division::Smpte { fps, subframes } => u16::from(fps) * u16::from(subframes),
    };

    let mut writer = ClipWriter::new(out, ppq)?;
    let mut ports = vec![0u8; file.track_count()];
    let mut started = false;
    let mut pending = 0u64;

    for event in file.merged()? {
        let event = event?;
        pending += event.delta;

        if let EventRef::Meta(m) = event.event {
            if let Some(port) = m.port() {
                ports[event.track as usize] = port & 0xF;
                continue;
            }
            if m.kind == meta::END_OF_TRACK {
                continue;
            }
        }

        if !started {
            writer.start_clip(pending)?;
            pending = 0;
            started = true;
        }

        let group = match file.kind() {
            crate::file::FileKind::Clip => event.group,
            crate::file::FileKind::Smf => ports[event.track as usize],
        };

        writer.event(pending, group, event.event)?;
        pending = 0;
    }

    if !started {
        writer.start_clip(pending)?;
        pending = 0;
    }

    writer.end_clip(pending)?;
    writer.finish()?;
    Ok(())
}

/// Writes every event of `file` out as a Standard MIDI File, one track per UMP
/// Group in use.
///
/// MIDI 2.0 Channel Voice messages are scaled down to MIDI 1.0 resolution;
/// messages with no MIDI 1.0 equivalent are dropped.
pub fn to_smf<W: Write>(file: &MidiFile<'_>, out: W) -> Result<()> {
    let groups = groups_used(file)?;
    let mut writer = SmfWriter::new(out, 1, file.division(), groups.len() as u16)?;

    for group in groups {
        writer.begin_track();
        writer.event(
            0,
            EventRef::Meta(Meta {
                kind: meta::MIDI_PORT,
                data: std::slice::from_ref(&group),
            }),
        )?;

        let mut sysex = Vec::new();
        let mut text = Vec::new();
        let mut pending = 0u64;

        for event in file.merged()? {
            let event = event?;
            pending += event.delta;
            if event.group != group {
                continue;
            }

            let delta = (pending.min(0x0FFF_FFFF)) as u32;
            let mut written = true;

            match event.event {
                EventRef::Midi(message) => writer.event(delta, EventRef::Midi(message))?,
                EventRef::SysEx(payload) => writer.event(delta, EventRef::SysEx(payload))?,
                EventRef::Escape(payload) => writer.event(delta, EventRef::Escape(payload))?,
                EventRef::Meta(m) => writer.event(delta, EventRef::Meta(m))?,

                EventRef::Ump(packet) => match packet.message_type() {
                    mt::MIDI2_CHANNEL_VOICE => match midi2_to_midi1(&packet) {
                        Some(message) => writer.event(delta, EventRef::Midi(message))?,
                        None => written = false,
                    },

                    mt::SYSEX7 => match gather_sysex7(&packet, &mut sysex) {
                        Some(payload) => writer.event(delta, EventRef::SysEx(payload))?,
                        None => written = false,
                    },

                    mt::FLEX_DATA => match flex_to_meta(&packet, &mut text) {
                        Some((kind, data)) => {
                            writer.event(delta, EventRef::Meta(Meta { kind, data }))?
                        }
                        None => written = false,
                    },

                    _ => written = false,
                },
            }

            if written {
                pending = 0;
            }
        }

        writer.end_track()?;
    }

    writer.finish()?;
    Ok(())
}

/// The UMP Groups a clip file addresses, or a single group 0 for an SMF.
fn groups_used(file: &MidiFile<'_>) -> Result<Vec<u8>> {
    let mut seen = 0u16;
    for event in file.merged()? {
        seen |= 1 << event?.group;
    }

    let groups: Vec<u8> = (0..16).filter(|group| seen & (1 << group) != 0).collect();
    Ok(if groups.is_empty() { vec![0] } else { groups })
}

/// Scales a MIDI 2.0 Channel Voice message down to MIDI 1.0, M2-104-UM §4.
fn midi2_to_midi1(packet: &Ump) -> Option<MidiMessage> {
    let (status, index, _, data) = packet.midi2()?;

    let (data1, data2) = match status & 0xF0 {
        // Velocity is 16 bits in the top half of the data word. A MIDI 2.0
        // Note On with a velocity that scales to zero still has to sound, so it
        // floors at 1 rather than turning into a Note Off.
        0x80 => (index & 0x7F, ((data >> 25) as u8).min(0x7F)),
        0x90 => (index & 0x7F, ((data >> 25) as u8).max(1)),
        0xA0 | 0xB0 => (index & 0x7F, (data >> 25) as u8),
        0xC0 => ((data >> 24) as u8 & 0x7F, 0),
        0xD0 => ((data >> 25) as u8, 0),
        0xE0 => {
            let bend = data >> 18;
            ((bend & 0x7F) as u8, ((bend >> 7) & 0x7F) as u8)
        }
        _ => return None,
    };

    Some(MidiMessage {
        status,
        data1,
        data2,
    })
}

/// Collects SysEx7 packets, returning the payload once the last one arrives.
fn gather_sysex7<'a>(packet: &Ump, buffer: &'a mut Vec<u8>) -> Option<&'a [u8]> {
    let (status, bytes, count) = packet.sysex7()?;

    if status == 0x0 || status == 0x1 {
        buffer.clear();
    }
    buffer.extend_from_slice(&bytes[..count]);

    (status == 0x0 || status == 0x3).then_some(buffer.as_slice())
}

/// The meta event a Flex Data message stands in for, if there is one.
fn flex_to_meta<'a>(packet: &Ump, buffer: &'a mut Vec<u8>) -> Option<(u8, &'a [u8])> {
    let (form, bank, status) = packet.flex()?;
    let payload = packet.payload();

    if bank == flex::BANK_SETUP {
        buffer.clear();
        match status {
            flex::SET_TEMPO => {
                let micros = payload[0] / 100;
                buffer.extend_from_slice(&micros.to_be_bytes()[1..]);
                return Some((meta::SET_TEMPO, buffer.as_slice()));
            }

            flex::SET_TIME_SIGNATURE => {
                let denominator = (payload[0] >> 16) as u8;
                buffer.extend_from_slice(&[
                    (payload[0] >> 24) as u8,
                    denominator.max(1).ilog2() as u8,
                    24,
                    (payload[0] >> 8) as u8,
                ]);
                return Some((meta::TIME_SIGNATURE, buffer.as_slice()));
            }

            flex::SET_KEY_SIGNATURE => {
                // Four-bit two's complement, so 0xF is one flat.
                let sharps_flats = (payload[0] >> 28) as u8;
                let signed = if sharps_flats & 0x8 != 0 {
                    sharps_flats | 0xF0
                } else {
                    sharps_flats
                };
                buffer.extend_from_slice(&[signed, 0]);
                return Some((meta::KEY_SIGNATURE, buffer.as_slice()));
            }

            _ => return None,
        }
    }

    let kind = match (bank, status) {
        (flex::BANK_METADATA, flex::COPYRIGHT) => meta::COPYRIGHT,
        (flex::BANK_METADATA, flex::CLIP_NAME) => meta::TRACK_NAME,
        (flex::BANK_METADATA, flex::PROJECT_NAME | flex::COMPOSITION_NAME) => meta::TEXT,
        (flex::BANK_PERFORMANCE, flex::LYRICS) => meta::LYRIC,
        _ => return None,
    };

    // Form 0 is a whole message; 1 through 3 are the start, middle and end of
    // one, and only the last packet completes it.
    if form == 0 || form == 1 {
        buffer.clear();
    }
    for word in payload {
        buffer.extend_from_slice(&word.to_be_bytes());
    }
    while buffer.last() == Some(&0) {
        buffer.pop();
    }

    (form == 0 || form == 3).then_some((kind, buffer.as_slice()))
}
