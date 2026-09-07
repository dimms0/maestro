use std::{cmp::Reverse, collections::BinaryHeap};

use crate::{cursor::Cursor, error::Result, event::EventRef, file::MidiFile};

/// One event out of the merged stream.
#[derive(Debug, Clone, Copy)]
pub struct MergedEvent<'a> {
    /// Ticks since the previous event in the merged stream.
    pub delta: u64,
    /// Absolute tick, counted from the start of the file.
    pub tick: u64,
    /// The track it came from, which is what a MIDI Port meta event applies to.
    pub track: u32,
    /// The UMP Group, for clip files. Always 0 for an SMF, where routing comes
    /// from the track's MIDI Port meta event instead.
    pub group: u8,
    pub event: EventRef<'a>,
}

/// Every track of a file merged into one stream, in tick order.
///
/// This is what makes timing in a renderer simple: the tracks of a format 1
/// file arrive as if they had been written as a single one. Ties are broken by
/// track number, so a given file always merges the same way.
pub struct Merged<'a> {
    cursors: Vec<Cursor<'a>>,
    pending: Vec<Option<(u8, EventRef<'a>)>>,
    /// (tick, track) of every track still holding an event, earliest first.
    queue: BinaryHeap<Reverse<(u64, u32)>>,
    last_tick: u64,
    failed: bool,
}

impl<'a> Merged<'a> {
    pub(crate) fn new(file: &'a MidiFile<'a>) -> Result<Self> {
        let count = file.track_count();
        let mut merged = Self {
            cursors: (0..count).map(|index| file.track(index).unwrap()).collect(),
            pending: vec![None; count],
            queue: BinaryHeap::with_capacity(count),
            last_tick: 0,
            failed: false,
        };

        for track in 0..count as u32 {
            merged.advance(track)?;
        }

        Ok(merged)
    }

    /// Fills `out` with every event sharing the next tick, track and Group,
    /// and returns how far that tick is from the previous batch along with the
    /// track and Group they all belong to.
    ///
    /// Batching by more than the tick is what keeps port routing intact: a port
    /// follows from the track in an SMF and from the Group in a clip file, so a
    /// batch that mixed either would have no single port to send to. Reusing
    /// `out` across calls keeps the merge free of per-batch allocation.
    pub fn next_batch(&mut self, out: &mut Vec<EventRef<'a>>) -> Option<Result<(u64, u32, u8)>> {
        out.clear();

        let first = match self.next()? {
            Ok(event) => event,
            Err(error) => return Some(Err(error)),
        };
        out.push(first.event);

        while self.queue.peek() == Some(&Reverse((first.tick, first.track)))
            && self.pending[first.track as usize].is_some_and(|(group, _)| group == first.group)
        {
            match self.next()? {
                Ok(event) => out.push(event.event),
                Err(error) => return Some(Err(error)),
            }
        }

        Some(Ok((first.delta, first.track, first.group)))
    }

    fn advance(&mut self, track: u32) -> Result<()> {
        let event = match self.cursors[track as usize].next_event() {
            Some(Ok((tick, group, event))) => {
                self.queue.push(Reverse((tick, track)));
                Some((group, event))
            }
            Some(Err(error)) => return Err(error),
            None => None,
        };

        self.pending[track as usize] = event;
        Ok(())
    }
}

impl<'a> Iterator for Merged<'a> {
    type Item = Result<MergedEvent<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }

        let Reverse((tick, track)) = self.queue.pop()?;
        let (group, event) = self.pending[track as usize].take()?;

        if let Err(error) = self.advance(track) {
            self.failed = true;
            return Some(Err(error));
        }

        let delta = tick - self.last_tick;
        self.last_tick = tick;

        Some(Ok(MergedEvent {
            delta,
            tick,
            track,
            group,
            event,
        }))
    }
}

impl<'a> MidiFile<'a> {
    /// Every track merged into one tick-ordered stream.
    pub fn merged(&'a self) -> Result<Merged<'a>> {
        Merged::new(self)
    }
}
