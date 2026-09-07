use crate::event::MaestroEvent;

mod packet;

pub(super) use packet::{Midi2ChannelVoiceMessage, ParseError, Ump, parse_ump};

pub const UMP_GROUPS: u8 = 16;

/// Reserved types still have a defined size
pub(super) const UMP_MT_WORDS: [usize; 16] = [1, 1, 1, 2, 2, 4, 1, 1, 2, 2, 2, 3, 3, 4, 4, 4];

pub(super) fn midi2_cv_events(m: &Midi2ChannelVoiceMessage, mut emit: impl FnMut(MaestroEvent)) {
    let channel = m.channel;
    match m.status {
        0x8 => emit(MaestroEvent::NoteOff {
            channel,
            key: m.byte3,
        }),
        // velocity 16 -> 7 bit. A MIDI 2.0 Note On with velocity 0 is
        // still a Note On — the spec translates it to velocity 1.
        0x9 => {
            let vel16 = (m.data32 >> 16) & 0xFFFF;
            let vel = ((vel16 >> 9) as u8).max(1);
            emit(MaestroEvent::NoteOn {
                channel,
                key: m.byte3,
                vel,
            });
        }
        // Poly Pressure: data 32 -> 7 bit.
        0xA => emit(MaestroEvent::PolyphonicAftertouch {
            channel,
            key: m.byte3,
            pressure: (m.data32 >> 25) as u8,
        }),
        // Control Change: data 32 -> 7 bit.
        0xB => emit(MaestroEvent::ControlChange {
            channel,
            param: m.byte3,
            val: (m.data32 >> 25) as u8,
        }),
        // Registered / Assignable Controller (RPN / NRPN): data 32 -> 14 bit,
        // emitted as the classic CC 101/100 (or 99/98) + CC 6/38 sequence.
        0x2 | 0x3 => {
            let (bank_cc, index_cc) = if m.status == 0x2 {
                (101, 100)
            } else {
                (99, 98)
            };
            let data = (m.data32 >> 18) & 0x3FFF;
            for (param, val) in [
                (bank_cc, m.byte3),
                (index_cc, m.byte4),
                (6, (data >> 7) as u8),
                (38, (data & 0x7F) as u8),
            ] {
                emit(MaestroEvent::ControlChange {
                    channel,
                    param,
                    val,
                });
            }
        }
        // Program Change: optionally carries a bank select (flag bit 0 of the
        // option byte), which maps to CC 0 / CC 32 before the program change.
        0xC => {
            if m.byte4 & 0x1 != 0 {
                emit(MaestroEvent::ControlChange {
                    channel,
                    param: 0,
                    val: ((m.data32 >> 8) & 0x7F) as u8,
                });
                emit(MaestroEvent::ControlChange {
                    channel,
                    param: 32,
                    val: (m.data32 & 0x7F) as u8,
                });
            }
            emit(MaestroEvent::ProgramChange {
                channel,
                program: ((m.data32 >> 24) & 0x7F) as u8,
            });
        }
        // Channel Pressure: data 32 -> 7 bit.
        0xD => emit(MaestroEvent::ChannelAftertouch {
            channel,
            pressure: (m.data32 >> 25) as u8,
        }),
        // Pitch Bend: data 32 -> 14 bit.
        0xE => {
            let val = m.data32 >> 18;
            emit(MaestroEvent::PitchBendChange {
                channel,
                lsb: (val & 0x7F) as u8,
                msb: ((val >> 7) & 0x7F) as u8,
            });
        }
        _ => {}
    }
}
