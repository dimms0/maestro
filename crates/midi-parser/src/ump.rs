//! Universal MIDI Packet, M2-104-UM Appendix F.
//!
//! A UMP is kept as the 32-bit words it is stored as, with accessors for the
//! fields that matter here. Nothing is decoded into per-message structs: the
//! clip file format is a stream of UMPs, and consumers that care about a
//! particular message already know its layout.

use crate::event::MidiMessage;

/// Message Types, M2-104-UM §2.1.4.
pub mod mt {
    pub const UTILITY: u8 = 0x0;
    pub const SYSTEM: u8 = 0x1;
    pub const MIDI1_CHANNEL_VOICE: u8 = 0x2;
    pub const SYSEX7: u8 = 0x3;
    pub const MIDI2_CHANNEL_VOICE: u8 = 0x4;
    pub const DATA128: u8 = 0x5;
    pub const FLEX_DATA: u8 = 0xD;
    pub const STREAM: u8 = 0xF;
}

/// Utility statuses, M2-104-UM Table 26.
pub mod utility {
    pub const NOOP: u8 = 0x0;
    pub const JR_CLOCK: u8 = 0x1;
    pub const JR_TIMESTAMP: u8 = 0x2;
    /// Delta Clockstamp Ticks Per Quarter Note: 16-bit value in the low half.
    pub const DCTPQ: u8 = 0x3;
    /// Delta Clockstamp: 20-bit tick count in the low 20 bits.
    pub const DELTA_CLOCKSTAMP: u8 = 0x4;
}

/// UMP Stream statuses used by clip files, M2-104-UM Table 33.
pub mod stream {
    pub const START_OF_CLIP: u16 = 0x20;
    pub const END_OF_CLIP: u16 = 0x21;
}

/// Flex Data status banks and statuses, M2-104-UM Table 32.
pub mod flex {
    pub const BANK_SETUP: u8 = 0x00;
    pub const SET_TEMPO: u8 = 0x00;
    pub const SET_TIME_SIGNATURE: u8 = 0x01;
    pub const SET_METRONOME: u8 = 0x02;
    pub const SET_KEY_SIGNATURE: u8 = 0x05;
    pub const SET_CHORD_NAME: u8 = 0x06;

    pub const BANK_METADATA: u8 = 0x01;
    pub const PROJECT_NAME: u8 = 0x01;
    pub const COMPOSITION_NAME: u8 = 0x02;
    pub const CLIP_NAME: u8 = 0x03;
    pub const COPYRIGHT: u8 = 0x04;
    pub const COMPOSER_NAME: u8 = 0x05;
    pub const LYRICIST_NAME: u8 = 0x06;

    pub const BANK_PERFORMANCE: u8 = 0x02;
    pub const LYRICS: u8 = 0x01;
}

/// The largest tick count a single Delta Clockstamp can carry.
pub const MAX_DELTA_CLOCKSTAMP: u32 = 0x000F_FFFF;

/// Words per UMP by Message Type, M2-104-UM Table 4. Reserved types have a
/// defined size too, which is what lets an unknown message be skipped.
const WORDS: [u8; 16] = [1, 1, 1, 2, 2, 4, 1, 1, 2, 2, 2, 3, 3, 4, 4, 4];

/// A Universal MIDI Packet: one to four big-endian 32-bit words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ump {
    words: [u32; 4],
    len: u8,
}

impl Ump {
    /// How many words a packet whose first word is `first` occupies. Every
    /// Message Type has a fixed size, including the reserved ones, which is
    /// what lets an unrecognised message be stepped over.
    #[inline]
    pub const fn word_count(first: u32) -> usize {
        WORDS[(first >> 28) as usize] as usize
    }

    /// Builds a packet from words already in host order, truncating or
    /// zero-padding to the length its Message Type calls for.
    pub fn new(words: &[u32]) -> Option<Self> {
        let first = *words.first()?;
        let len = WORDS[(first >> 28) as usize] as usize;
        if words.len() < len {
            return None;
        }

        let mut packed = [0u32; 4];
        packed[..len].copy_from_slice(&words[..len]);
        Some(Self {
            words: packed,
            len: len as u8,
        })
    }

    #[inline]
    pub fn words(&self) -> &[u32] {
        &self.words[..self.len as usize]
    }

    #[inline]
    pub const fn message_type(&self) -> u8 {
        (self.words[0] >> 28) as u8
    }

    /// The Group field. Message Types 0x0 and 0xF have no Group, and report 0.
    #[inline]
    pub const fn group(&self) -> u8 {
        match self.message_type() {
            mt::UTILITY | mt::STREAM => 0,
            _ => ((self.words[0] >> 24) & 0xF) as u8,
        }
    }

    /// The 4-bit status of a Utility message.
    #[inline]
    pub const fn utility_status(&self) -> u8 {
        ((self.words[0] >> 20) & 0xF) as u8
    }

    /// The 10-bit status of a UMP Stream message.
    #[inline]
    pub const fn stream_status(&self) -> u16 {
        ((self.words[0] >> 16) & 0x3FF) as u16
    }

    /// Ticks carried by a Delta Clockstamp, or ticks per quarter note carried
    /// by a DCTPQ.
    pub const fn delta_ticks(&self) -> Option<u32> {
        if self.message_type() != mt::UTILITY {
            return None;
        }
        match self.utility_status() {
            utility::DELTA_CLOCKSTAMP => Some(self.words[0] & MAX_DELTA_CLOCKSTAMP),
            utility::DCTPQ => Some(self.words[0] & 0xFFFF),
            _ => None,
        }
    }

    /// True for the Utility messages that carry timing rather than music, none
    /// of which are events in their own right.
    #[inline]
    pub const fn is_timing(&self) -> bool {
        self.message_type() == mt::UTILITY
    }

