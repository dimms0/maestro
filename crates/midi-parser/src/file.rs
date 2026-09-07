use std::{ops::Range, path::Path};

use crate::{
    cursor::Cursor,
    error::{Error, Result},
    source::Source,
};

/// What delta times in a file are measured in, RP-001 §"division".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Division {
    /// Ticks per quarter note.
    Ppq(u16),
    /// Ticks per second, expressed as SMPTE frames per second and ticks per
    /// frame. Tempo has no bearing on timing in this mode.
    Smpte { fps: u8, subframes: u8 },
}

impl Division {
    fn parse(raw: u16) -> Self {
        if raw & 0x8000 == 0 {
            Self::Ppq(raw & 0x7FFF)
        } else {
            // Bits 14-8 hold the frame rate as a negative number.
            Self::Smpte {
                fps: (raw >> 8) as u8 & 0x7F,
                subframes: raw as u8,
            }
        }
    }

    /// Ticks per quarter note, or `None` for SMPTE timing.
    pub const fn ppq(&self) -> Option<u16> {
        match self {
            Self::Ppq(ppq) => Some(*ppq),
            Self::Smpte { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// Standard MIDI File, RP-001.
    Smf,
    /// MIDI Clip File, M2-116-U.
    Clip,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Reject anything that violates the specs instead of recovering from it.
    pub strict: bool,
}

/// A MIDI file, opened but not yet decoded.
///
/// Opening only reads the header and finds where the tracks are; no event is
/// touched until iteration starts.
pub struct MidiFile<'a> {
    source: Source<'a>,
    kind: FileKind,
    format: u16,
    division: Division,
    tracks: Vec<Range<usize>>,
    options: Options,
}

const MTHD: &[u8] = b"MThd";
const MTRK: &[u8] = b"MTrk";
const SMF2CLIP: &[u8] = b"SMF2CLIP";

impl MidiFile<'static> {
    /// Opens a file by memory-mapping it. The OS pages it in on demand and can
    /// evict it again, so a multi-gigabyte file costs almost no resident memory.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path, Options::default())
    }

    pub fn open_with(path: impl AsRef<Path>, options: Options) -> Result<Self> {
        Self::build(Source::map(path.as_ref())?, options)
    }

    /// Reads the whole file into memory.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::load_with(path, Options::default())
    }

    pub fn load_with(path: impl AsRef<Path>, options: Options) -> Result<Self> {
        Self::build(Source::read(path.as_ref())?, options)
    }
}

impl<'a> MidiFile<'a> {
    pub fn from_slice(bytes: &'a [u8]) -> Result<Self> {
        Self::from_slice_with(bytes, Options::default())
    }

    pub fn from_slice_with(bytes: &'a [u8], options: Options) -> Result<Self> {
        Self::build(Source::Borrowed(bytes), options)
    }

    fn build(source: Source<'a>, options: Options) -> Result<Self> {
        let parsed = {
            let bytes: &[u8] = &source;
            if bytes.starts_with(MTHD) {
                parse_smf(bytes, options)?
            } else if bytes.starts_with(SMF2CLIP) {
                parse_clip(bytes, options)?
            } else {
                return Err(Error::BadMagic);
            }
        };

        Ok(Self {
            source,
            kind: parsed.kind,
            format: parsed.format,
            division: parsed.division,
            tracks: parsed.tracks,
            options,
        })
    }

    pub fn kind(&self) -> FileKind {
        self.kind
    }

    /// SMF format number: 0 for a single track, 1 for parallel tracks. Clip
    /// files report 0, being a single sequence.
    pub fn format(&self) -> u16 {
        self.format
    }

    pub fn division(&self) -> Division {
        self.division
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    pub fn bytes(&self) -> &[u8] {
        &self.source
    }

    /// Walks one track on its own. Events carry absolute ticks.
    pub fn track(&self, index: usize) -> Option<Cursor<'_>> {
        let range = self.tracks.get(index)?.clone();
        Some(match self.kind {
            FileKind::Smf => Cursor::smf(&self.source, range, self.options.strict),
            FileKind::Clip => Cursor::clip(&self.source, range.start, self.options.strict),
        })
    }
}

struct Parsed {
    kind: FileKind,
    format: u16,
    division: Division,
    tracks: Vec<Range<usize>>,
}

fn parse_smf(bytes: &[u8], options: Options) -> Result<Parsed> {
    let header = read_u32(bytes, 4).ok_or(Error::Truncated(4))? as usize;
    if header < 6 || bytes.len() < 8 + header {
        return Err(Error::Truncated(4));
    }

    let format = read_u16(bytes, 8).ok_or(Error::Truncated(8))?;
    if format > 1 {
        return Err(Error::UnsupportedFormat(format));
    }
    let division = Division::parse(read_u16(bytes, 12).ok_or(Error::Truncated(12))?);

    // The declared header length is authoritative rather than the six bytes we
    // know about: a longer one means a later revision added fields to skip.
    let mut tracks = Vec::new();
    let mut pos = 8 + header;
    while pos + 8 <= bytes.len() {
        let length = read_u32(bytes, pos + 4).unwrap() as usize;
        let start = pos + 8;
        let end = match start.checked_add(length) {
            Some(end) if end <= bytes.len() => end,
            // A chunk running past the end of the file is what a truncated
            // download looks like. Take whatever is there.
            _ if options.strict => return Err(Error::BadChunkLength(pos)),
            _ => bytes.len(),
        };

        if &bytes[pos..pos + 4] == MTRK {
            tracks.push(start..end);
        } else if options.strict {
            return Err(Error::BadChunkLength(pos));
        }

        pos = end;
    }

    Ok(Parsed {
        kind: FileKind::Smf,
        format,
        division,
        tracks,
    })
}

fn parse_clip(bytes: &[u8], options: Options) -> Result<Parsed> {
    // M2-116-U §3.2.1: the DCTPQ sits at the top of the Clip Configuration
    // Header, ahead of everything but optional Set Profile On messages.
    let ppq = Cursor::clip(bytes, SMF2CLIP.len(), options.strict).find_dctpq();
    let division = match ppq {
        Some(ppq) => Division::Ppq(ppq),
        None if options.strict => {
            return Err(Error::Clip("no Delta Clockstamp Ticks Per Quarter Note"));
        }
        None => Division::Ppq(96),
    };

    // A clip file is one sequence, so it has exactly one range of events.
    let sequence = SMF2CLIP.len()..bytes.len();

    Ok(Parsed {
        kind: FileKind::Clip,
        format: 0,
        division,
        tracks: vec![sequence],
    })
}

#[inline]
fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

#[inline]
fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}
