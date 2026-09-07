use std::io::Write;

use crate::{
    convert,
    error::{Error, Result},
    event::{EventRef, MidiMessage, meta},
    file::Division,
    ump::{MAX_DELTA_CLOCKSTAMP, Ump, stream, utility},
    varlen,
};

/// Writes a Standard MIDI File, RP-001.
///
/// The number of tracks goes in the header, so it has to be known up front.
/// Each track is built up in a reusable buffer and written out whole, which is
/// what lets the chunk length be filled in without seeking back.
pub struct SmfWriter<W: Write> {
    out: W,
    track: Vec<u8>,
    open: bool,
    /// Running status, suppressed when the caller turns it off.
    status: u8,
    compress: bool,
}

impl<W: Write> SmfWriter<W> {
    pub fn new(mut out: W, format: u16, division: Division, tracks: u16) -> Result<Self> {
        let raw = match division {
            Division::Ppq(ppq) => ppq & 0x7FFF,
            Division::Smpte { fps, subframes } => 0x8000 | u16::from(fps) << 8 | u16::from(subframes),
        };

        out.write_all(b"MThd")?;
        out.write_all(&6u32.to_be_bytes())?;
        out.write_all(&format.to_be_bytes())?;
        out.write_all(&tracks.to_be_bytes())?;
        out.write_all(&raw.to_be_bytes())?;

        Ok(Self {
            out,
            track: Vec::new(),
            open: false,
            status: 0,
            compress: true,
        })
    }

    /// Turns off running status compression, so every event carries its own
    /// status byte.
    pub fn with_running_status(mut self, compress: bool) -> Self {
        self.compress = compress;
        self
    }

    pub fn begin_track(&mut self) {
        self.track.clear();
        self.status = 0;
        self.open = true;
    }

    pub fn event(&mut self, delta: u32, event: EventRef<'_>) -> Result<()> {
        if !self.open {
            self.begin_track();
        }

        varlen::write(&mut self.track, delta.min(0x0FFF_FFFF));

        match event {
            EventRef::Midi(message) => {
                if !self.compress || self.status != message.status {
                    self.status = message.status;
                    self.track.push(message.status);
                }
                self.track.push(message.data1 & 0x7F);
                if MidiMessage::data_len(message.status) == 2 {
                    self.track.push(message.data2 & 0x7F);
                }
            }

            EventRef::SysEx(payload) => {
                self.status = 0;
                self.track.push(0xF0);
                // The F7 that terminates the message counts toward the length.
                varlen::write(&mut self.track, payload.len() as u32 + 1);
                self.track.extend_from_slice(payload);
                self.track.push(0xF7);
            }

            EventRef::Escape(payload) => {
                self.status = 0;
                self.track.push(0xF7);
                varlen::write(&mut self.track, payload.len() as u32);
                self.track.extend_from_slice(payload);
            }

            EventRef::Meta(event) => {
                self.status = 0;
                self.track.push(0xFF);
                self.track.push(event.kind);
                varlen::write(&mut self.track, event.data.len() as u32);
                self.track.extend_from_slice(event.data);
            }

            EventRef::Ump(_) => return Err(Error::Unrepresentable),
        }

        Ok(())
    }

    /// Closes the track, adding the End of Track event if the caller did not.
    pub fn end_track(&mut self) -> Result<()> {
        if !self.open {
            return Ok(());
        }

        if !self.track.ends_with(&[0xFF, meta::END_OF_TRACK, 0x00]) {
            self.track.extend_from_slice(&[0x00, 0xFF, meta::END_OF_TRACK, 0x00]);
        }

        self.out.write_all(b"MTrk")?;
        self.out.write_all(&(self.track.len() as u32).to_be_bytes())?;
        self.out.write_all(&self.track)?;

        self.open = false;
        Ok(())
    }

    pub fn finish(mut self) -> Result<W> {
        self.end_track()?;
        self.out.flush()?;
        Ok(self.out)
    }
}

/// Writes a MIDI Clip File, M2-116-U.
///
/// The file header, the opening Delta Clockstamp and the DCTPQ that the spec
/// requires at the top are written on construction; [`start_clip`] and
/// [`end_clip`] bracket the sequence data.
///
/// [`start_clip`]: ClipWriter::start_clip
/// [`end_clip`]: ClipWriter::end_clip
pub struct ClipWriter<W: Write> {
    out: W,
    scratch: Vec<Ump>,
}

impl<W: Write> ClipWriter<W> {
    pub fn new(mut out: W, ticks_per_quarter: u16) -> Result<Self> {
        out.write_all(b"SMF2CLIP")?;

        let mut writer = Self {
            out,
            scratch: Vec::new(),
        };
        // §3.2.1: the DCTPQ takes a Delta Clockstamp of zero ahead of it.
        writer.packet(Ump::delta_clockstamp(0))?;
        writer.packet(Ump::dctpq(ticks_per_quarter))?;
        Ok(writer)
    }

    pub fn start_clip(&mut self, delta: u64) -> Result<()> {
        self.delta(delta)?;
        self.packet(Ump::stream(stream::START_OF_CLIP))
    }

    pub fn end_clip(&mut self, delta: u64) -> Result<()> {
        self.delta(delta)?;
        self.packet(Ump::stream(stream::END_OF_CLIP))
    }

    /// Writes an event `delta` ticks after the previous one, addressed to
    /// `group`. Events with no clip-file equivalent are skipped.
    pub fn event(&mut self, delta: u64, group: u8, event: EventRef<'_>) -> Result<()> {
        let mut packets = std::mem::take(&mut self.scratch);
        convert::event_to_ump(group, event, &mut packets);

        if !packets.is_empty() {
            self.delta(delta)?;
            for packet in &packets {
                self.packet(*packet)?;
            }
        }

        self.scratch = packets;
        Ok(())
    }

    pub fn packet(&mut self, packet: Ump) -> Result<()> {
        for word in packet.words() {
            self.out.write_all(&word.to_be_bytes())?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<W> {
        self.out.flush()?;
        Ok(self.out)
    }

    /// Emits the Delta Clockstamps for `ticks`. §3.2.2 caps a single one at
    /// 0xFFFFF ticks and requires a NOOP to restart the count past that, so a
    /// long gap becomes a chain of full clockstamps and then the remainder.
    fn delta(&mut self, mut ticks: u64) -> Result<()> {
        while ticks > u64::from(MAX_DELTA_CLOCKSTAMP) {
            self.packet(Ump::delta_clockstamp(MAX_DELTA_CLOCKSTAMP))?;
            self.packet(Ump::utility(utility::NOOP, 0))?;
            ticks -= u64::from(MAX_DELTA_CLOCKSTAMP);
        }

        self.packet(Ump::delta_clockstamp(ticks as u32))
    }
}