    /// Microseconds per quarter note, for a Flex Data Set Tempo message. The
    /// message itself counts in 10 ns units.
    pub const fn tempo(&self) -> Option<u32> {
        if self.message_type() != mt::FLEX_DATA
            || (self.words[0] >> 8) as u8 != flex::BANK_SETUP
            || self.words[0] as u8 != flex::SET_TEMPO
        {
            return None;
        }
        Some(self.words[1] / 100)
    }

    /// The MIDI 1.0 Channel Voice message this packet holds, if it holds one.
    pub const fn midi1(&self) -> Option<MidiMessage> {
        if self.message_type() != mt::MIDI1_CHANNEL_VOICE {
            return None;
        }
        Some(MidiMessage {
            status: (self.words[0] >> 16) as u8,
            data1: (self.words[0] >> 8) as u8 & 0x7F,
            data2: self.words[0] as u8 & 0x7F,
        })
    }

    /// Payload bytes of a SysEx7 packet, along with its 4-bit status.
    pub fn sysex7(&self) -> Option<(u8, [u8; 6], usize)> {
        if self.message_type() != mt::SYSEX7 {
            return None;
        }

        let status = ((self.words[0] >> 20) & 0xF) as u8;
        let count = ((self.words[0] >> 16) & 0xF) as usize;
        let bytes = [
            (self.words[0] >> 8) as u8,
            self.words[0] as u8,
            (self.words[1] >> 24) as u8,
            (self.words[1] >> 16) as u8,
            (self.words[1] >> 8) as u8,
            self.words[1] as u8,
        ];

        Some((status, bytes, count.min(6)))
    }

    pub const fn utility(status: u8, data: u32) -> Self {
        Self::one((status as u32) << 20 | data)
    }

    pub const fn delta_clockstamp(ticks: u32) -> Self {
        Self::utility(utility::DELTA_CLOCKSTAMP, ticks & MAX_DELTA_CLOCKSTAMP)
    }

    pub const fn dctpq(ticks_per_quarter: u16) -> Self {
        Self::utility(utility::DCTPQ, ticks_per_quarter as u32)
    }

    pub const fn stream(status: u16) -> Self {
        Self {
            words: [(mt::STREAM as u32) << 28 | (status as u32) << 16, 0, 0, 0],
            len: 4,
        }
    }

    pub const fn midi1_message(group: u8, message: MidiMessage) -> Self {
        Self::one(
            (mt::MIDI1_CHANNEL_VOICE as u32) << 28
                | (group as u32 & 0xF) << 24
                | (message.status as u32) << 16
                | (message.data1 as u32 & 0x7F) << 8
                | (message.data2 as u32 & 0x7F),
        )
    }

    /// A Flex Data message addressed to a Group (Address = 1, Channel = 0),
    /// which is how clip files carry what used to be SMF meta events.
    ///
    /// `form` is 0 for a message that fits one packet, or 1/2/3 for the start,
    /// middle and end of one that does not.
    pub const fn flex_data(group: u8, form: u8, bank: u8, status: u8, data: [u32; 3]) -> Self {
        Self {
            words: [
                (mt::FLEX_DATA as u32) << 28
                    | (group as u32 & 0xF) << 24
                    | (form as u32 & 0x3) << 22
                    | 1 << 20
                    | (bank as u32) << 8
                    | status as u32,
                data[0],
                data[1],
                data[2],
            ],
            len: 4,
        }
    }

    /// Form, status bank and status of a Flex Data message.
    pub const fn flex(&self) -> Option<(u8, u8, u8)> {
        if self.message_type() != mt::FLEX_DATA {
            return None;
        }
        Some((
            ((self.words[0] >> 22) & 0x3) as u8,
            (self.words[0] >> 8) as u8,
            self.words[0] as u8,
        ))
    }

    /// The three data words of a Flex Data or UMP Stream message.
    pub const fn payload(&self) -> [u32; 3] {
        [self.words[1], self.words[2], self.words[3]]
    }

    /// A MIDI 2.0 Channel Voice message as status byte, index byte and 32-bit
    /// data word.
    pub const fn midi2(&self) -> Option<(u8, u8, u8, u32)> {
        if self.message_type() != mt::MIDI2_CHANNEL_VOICE {
            return None;
        }
        Some((
            (self.words[0] >> 16) as u8,
            (self.words[0] >> 8) as u8,
            self.words[0] as u8,
            self.words[1],
        ))
    }

    pub const fn set_tempo(group: u8, micros_per_quarter: u32) -> Self {
        Self::flex_data(
            group,
            0,
            flex::BANK_SETUP,
            flex::SET_TEMPO,
            [micros_per_quarter.saturating_mul(100), 0, 0],
        )
    }

    /// One SysEx7 packet holding up to six payload bytes.
    pub fn sysex7_packet(group: u8, status: u8, payload: &[u8]) -> Self {
        let mut bytes = [0u8; 6];
        let count = payload.len().min(6);
        bytes[..count].copy_from_slice(&payload[..count]);

        Self {
            words: [
                (mt::SYSEX7 as u32) << 28
                    | (group as u32 & 0xF) << 24
                    | (status as u32 & 0xF) << 20
                    | (count as u32) << 16
                    | u32::from(bytes[0]) << 8
                    | u32::from(bytes[1]),
                u32::from(bytes[2]) << 24
                    | u32::from(bytes[3]) << 16
                    | u32::from(bytes[4]) << 8
                    | u32::from(bytes[5]),
                0,
                0,
            ],
            len: 2,
        }
    }

    const fn one(word: u32) -> Self {
        Self {
            words: [word, 0, 0, 0],
            len: 1,
        }
    }
}
