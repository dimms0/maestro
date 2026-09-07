use std::sync::{
    Mutex,
    atomic::{AtomicU8, Ordering},
};

use crate::event::{
    MaestroEvent,
    ump::{ParseError, UMP_GROUPS, UMP_MT_WORDS, Ump, midi2_cv_events, parse_ump},
};

const MAX_SYSEX: usize = 64 * 1024;

pub(crate) struct MidiTranslator {
    ports: Box<[PortState]>,
}

// Padded to a cache line so two busy ports don't share one.
#[derive(Default)]
#[repr(align(64))]
struct PortState {
    running_status: AtomicU8,
    stream: Mutex<StreamState>,
}

#[derive(Default)]
struct StreamState {
    data: [u8; 2],
    len: u8,

    sysex: Vec<u8>,
    in_sysex: bool,
}

impl MidiTranslator {
    pub fn new() -> Self {
        Self {
            ports: (0..UMP_GROUPS).map(|_| PortState::default()).collect(),
        }
    }

    pub fn reset(&self) {
        for port in &self.ports {
            port.running_status.store(0, Ordering::Relaxed);

            let mut stream = port.stream.lock().unwrap();
            stream.len = 0;
            stream.in_sysex = false;
            stream.sysex.clear();
        }
    }

    #[inline]
    pub fn short(&self, port: u8, short: u32, mut emit: impl FnMut(MaestroEvent)) {
        let Some(state) = self.ports.get(port as usize) else {
            return;
        };

        let head = short as u8;
        let (status, d1, d2) = if head >= 0x80 {
            if head >= 0xF0 {
                // System Real Time leaves running status alone; System Common
                // clears it. Neither one is a channel message.
                if head < 0xF8 {
                    state.running_status.store(0, Ordering::Relaxed);
                } else if head == 0xFF {
                    emit(MaestroEvent::SystemReset);
                }

                return;
            }

            // Reading first keeps the common case of a repeated status byte off
            // the store path, so other cores' copies of the line stay valid.
            if state.running_status.load(Ordering::Relaxed) != head {
                state.running_status.store(head, Ordering::Relaxed);
            }

            (head, (short >> 8) as u8 & 0x7F, (short >> 16) as u8 & 0x7F)
        } else {
            match state.running_status.load(Ordering::Relaxed) {
                0 => return,
                status => (status, head & 0x7F, (short >> 8) as u8 & 0x7F),
            }
        };

        if let Some(event) = channel_event(status, d1, d2) {
            emit(event);
        }
    }

    pub fn long(&self, port: u8, bytes: &[u8], mut emit: impl FnMut(MaestroEvent)) {
        let Some(state) = self.ports.get(port as usize) else {
            return;
        };

        let mut stream = state.stream.lock().unwrap();
        let mut status = state.running_status.load(Ordering::Relaxed);

        for &byte in bytes {
            if byte >= 0x80 {
                // System Real Time is allowed anywhere, even between the bytes
                // of another message, and never disturbs the one in progress.
                if byte >= 0xF8 {
                    if byte == 0xFF {
                        status = 0;
                        stream.len = 0;
                        stream.in_sysex = false;
                        emit(MaestroEvent::SystemReset);
                    }

                    continue;
                }

                // Any other status byte ends a SysEx: 0xF7 delivers it, and
                // anything else aborts it, dropping what was gathered.
                if stream.in_sysex {
                    let payload = stream.finish_sysex();
                    if byte == 0xF7 {
                        emit(MaestroEvent::SystemExclusive(payload));
                    }
                }

                stream.len = 0;
                status = if byte < 0xF0 {
                    // Channel voice messages arm running status.
                    byte
                } else {
                    if byte == 0xF0 {
                        stream.sysex.clear();
                        stream.in_sysex = true;
                    }

                    // System Common clears running status. The data bytes of
                    // the ones we don't handle are dropped below.
                    0
                };

                continue;
            }

            if stream.in_sysex {
                if stream.sysex.len() < MAX_SYSEX {
                    stream.sysex.push(byte);
                }

                continue;
            }

            if status == 0 {
                // A data byte with no status to attach it to.
                continue;
            }

            let len = stream.len as usize;
            stream.data[len] = byte;
            stream.len += 1;

            if stream.len == expected_data_len(status) {
                stream.len = 0;
                if let Some(event) = channel_event(status, stream.data[0], stream.data[1]) {
                    emit(event);
                }
            }
        }

        state.running_status.store(status, Ordering::Relaxed);
    }

