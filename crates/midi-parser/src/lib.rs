//! A reader and writer for both Standard MIDI Files (RP-001) and MIDI Clip
//! Files (M2-116-U), the MIDI 2.0 format built on the Universal MIDI Packet.
//!
//! Files can be memory-mapped rather than read, so a file far larger than
//! memory costs almost nothing to open, and every track can be merged into one
//! tick-ordered stream so a player never has to reconcile tracks itself.
//!
//! Timing is reported in ticks throughout. Turning ticks into seconds needs a
//! tempo map, which belongs to whatever is doing the playing.
//!
//! ```no_run
//! use midi_parser::MidiFile;
//!
//! let file = MidiFile::open("song.mid")?;
//! for event in file.merged()? {
//!     let event = event?;
//!     println!("{} ticks later: {:?}", event.delta, event.event);
//! }
//! # Ok::<(), midi_parser::Error>(())
//! ```

pub mod convert;
mod cursor;
mod error;
mod event;
mod file;
mod merge;
mod scan;
mod source;
pub mod ump;
mod varlen;
pub mod writer;

pub use cursor::Cursor;
pub use error::{Error, Result};
pub use event::{EventRef, Meta, MidiMessage, meta};
pub use file::{Division, FileKind, MidiFile, Options};
pub use merge::{Merged, MergedEvent};
pub use scan::{DEFAULT_TEMPO, MidiInfo};
pub use ump::Ump;
pub use writer::{ClipWriter, SmfWriter};
