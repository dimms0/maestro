#[derive(Debug, Clone, PartialEq)]
pub enum Ump {
    Utility(UtilityMessage),
    SystemCommonRealTime(SystemCommonRealTimeMessage),
    Midi1ChannelVoice(Midi1ChannelVoiceMessage),
    Data64Bit(Data64BitMessage),
    Midi2ChannelVoice(Midi2ChannelVoiceMessage),
    Data128Bit(Data128BitMessage),
    FlexData(FlexDataMessage),
    Stream(UmpStreamMessage),
}

#[derive(Debug, Clone, PartialEq)]
pub struct UtilityMessage {
    pub group: u8,
    pub status: u8,
    pub data: u32, // Contains the 16-bit or 20-bit timestamp/clock details
}

#[derive(Debug, Clone, PartialEq)]
pub struct SystemCommonRealTimeMessage {
    pub group: u8,
    pub status: u8,
    pub byte3: u8,
    pub byte4: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Midi1ChannelVoiceMessage {
    pub group: u8,
    pub status: u8,
    pub channel: u8,
    pub byte3: u8,
    pub byte4: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Data64BitMessage {
    pub group: u8,
    pub status: u8,
    pub channel_other: u8,
    pub data: [u8; 6], // Bytes 3 to 8
}

#[derive(Debug, Clone, PartialEq)]
pub struct Midi2ChannelVoiceMessage {
    pub group: u8,
    pub status: u8,
    pub channel: u8,
    pub byte3: u8,
    pub byte4: u8,
    pub data32: u32, // Bytes 5-8 (32-bit data field)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Data128BitMessage {
    pub group: u8,
    pub status: u8,
    pub count_or_id: u8, // #bytes or mds id
    pub stream_id: u8,
    pub payload: [u8; 13], // Bytes 4 through 16 packed
}

#[derive(Debug, Clone, PartialEq)]
pub struct FlexDataMessage {
    pub group: u8,
    pub form: u8,
    pub address: u8,
    pub channel: u8,
    pub status_bank: u8,
    pub status: u8,
    pub payload: [u32; 3], // Bytes 5-16 (Words 1, 2, and 3)
}

#[derive(Debug, Clone, PartialEq)]
pub struct UmpStreamMessage {
    pub group: u8,
    pub status: u8,
    pub byte3: u8,
    pub byte4: u8,
    pub payload: [u32; 3], // Bytes 5-16 (Words 1, 2, and 3)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    InsufficientWords,
    UnknownMessageType(u8),
}

pub fn parse_ump(words: &[u32]) -> Result<(Ump, usize), ParseError> {
    if words.is_empty() {
        return Err(ParseError::InsufficientWords);
    }

    let w0 = words[0];
    let mt = ((w0 >> 28) & 0xF) as u8;
    let group = ((w0 >> 24) & 0xF) as u8;

    match mt {
        // MT 0x0: UTILITY (1 Word / 32 bits)
        0x0 => {
            let status = ((w0 >> 20) & 0xF) as u8;
            let data = w0 & 0xF_FFFF; // 20-bit lower data field
            Ok((
                Ump::Utility(UtilityMessage {
                    group,
                    status,
                    data,
                }),
                1,
            ))
        }

        // MT 0x1: SYSTEM COMMON & REAL TIME (1 Word / 32 bits)
        0x1 => {
            let status = ((w0 >> 16) & 0xFF) as u8;
            let byte3 = ((w0 >> 8) & 0xFF) as u8;
            let byte4 = (w0 & 0xFF) as u8;
            Ok((
                Ump::SystemCommonRealTime(SystemCommonRealTimeMessage {
                    group,
                    status,
                    byte3,
                    byte4,
                }),
                1,
            ))
        }

        // MT 0x2: MIDI 1.0 CHANNEL VOICE (1 Word / 32 bits)
        0x2 => {
            let status = ((w0 >> 20) & 0xF) as u8;
            let channel = ((w0 >> 16) & 0xF) as u8;
            let byte3 = ((w0 >> 8) & 0xFF) as u8;
            let byte4 = (w0 & 0xFF) as u8;
            Ok((
                Ump::Midi1ChannelVoice(Midi1ChannelVoiceMessage {
                    group,
                    status,
                    channel,
                    byte3,
                    byte4,
                }),
                1,
            ))
        }

        // MT 0x3: DATA 64 BIT / SysEx 7 (2 Words / 64 bits)
        0x3 => {
            if words.len() < 2 {
                return Err(ParseError::InsufficientWords);
            }
            let w1 = words[1];
            let status = ((w0 >> 20) & 0xF) as u8;
            let channel_other = ((w0 >> 16) & 0xF) as u8;
            let data = [
                ((w0 >> 8) & 0xFF) as u8,
                (w0 & 0xFF) as u8,
                ((w1 >> 24) & 0xFF) as u8,
                ((w1 >> 16) & 0xFF) as u8,
                ((w1 >> 8) & 0xFF) as u8,
                (w1 & 0xFF) as u8,
            ];
            Ok((
                Ump::Data64Bit(Data64BitMessage {
                    group,
                    status,
                    channel_other,
                    data,
                }),
                2,
            ))
        }

        // MT 0x4: MIDI 2.0 CHANNEL VOICE (2 Words / 64 bits)
        0x4 => {
            if words.len() < 2 {
                return Err(ParseError::InsufficientWords);
            }
            let w1 = words[1];
            let status = ((w0 >> 20) & 0xF) as u8;
            let channel = ((w0 >> 16) & 0xF) as u8;
            let byte3 = ((w0 >> 8) & 0xFF) as u8;
            let byte4 = (w0 & 0xFF) as u8;
            Ok((
                Ump::Midi2ChannelVoice(Midi2ChannelVoiceMessage {
                    group,
                    status,
                    channel,
                    byte3,
                    byte4,
                    data32: w1,
                }),
                2,
            ))
        }

        // MT 0x5: DATA 128 BIT / SysEx 8 (4 Words / 128 bits)
        0x5 => {
            if words.len() < 4 {
                return Err(ParseError::InsufficientWords);
            }
            let w1 = words[1];
            let w2 = words[2];
            let w3 = words[3];
            let status = ((w0 >> 20) & 0xF) as u8;
            let count_or_id = ((w0 >> 16) & 0xF) as u8;
            let stream_id = ((w0 >> 8) & 0xFF) as u8;
            let payload = [
                (w0 & 0xFF) as u8,
                ((w1 >> 24) & 0xFF) as u8,
                ((w1 >> 16) & 0xFF) as u8,
                ((w1 >> 8) & 0xFF) as u8,
                (w1 & 0xFF) as u8,
                ((w2 >> 24) & 0xFF) as u8,
                ((w2 >> 16) & 0xFF) as u8,
                ((w2 >> 8) & 0xFF) as u8,
                (w2 & 0xFF) as u8,
                ((w3 >> 24) & 0xFF) as u8,
                ((w3 >> 16) & 0xFF) as u8,
                ((w3 >> 8) & 0xFF) as u8,
                (w3 & 0xFF) as u8,
            ];
            Ok((
                Ump::Data128Bit(Data128BitMessage {
                    group,
                    status,
                    count_or_id,
                    stream_id,
                    payload,
                }),
                4,
            ))
        }

        // MT 0xD: FLEX DATA (4 Words / 128 bits)
        0xD => {
            if words.len() < 4 {
                return Err(ParseError::InsufficientWords);
            }
            let form = ((w0 >> 22) & 0x3) as u8;
            let address = ((w0 >> 20) & 0x3) as u8;
            let channel = ((w0 >> 16) & 0xF) as u8;
            let status_bank = ((w0 >> 8) & 0xFF) as u8;
            let status = (w0 & 0xFF) as u8;
            Ok((
                Ump::FlexData(FlexDataMessage {
                    group,
                    form,
                    address,
                    channel,
                    status_bank,
                    status,
                    payload: [words[1], words[2], words[3]],
                }),
                4,
            ))
        }

        // MT 0xF: UMP STREAM (4 Words / 128 bits)
        0xF => {
            if words.len() < 4 {
                return Err(ParseError::InsufficientWords);
            }
            let status = ((w0 >> 16) & 0xFF) as u8;
            let byte3 = ((w0 >> 8) & 0xFF) as u8;
            let byte4 = (w0 & 0xFF) as u8;
            Ok((
                Ump::Stream(UmpStreamMessage {
                    group,
                    status,
                    byte3,
                    byte4,
                    payload: [words[1], words[2], words[3]],
                }),
                4,
            ))
        }

        _ => Err(ParseError::UnknownMessageType(mt)),
    }
}