    pub fn ump(&self, mut words: &[u32], mut emit: impl FnMut(u8, MaestroEvent)) {
        while !words.is_empty() {
            let consumed = match parse_ump(words) {
                Ok((packet, consumed)) => {
                    self.translate_packet(packet, &mut emit);
                    consumed
                }
                Err(ParseError::InsufficientWords) => break,
                Err(ParseError::UnknownMessageType(mt)) => {
                    let skip = UMP_MT_WORDS[(mt & 0xF) as usize];
                    if words.len() < skip {
                        break;
                    }

                    skip
                }
            };

            words = &words[consumed..];
        }
    }

    fn translate_packet(&self, packet: Ump, emit: &mut impl FnMut(u8, MaestroEvent)) {
        match packet {
            Ump::Midi1ChannelVoice(m) => {
                if let Some(event) = channel_event(m.status << 4 | m.channel, m.byte3, m.byte4) {
                    emit(m.group, event);
                }
            }
            // No synth supports MIDI 2.0 yet so we just convert to MIDI 1.0
            Ump::Midi2ChannelVoice(m) => midi2_cv_events(&m, |e| emit(m.group, e)),
            Ump::SystemCommonRealTime(m) => {
                if m.status == 0xFF {
                    emit(m.group, MaestroEvent::SystemReset);
                }
            }
            Ump::Data64Bit(m) => {
                let Some(state) = self.ports.get(m.group as usize) else {
                    return;
                };

                // `channel_other` carries the number of valid payload bytes.
                let count = (m.channel_other as usize).min(6);
                let bytes = &m.data[..count];

                let mut stream = state.stream.lock().unwrap();

                // SysEx7 status: 0x0 complete, 0x1 start, 0x2 continue, 0x3 end.
                match m.status {
                    0x0 => emit(m.group, MaestroEvent::SystemExclusive(bytes.into())),
                    0x1 => {
                        stream.sysex.clear();
                        stream.in_sysex = true;
                        stream.push_sysex(bytes);
                    }
                    0x2 if stream.in_sysex => stream.push_sysex(bytes),
                    0x3 if stream.in_sysex => {
                        stream.push_sysex(bytes);
                        let payload = stream.finish_sysex();
                        drop(stream);

                        emit(m.group, MaestroEvent::SystemExclusive(payload));
                    }
                    _ => {}
                }
            }

            Ump::Utility(_) | Ump::Data128Bit(_) | Ump::FlexData(_) | Ump::Stream(_) => {}
        }
    }
}

impl StreamState {
    #[inline]
    fn push_sysex(&mut self, bytes: &[u8]) {
        let room = MAX_SYSEX.saturating_sub(self.sysex.len());
        self.sysex
            .extend_from_slice(&bytes[..bytes.len().min(room)]);
    }

    #[inline]
    fn finish_sysex(&mut self) -> Box<[u8]> {
        self.in_sysex = false;
        self.sysex.as_slice().into()
    }
}

#[inline]
const fn expected_data_len(status: u8) -> u8 {
    match status & 0xF0 {
        0xC0 | 0xD0 => 1,
        _ => 2,
    }
}

