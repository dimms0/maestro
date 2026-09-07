pub(crate) mod ump;

mod translator;

pub(crate) use translator::MidiTranslator;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MaestroTimedEvent {
    pub event: MaestroEvent,
    pub pos: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MaestroEvent {
    NoteOff { channel: u8, key: u8 },
    NoteOn { channel: u8, key: u8, vel: u8 },
    PolyphonicAftertouch { channel: u8, key: u8, pressure: u8 },
    ControlChange { channel: u8, param: u8, val: u8 },
    ProgramChange { channel: u8, program: u8 },
    ChannelAftertouch { channel: u8, pressure: u8 },
    PitchBendChange { channel: u8, lsb: u8, msb: u8 },

    SystemReset,
    SystemExclusive(Box<[u8]>),
}
