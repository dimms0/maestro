use std::sync::atomic::{AtomicBool, Ordering};

use crate::{
    error::Result,
    event::EventRef,
    file::{Division, FileKind, MidiFile},
};

/// The default when a file declares no tempo, RP-001 §"Set Tempo": 120 beats
/// per minute.
pub const DEFAULT_TEMPO: u32 = 500_000;

/// What a file holds, without decoding it twice.
///
/// A scan walks every track once and skips event payloads by their declared
/// length, so it costs a fraction of a full read and answers the questions a
/// player needs up front: how long is it, how many notes, which ports.
#[derive(Debug, Clone)]
pub struct MidiInfo {
    pub kind: FileKind,
    pub format: u16,
    pub division: Division,
    pub track_count: usize,
    /// Length of the longest track, in ticks.
    pub total_ticks: u64,
    /// Length in seconds, with every tempo change applied.
    pub duration: f64,
    pub events: u64,
    /// Note On events with a non-zero velocity.
    pub notes: u64,
    /// The port each track's MIDI Port meta event assigns it, or the UMP Group
    /// for a clip file.
    pub track_ports: Vec<u8>,
    /// Bit set of the ports in use.
    pub ports: u32,
    /// Bit set of the MIDI channels in use.
    pub channels: u16,
    pub tempo_changes: usize,
}

#[derive(Default)]
struct TrackScan {
    ticks: u64,
    events: u64,
    notes: u64,
    port: u8,
    channels: u16,
    tempos: Vec<(u64, u32)>,
}

impl<'a> MidiFile<'a> {
    /// Walks the whole file and reports what is in it.
    pub fn scan(&self) -> Result<MidiInfo> {
        self.scan_with(|_| {}, &AtomicBool::new(false))
    }

    /// Scans while reporting each finished track, and gives up early when
    /// `cancel` is set.
    pub fn scan_with(
        &self,
        on_track: impl Fn(usize) + Sync,
        cancel: &AtomicBool,
    ) -> Result<MidiInfo> {
        let indices: Vec<usize> = (0..self.track_count()).collect();

        #[cfg(feature = "parallel")]
        let scans: Result<Vec<TrackScan>> = {
            use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
            indices
                .par_iter()
                .map(|&index| {
                    let scan = self.scan_track(index, cancel);
                    on_track(index);
                    scan
                })
                .collect()
        };

        #[cfg(not(feature = "parallel"))]
        let scans: Result<Vec<TrackScan>> = indices
            .iter()
            .map(|&index| {
                let scan = self.scan_track(index, cancel);
                on_track(index);
                scan
            })
            .collect();

        Ok(self.summarise(scans?))
    }

    fn scan_track(&self, index: usize, cancel: &AtomicBool) -> Result<TrackScan> {
        let mut scan = TrackScan::default();
        let Some(cursor) = self.track(index) else {
            return Ok(scan);
        };

        for (count, event) in cursor.enumerate() {
            // Checking the flag on every event would cost more than the decode
            // itself, and a track is not so long that this is coarse.
            if count % 4096 == 0 && cancel.load(Ordering::Relaxed) {
                break;
            }

            let (tick, group, event) = event?;
            scan.ticks = tick;
            scan.events += 1;

            match event {
                EventRef::Midi(message) => {
                    scan.channels |= 1 << message.channel();
                    if message.kind() == 0x90 && message.data2 > 0 {
                        scan.notes += 1;
                    }
                }

                EventRef::Meta(m) => {
                    if let Some(micros) = m.tempo() {
                        scan.tempos.push((tick, micros));
                    }
                    if let Some(port) = m.port() {
                        scan.port = port;
                    }
                }

                EventRef::Ump(packet) => {
                    scan.port = group;
                    if let Some(micros) = packet.tempo() {
                        scan.tempos.push((tick, micros));
                    }
                }

                EventRef::SysEx(_) | EventRef::Escape(_) => {}
            }
        }

        Ok(scan)
    }

    fn summarise(&self, scans: Vec<TrackScan>) -> MidiInfo {
        let mut info = MidiInfo {
            kind: self.kind(),
            format: self.format(),
            division: self.division(),
            track_count: scans.len(),
            total_ticks: 0,
            duration: 0.0,
            events: 0,
            notes: 0,
            track_ports: Vec::with_capacity(scans.len()),
            ports: 0,
            channels: 0,
            tempo_changes: 0,
        };

        let mut tempos = Vec::new();
        for scan in scans {
            info.total_ticks = info.total_ticks.max(scan.ticks);
            info.events += scan.events;
            info.notes += scan.notes;
            info.channels |= scan.channels;
            info.ports |= 1 << (scan.port & 0x1F);
            info.track_ports.push(scan.port);
            tempos.extend(scan.tempos);
        }

        tempos.sort_unstable_by_key(|(tick, _)| *tick);
        info.tempo_changes = tempos.len();
        info.duration = duration(self.division(), info.total_ticks, &tempos);

        info
    }
}

/// Integrates a tick count into seconds across the tempo changes in `tempos`.
fn duration(division: Division, total_ticks: u64, tempos: &[(u64, u32)]) -> f64 {
    let ppq = match division {
        Division::Ppq(ppq) if ppq > 0 => f64::from(ppq),
        // SMPTE timing runs at a fixed tick rate that tempo does not touch.
        Division::Smpte { fps, subframes } => {
            let rate = f64::from(fps) * f64::from(subframes);
            return if rate > 0.0 {
                total_ticks as f64 / rate
            } else {
                0.0
            };
        }
        Division::Ppq(_) => return 0.0,
    };

    let mut seconds = 0.0;
    let mut last = 0u64;
    let mut tempo = f64::from(DEFAULT_TEMPO);

    for &(tick, micros) in tempos {
        let tick = tick.min(total_ticks);
        seconds += (tick - last) as f64 * tempo / ppq / 1e6;
        last = tick;
        tempo = f64::from(micros);
    }

    seconds + (total_ticks - last) as f64 * tempo / ppq / 1e6
}