#[inline]
pub(super) fn channel_event(status: u8, d1: u8, d2: u8) -> Option<MaestroEvent> {
    let channel = status & 0x0F;
    let d1 = d1 & 0x7F;
    let d2 = d2 & 0x7F;

    match status >> 4 {
        0x8 => Some(MaestroEvent::NoteOff { channel, key: d1 }),
        0x9 => {
            if d2 == 0 {
                Some(MaestroEvent::NoteOff { channel, key: d1 })
            } else {
                Some(MaestroEvent::NoteOn {
                    channel,
                    key: d1,
                    vel: d2,
                })
            }
        }
        0xA => Some(MaestroEvent::PolyphonicAftertouch {
            channel,
            key: d1,
            pressure: d2,
        }),
        0xB => Some(MaestroEvent::ControlChange {
            channel,
            param: d1,
            val: d2,
        }),
        0xC => Some(MaestroEvent::ProgramChange {
            channel,
            program: d1,
        }),
        0xD => Some(MaestroEvent::ChannelAftertouch {
            channel,
            pressure: d1,
        }),
        0xE => Some(MaestroEvent::PitchBendChange {
            channel,
            lsb: d1,
            msb: d2,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translator() -> MidiTranslator {
        MidiTranslator::new()
    }

    fn long(t: &MidiTranslator, bytes: &[u8]) -> Vec<MaestroEvent> {
        let mut out = Vec::new();
        t.long(0, bytes, |event| out.push(event));
        out
    }

    fn short(t: &MidiTranslator, short: u32) -> Vec<MaestroEvent> {
        let mut out = Vec::new();
        t.short(0, short, |event| out.push(event));
        out
    }

    fn ump(t: &MidiTranslator, words: &[u32]) -> Vec<(u8, MaestroEvent)> {
        let mut out = Vec::new();
        t.ump(words, |group, event| out.push((group, event)));
        out
    }

    fn note_on(channel: u8, key: u8, vel: u8) -> MaestroEvent {
        MaestroEvent::NoteOn { channel, key, vel }
    }

    #[test]
    fn short_with_a_status_byte_decodes() {
        let t = translator();
        assert_eq!(short(&t, 0x64_3C_95), vec![note_on(5, 60, 100)]);
    }

    #[test]
    fn short_reuses_running_status_from_an_earlier_short() {
        let t = translator();
        short(&t, 0x64_3C_95);

        // Data bytes only: key 62, velocity 110 against the running 0x95.
        assert_eq!(short(&t, 0x6E_3E), vec![note_on(5, 62, 110)]);
    }

    #[test]
    fn short_without_running_status_is_ignored() {
        let t = translator();
        assert_eq!(short(&t, 0x64_3C), Vec::new());
    }

    #[test]
    fn short_running_status_survives_real_time_but_not_system_common() {
        let t = translator();
        short(&t, 0x64_3C_95);

        // Timing clock must not disturb running status.
        assert_eq!(short(&t, 0xF8), Vec::new());
        assert_eq!(short(&t, 0x6E_3E), vec![note_on(5, 62, 110)]);

        // Song select does clear it, so the next data bytes go nowhere.
        assert_eq!(short(&t, 0x01_F3), Vec::new());
        assert_eq!(short(&t, 0x6E_3E), Vec::new());
    }

    #[test]
    fn short_running_status_is_per_port() {
        let t = MidiTranslator::new();
        t.short(0, 0x64_3C_95, |_| {});

        let mut out = Vec::new();
        t.short(1, 0x6E_3E, |event| out.push(event));
        assert_eq!(out, Vec::new());
    }

    #[test]
    fn short_note_on_with_zero_velocity_is_a_note_off() {
        let t = translator();
        assert_eq!(
            short(&t, 0x00_3C_90),
            vec![MaestroEvent::NoteOff {
                channel: 0,
                key: 60
            }]
        );
    }

    #[test]
    fn short_system_reset_translates() {
        let t = translator();
        assert_eq!(short(&t, 0xFF), vec![MaestroEvent::SystemReset]);
    }

    #[test]
    fn long_running_status_continues() {
        // One 0x90 status byte followed by two note pairs: the second pair
        // should reuse the running status and decode as another NoteOn.
        let t = translator();
        assert_eq!(
            long(&t, &[0x90, 60, 100, 62, 110]),
            vec![note_on(0, 60, 100), note_on(0, 62, 110)]
        );
    }

    #[test]
    fn long_data_bytes_without_running_status_are_ignored() {
        let t = translator();
        assert_eq!(long(&t, &[60, 100]), Vec::new());
    }

    #[test]
    fn long_running_status_carries_across_calls() {
        let t = translator();
        long(&t, &[0x90, 60, 100]);
        assert_eq!(long(&t, &[62, 110]), vec![note_on(0, 62, 110)]);
    }

    #[test]
    fn long_message_split_across_calls_is_reassembled() {
        // The status byte, the key and the velocity each arrive on their own.
        let t = translator();
        assert_eq!(long(&t, &[0x90]), Vec::new());
        assert_eq!(long(&t, &[60]), Vec::new());
        assert_eq!(long(&t, &[100]), vec![note_on(0, 60, 100)]);
    }

    #[test]
    fn long_one_byte_message_split_across_calls_is_reassembled() {
        let t = translator();
        assert_eq!(long(&t, &[0xC3]), Vec::new());
        assert_eq!(
            long(&t, &[42]),
            vec![MaestroEvent::ProgramChange {
                channel: 3,
                program: 42
            }]
        );
    }

    #[test]
    fn long_running_status_is_shared_with_shorts_on_the_same_port() {
        let t = translator();
        short(&t, 0x64_3C_95);
        assert_eq!(long(&t, &[62, 110]), vec![note_on(5, 62, 110)]);
    }

    #[test]
    fn long_sysex_with_terminator_is_delivered_without_its_framing() {
        let t = translator();
        assert_eq!(
            long(&t, &[0xF0, 0x7E, 0x7F, 0xF7]),
            vec![MaestroEvent::SystemExclusive(
                vec![0x7E, 0x7F].into_boxed_slice()
            )]
        );
    }

    #[test]
    fn long_sysex_split_across_calls_is_reassembled() {
        let t = translator();
        assert_eq!(long(&t, &[0xF0, 0x01, 0x02]), Vec::new());
        assert_eq!(long(&t, &[0x03, 0x04]), Vec::new());
        assert_eq!(
            long(&t, &[0x05, 0xF7]),
            vec![MaestroEvent::SystemExclusive(
                vec![0x01, 0x02, 0x03, 0x04, 0x05].into_boxed_slice()
            )]
        );
    }

    #[test]
    fn long_real_time_inside_a_sysex_does_not_corrupt_it() {
        let t = translator();
        assert_eq!(
            long(&t, &[0xF0, 0x01, 0xF8, 0x02, 0xF7]),
            vec![MaestroEvent::SystemExclusive(
                vec![0x01, 0x02].into_boxed_slice()
            )]
        );
    }

    #[test]
    fn long_real_time_between_data_bytes_does_not_corrupt_the_message() {
        let t = translator();
        assert_eq!(long(&t, &[0x90, 60, 0xFE, 100]), vec![note_on(0, 60, 100)]);
    }

    #[test]
    fn long_sysex_aborted_by_a_status_byte_is_dropped() {
        // No EOX arrives: the partial payload is discarded and the note that
        // interrupted it still decodes.
        let t = translator();
        assert_eq!(
            long(&t, &[0xF0, 0x01, 0x02, 0x90, 60, 100]),
            vec![note_on(0, 60, 100)]
        );
    }

    #[test]
    fn long_system_reset_clears_pending_state() {
        let t = translator();
        long(&t, &[0x90, 60]);

        assert_eq!(long(&t, &[0xFF]), vec![MaestroEvent::SystemReset]);
        assert_eq!(long(&t, &[100]), Vec::new());
    }

    #[test]
    fn long_unterminated_sysex_stays_pending_rather_than_being_emitted() {
        let t = translator();
        assert_eq!(long(&t, &[0xF0, 0x01, 0x02, 0x03]), Vec::new());
    }

    #[test]
    fn long_system_common_data_bytes_are_dropped() {
        // Song position pointer: two data bytes that must not decode as a note.
        let t = translator();
        long(&t, &[0x90, 60, 100]);
        assert_eq!(long(&t, &[0xF2, 0x10, 0x20]), Vec::new());
    }

    #[test]
    fn reset_drops_running_status_and_pending_bytes() {
        let t = translator();
        long(&t, &[0x90, 60]);
        t.reset();

        assert_eq!(long(&t, &[100]), Vec::new());
    }

    #[test]
    fn oversized_sysex_is_capped() {
        let t = translator();

        let mut bytes = vec![0xF0];
        bytes.extend(std::iter::repeat_n(0x01, MAX_SYSEX + 64));
        bytes.push(0xF7);

        let events = long(&t, &bytes);
        assert_eq!(events.len(), 1);
        match &events[0] {
            MaestroEvent::SystemExclusive(data) => assert_eq!(data.len(), MAX_SYSEX),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn ump_midi1_cv_note_on() {
        // MT=2, group 3, NoteOn ch 5, key 60, vel 100.
        let t = translator();
        assert_eq!(ump(&t, &[0x2395_3C64]), vec![(3, note_on(5, 60, 100))]);
    }

    #[test]
    fn ump_midi2_note_on_velocity_scales_down() {
        // MT=4, group 0, NoteOn ch 0, key 60, velocity 0xFFFF -> 127.
        let t = translator();
        assert_eq!(
            ump(&t, &[0x40_90_3C_00, 0xFFFF_0000]),
            vec![(0, note_on(0, 60, 127))]
        );
    }

    #[test]
    fn ump_midi2_note_on_zero_velocity_stays_note_on() {
        // Velocity 0 must translate to MIDI 1.0 velocity 1, not a Note Off.
        let t = translator();
        assert_eq!(
            ump(&t, &[0x40_90_3C_00, 0x0000_0000]),
            vec![(0, note_on(0, 60, 1))]
        );
    }

    #[test]
    fn ump_midi2_pitch_bend_scales_to_14_bits() {
        // Center position: 0x8000_0000 -> 14-bit 0x2000 (lsb 0, msb 0x40).
        let t = translator();
        assert_eq!(
            ump(&t, &[0x40_E0_00_00, 0x8000_0000]),
            vec![(
                0,
                MaestroEvent::PitchBendChange {
                    channel: 0,
                    lsb: 0,
                    msb: 0x40
                }
            )]
        );
    }

    #[test]
    fn ump_midi2_program_change_with_bank() {
        // Bank valid flag set: expect CC0, CC32, then the program change.
        let t = translator();
        assert_eq!(
            ump(&t, &[0x40_C0_00_01, 0x05_00_02_03]),
            vec![
                (
                    0,
                    MaestroEvent::ControlChange {
                        channel: 0,
                        param: 0,
                        val: 2
                    }
                ),
                (
                    0,
                    MaestroEvent::ControlChange {
                        channel: 0,
                        param: 32,
                        val: 3
                    }
                ),
                (
                    0,
                    MaestroEvent::ProgramChange {
                        channel: 0,
                        program: 5
                    }
                ),
            ]
        );
    }

    #[test]
    fn ump_sysex7_reassembles_across_packets() {
        // Start (6 bytes) + end (2 bytes) on group 1.
        let t = translator();
        assert_eq!(
            ump(
                &t,
                &[0x31_16_01_02, 0x03_04_05_06, 0x31_32_07_08, 0x00_00_00_00]
            ),
            vec![(
                1,
                MaestroEvent::SystemExclusive(vec![1, 2, 3, 4, 5, 6, 7, 8].into_boxed_slice())
            )]
        );
    }

    #[test]
    fn ump_sysex7_complete_in_one_packet() {
        let t = translator();
        assert_eq!(
            ump(&t, &[0x30_03_7E_7F, 0x09_00_00_00]),
            vec![(
                0,
                MaestroEvent::SystemExclusive(vec![0x7E, 0x7F, 0x09].into_boxed_slice())
            )]
        );
    }

    #[test]
    fn ump_truncated_packet_is_dropped() {
        // A MIDI 2.0 CV packet missing its second word yields nothing.
        let t = translator();
        assert_eq!(ump(&t, &[0x40_90_3C_00]), Vec::new());
    }

    #[test]
    fn ump_system_reset_translates() {
        let t = translator();
        assert_eq!(
            ump(&t, &[0x10_FF_00_00]),
            vec![(0, MaestroEvent::SystemReset)]
        );
    }
}
