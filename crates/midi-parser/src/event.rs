use crate::ump::Ump;

/// A MIDI 1.0 Channel Voice message, kept as the bytes it is on the wire.
///
/// `status` carries the channel in its low nibble; `data2` is 0 for the
/// one-byte messages (Program Change, Channel Pressure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiMessage {
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}

impl MidiMessage {
    #[inline]
    pub const fn channel(&self) -> u8 {
        self.status & 0x0F
    }

    /// The message kind, i.e. the status byte with the channel masked off.
    #[inline]
    pub const fn kind(&self) -> u8 {
        self.status & 0xF0
    }

    /// Packed as a Windows-style short message: status, then data bytes in
    /// ascending byte order.
    #[inline]
    pub const fn as_short(&self) -> u32 {
        self.status as u32 | (self.data1 as u32) << 8 | (self.data2 as u32) << 16
    }

    /// How many data bytes follow this status byte.
    #[inline]
    pub const fn data_len(status: u8) -> usize {
        match status & 0xF0 {
            0xC0 | 0xD0 => 1,
            _ => 2,
        }
    }
}

/// SMF meta event types, RP-001 §"Meta-Events".
pub mod meta {
    pub const SEQUENCE_NUMBER: u8 = 0x00;
    pub const TEXT: u8 = 0x01;
    pub const COPYRIGHT: u8 = 0x02;
    pub const TRACK_NAME: u8 = 0x03;
    pub const INSTRUMENT_NAME: u8 = 0x04;
    pub const LYRIC: u8 = 0x05;
    pub const MARKER: u8 = 0x06;
    pub const CUE_POINT: u8 = 0x07;
    pub const CHANNEL_PREFIX: u8 = 0x20;
    /// MIDI Port. Not in RP-001 — a de-facto extension every sequencer writes,
    /// and the only way a Standard MIDI File addresses more than 16 channels.
    pub const MIDI_PORT: u8 = 0x21;
    pub const END_OF_TRACK: u8 = 0x2F;
    pub const SET_TEMPO: u8 = 0x51;
    pub const SMPTE_OFFSET: u8 = 0x54;
    pub const TIME_SIGNATURE: u8 = 0x58;
    pub const KEY_SIGNATURE: u8 = 0x59;
    pub const SEQUENCER_SPECIFIC: u8 = 0x7F;
}

/// An SMF meta event. `data` is the payload with the `FF`, type byte and
/// length prefix already stripped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta<'a> {
    pub kind: u8,
    pub data: &'a [u8],
}

impl Meta<'_> {
    /// Microseconds per quarter note, for a Set Tempo event.
    pub fn tempo(&self) -> Option<u32> {
        match (self.kind, self.data) {
            (meta::SET_TEMPO, [a, b, c]) => {
                Some(u32::from(*a) << 16 | u32::from(*b) << 8 | u32::from(*c))
            }
            _ => None,
        }
    }

    /// Port number, for a MIDI Port event.
    pub fn port(&self) -> Option<u8> {
        match (self.kind, self.data) {
            (meta::MIDI_PORT, [port, ..]) => Some(*port),
            _ => None,
        }
    }
}

/// One event, borrowed straight out of the file.
///
/// SMF files produce [`Midi`](EventRef::Midi), [`SysEx`](EventRef::SysEx),
/// [`Escape`](EventRef::Escape) and [`Meta`](EventRef::Meta); clip files
/// produce [`Midi`](EventRef::Midi) and [`Ump`](EventRef::Ump).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventRef<'a> {
    Midi(MidiMessage),
    /// SysEx payload with the leading `F0` and trailing `F7` stripped.
    SysEx(&'a [u8]),
    /// An `F7` escape: raw bytes to put on the wire as-is.
    Escape(&'a [u8]),
    Meta(Meta<'a>),
    Ump(Ump),
}

impl EventRef<'_> {
    /// True for the event that ends an SMF track.
    #[inline]
    pub fn is_end_of_track(&self) -> bool {
        matches!(self, Self::Meta(m) if m.kind == meta::END_OF_TRACK)
    }

    /// Microseconds per quarter note, from either format's tempo message.
    pub fn tempo(&self) -> Option<u32> {
        match self {
            Self::Meta(m) => m.tempo(),
            Self::Ump(u) => u.tempo(),
            _ => None,
        }
    }
}
