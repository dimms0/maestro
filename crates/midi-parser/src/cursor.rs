use std::ops::Range;

use crate::{
    error::{Error, Result},
    event::{EventRef, Meta, MidiMessage, meta},
    ump::{Ump, mt, utility},
    varlen,
};

/// Walks one track's events in place, decoding straight out of the file.
///
/// Both file formats reduce to the same thing: a stream of events, each with a
/// tick offset from the one before. Events come out with absolute ticks so a
/// merge across tracks needs no bookkeeping of its own.
/// An event with its absolute tick and UMP Group.
pub type Timed<'a> = (u64, u8, EventRef<'a>);

pub enum Cursor<'a> {
    Smf(SmfCursor<'a>),
    Clip(ClipCursor<'a>),
}

impl<'a> Cursor<'a> {
    pub(crate) fn smf(bytes: &'a [u8], range: Range<usize>, strict: bool) -> Self {
        Self::Smf(SmfCursor {
            bytes,
            pos: range.start,
            end: range.end,
            start: range.start,
            tick: 0,
            status: 0,
            done: false,
            strict,
        })
    }

    pub(crate) fn clip(bytes: &'a [u8], start: usize, strict: bool) -> Self {
        Self::Clip(ClipCursor {
            bytes,
            pos: start,
            tick: 0,
            done: false,
            strict,
        })
    }

    /// The next event, the absolute tick it falls on, and the UMP Group it is
    /// addressed to. SMF tracks have no Group and always report 0.
    #[inline]
    pub fn next_event(&mut self) -> Option<Result<Timed<'a>>> {
        match self {
            Self::Smf(c) => c.next_event(),
            Self::Clip(c) => c.next_event(),
        }
    }

    /// Ticks per quarter note declared by a clip file's DCTPQ message,
    /// consuming the cursor. Always `None` for an SMF track.
    pub(crate) fn find_dctpq(mut self) -> Option<u16> {
        let Self::Clip(cursor) = &mut self else {
            return None;
        };

        while let Some(packet) = cursor.next_packet() {
            if packet.message_type() != mt::UTILITY {
                return None;
            }
            if packet.utility_status() == utility::DCTPQ {
                return packet.delta_ticks().map(|ticks| ticks as u16);
            }
        }

        None
    }
}

impl<'a> Iterator for Cursor<'a> {
    type Item = Result<Timed<'a>>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.next_event()
    }
}

pub struct SmfCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    end: usize,
    start: usize,
    tick: u64,
    /// Running status, RP-001 §"Running Status". Zero when none is armed.
    status: u8,
    done: bool,
    strict: bool,
}

impl<'a> SmfCursor<'a> {
    fn next_event(&mut self) -> Option<Result<Timed<'a>>> {
        if self.done {
            return None;
        }

        match self.decode() {
            Ok(Some(event)) => Some(Ok(event)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(error) => {
                self.done = true;
                Some(Err(error))
            }
        }
    }

    fn decode(&mut self) -> Result<Option<Timed<'a>>> {
        loop {
            if self.pos >= self.end {
                // Running off the end of a track without an End of Track is
                // common in files that were cut short.
                return if self.strict {
                    Err(Error::MissingEndOfTrack(self.start))
                } else {
                    Ok(None)
                };
            }

            let delta = varlen::read(self.bytes, &mut self.pos).ok_or(Error::Truncated(self.pos))?;
            self.tick += u64::from(delta);

            let head = *self.byte(self.pos)?;
            let status = if head >= 0x80 {
                self.pos += 1;
                // System messages clear running status; channel messages arm it.
                self.status = if head < 0xF0 { head } else { 0 };
                head
            } else if self.status != 0 {
                self.status
            } else if self.strict {
                return Err(Error::NoRunningStatus(self.pos));
            } else {
                self.pos += 1;
                continue;
            };

            let event = match status {
                0x80..=0xEF => {
                    let len = MidiMessage::data_len(status);
                    let data = self.take(len)?;
                    EventRef::Midi(MidiMessage {
                        status,
                        data1: data[0] & 0x7F,
                        data2: if len == 2 { data[1] & 0x7F } else { 0 },
                    })
                }

                0xF0 => {
                    let payload = self.take_chunk()?;
                    // The terminating F7 is part of the declared length.
                    EventRef::SysEx(payload.strip_suffix(&[0xF7]).unwrap_or(payload))
                }

                0xF7 => EventRef::Escape(self.take_chunk()?),

                0xFF => {
                    let kind = *self.byte(self.pos)?;
                    self.pos += 1;
                    let data = self.take_chunk()?;
                    if kind == meta::END_OF_TRACK {
                        self.done = true;
                    }
                    EventRef::Meta(Meta { kind, data })
                }

                // System Common and Real Time have no business in a track, and
                // carry no length to skip by. Step over their data bytes.
                _ => {
                    let len = match status {
                        0xF1 | 0xF3 => 1,
                        0xF2 => 2,
                        _ => 0,
                    };
                    self.pos = (self.pos + len).min(self.end);
                    continue;
                }
            };

            return Ok(Some((self.tick, 0, event)));
        }
    }

    #[inline]
    fn byte(&self, at: usize) -> Result<&'a u8> {
        self.bytes
            .get(at)
            .filter(|_| at < self.end)
            .ok_or(Error::Truncated(at))
    }

    #[inline]
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.pos + len;
        if end > self.end {
            return Err(Error::Truncated(self.pos));
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// A variable-length-prefixed payload, as SysEx, escapes and meta events
    /// all carry.
    #[inline]
    fn take_chunk(&mut self) -> Result<&'a [u8]> {
        let len = varlen::read(self.bytes, &mut self.pos).ok_or(Error::Truncated(self.pos))?;
        self.take(len as usize)
    }
}

pub struct ClipCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    tick: u64,
    done: bool,
    strict: bool,
}

impl<'a> ClipCursor<'a> {
    fn next_event(&mut self) -> Option<Result<Timed<'a>>> {
        if self.done {
            return None;
        }

        while let Some(packet) = self.next_packet() {
            // M2-116-U §3.2.2: Delta Clockstamps accumulate into the timing of
            // the next real message; the other Utility messages are timing
            // scaffolding and carry no event of their own.
            if packet.message_type() == mt::UTILITY {
                if packet.utility_status() == utility::DELTA_CLOCKSTAMP {
                    self.tick += u64::from(packet.delta_ticks().unwrap_or(0));
                }
                continue;
            }

            let event = match packet.midi1() {
                Some(message) => EventRef::Midi(message),
                None => EventRef::Ump(packet),
            };

            return Some(Ok((self.tick, packet.group(), event)));
        }

        self.done = true;
        if self.strict && self.pos < self.bytes.len() {
            return Some(Err(Error::Truncated(self.pos)));
        }

        None
    }

    fn next_packet(&mut self) -> Option<Ump> {
        let mut words = [0u32; 4];
        let first = self.word(self.pos)?;
        words[0] = first;

        let count = Ump::word_count(first);
        for (index, word) in words.iter_mut().enumerate().take(count).skip(1) {
            *word = self.word(self.pos + index * 4)?;
        }

        self.pos += count * 4;
        Ump::new(&words[..count])
    }

    #[inline]
    fn word(&self, at: usize) -> Option<u32> {
        Some(u32::from_be_bytes(
            self.bytes.get(at..at + 4)?.try_into().ok()?,
        ))
    }
}
