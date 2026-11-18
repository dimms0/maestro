use rayon::prelude::*;

use crate::{
    cursor::Cursor,
    error::Result,
    event::EventRef,
    file::{FileKind, MidiFile},
};

type Timed<'a> = (u64, u8, EventRef<'a>);

/// Every track merged in tick order like [`crate::Merged`], but a window at a
/// time and in parallel: tracks decode side by side, each handing its events to
/// a callback with a state of its own, and whatever that makes of them is
/// merged per lane, ties going to the lower track.
pub struct ParMerged<'a, S, T> {
    tracks: Vec<Track<'a, T>>,
    states: Vec<S>,
    lanes: usize,
}

struct Track<'a, T> {
    cursor: Cursor<'a>,
    next: Option<Timed<'a>>,
    offset: u64,
    /// (tick, value) per lane for the current window.
    out: Vec<Vec<(u64, T)>>,
}

/// Where a [`ParMerged::window`] callback sends what it makes of an event.
pub struct Emit<'w, T> {
    out: &'w mut [Vec<(u64, T)>],
    tick: u64,
}

impl<T> Emit<'_, T> {
    #[inline]
    pub fn push(&mut self, lane: usize, value: T) {
        self.out[lane].push((self.tick, value));
    }
}

impl<'a> MidiFile<'a> {
    /// A parallel merge into `lanes` outputs, with `state` building each
    /// track's callback state from its index.
    pub fn merged_par<S, T>(&'a self, lanes: usize, state: impl FnMut(usize) -> S) -> Result<ParMerged<'a, S, T>> {
        let count = self.track_count();

        // Format 2 tracks play one after another, so each starts where the
        // last one's final event lands, as in `merged`.
        let mut offsets = vec![0; count];
        if self.kind() == FileKind::Smf && self.format() == 2 {
            let ends: Vec<u64> = (0..count)
                .into_par_iter()
                .map(|index| self.track(index).unwrap().map_while(Result::ok).last().map_or(0, |e| e.0))
                .collect();
            for index in 1..count {
                offsets[index] = offsets[index - 1] + ends[index - 1];
            }
        }

        let tracks = (0..count)
            .map(|index| {
                let mut cursor = self.track(index).unwrap();
                Ok(Track {
                    next: cursor.next().transpose()?,
                    cursor,
                    offset: offsets[index],
                    out: (0..lanes).map(|_| Vec::new()).collect(),
                })
            })
            .collect::<Result<_>>()?;

        Ok(ParMerged {
            tracks,
            states: (0..count).map(state).collect(),
            lanes,
        })
    }
}

impl<'a, S: Send, T: Copy + Send + Sync> ParMerged<'a, S, T> {
    /// Hands every event before `end` to `visit` and returns each lane's
    /// output in tick order.
    pub fn window(
        &mut self,
        end: u64,
        visit: impl Fn(&mut S, u64, u8, EventRef<'a>, &mut Emit<T>) + Sync,
    ) -> Result<Vec<Vec<T>>> {
        self.tracks
            .par_iter_mut()
            .zip(&mut self.states)
            .try_for_each(|(track, state)| track.decode(end, state, &visit))?;

        let tracks = &self.tracks;
        Ok((0..self.lanes)
            .into_par_iter()
            .map(|lane| {
                let outs: Vec<_> = tracks.iter().map(|track| &track.out[lane][..]).collect();
                merge(&outs)
            })
            .collect())
    }

    pub fn states_mut(&mut self) -> &mut [S] {
        &mut self.states
    }

    pub fn is_done(&self) -> bool {
        self.tracks.iter().all(|track| track.next.is_none())
    }
}

impl<'a, T> Track<'a, T> {
    fn decode<S>(
        &mut self,
        end: u64,
        state: &mut S,
        visit: &impl Fn(&mut S, u64, u8, EventRef<'a>, &mut Emit<T>),
    ) -> Result<()> {
        self.out.iter_mut().for_each(Vec::clear);

        while let Some((tick, group, event)) = self.next {
            let tick = tick + self.offset;
            if tick >= end {
                break;
            }

            let next = self.cursor.next().transpose();
            self.next = next.as_ref().ok().copied().flatten();
            next?;

            visit(state, tick, group, event, &mut Emit { out: &mut self.out, tick });
        }

        Ok(())
    }
}

/// One lane's output from every track, ties going to the lower track.
///
/// Split points sampled across all tracks cut the merge into balanced groups,
/// each found in every track by binary search and merged straight into its own
/// part of the output, so no part of it is left to one thread.
fn merge<T: Copy + Send + Sync>(outs: &[&[(u64, T)]]) -> Vec<T> {
    let total: usize = outs.iter().map(|out| out.len()).sum();
    let groups = (total / 16384).clamp(1, rayon::current_num_threads() * 4);

    let stride = (total / (groups * 16)).max(1);
    let mut samples: Vec<(u64, usize)> = outs
        .iter()
        .enumerate()
        .flat_map(|(index, out)| out.iter().step_by(stride).map(move |&(tick, _)| (tick, index)))
        .collect();
    samples.sort_unstable();

    // A split at (tick, track) takes everything earlier, and at that tick only the lower tracks.
    let splits: Vec<Vec<usize>> = (1..groups)
        .into_par_iter()
        .map(|group| {
            let (tick, track) = samples[group * samples.len() / groups];
            outs.iter()
                .enumerate()
                .map(|(index, out)| out.partition_point(|e| e.0 < tick || (e.0 == tick && index < track)))
                .collect()
        })
        .collect();
    let edges: Vec<Vec<usize>> = std::iter::once(vec![0; outs.len()])
        .chain(splits)
        .chain(std::iter::once(outs.iter().map(|out| out.len()).collect()))
        .collect();

    let mut values = Vec::with_capacity(total);
    let mut rest = values.spare_capacity_mut();
    let mut jobs = Vec::with_capacity(groups);
    for pair in edges.windows(2) {
        let len = pair[0].iter().zip(&pair[1]).map(|(a, b)| b - a).sum();
        let (dest, tail) = rest.split_at_mut(len);
        rest = tail;
        jobs.push((pair, dest));
    }

    jobs.into_par_iter().for_each(|(pair, dest)| {
        let slices: Vec<_> = outs.iter().zip(pair[0].iter().zip(&pair[1])).map(|(out, (&a, &b))| &out[a..b]).collect();
        let Some(lo) = slices.iter().filter_map(|slice| slice.first()).map(|e| e.0).min() else {
            return;
        };
        let hi = slices.iter().filter_map(|slice| slice.last()).map(|e| e.0).max().unwrap();

        // Going through tracks in order keeps ties in track order either way.
        let span = (hi - lo) as usize + 1;
        if span <= dest.len() * 4 {
            let mut at = vec![0usize; span + 1];
            for &(tick, _) in slices.iter().copied().flatten() {
                at[(tick - lo) as usize + 1] += 1;
            }
            for index in 1..at.len() {
                at[index] += at[index - 1];
            }
            for &(tick, value) in slices.iter().copied().flatten() {
                let slot = &mut at[(tick - lo) as usize];
                dest[*slot].write(value);
                *slot += 1;
            }
        } else {
            let mut group: Vec<_> = slices.iter().copied().flatten().copied().collect();
            group.sort_by_key(|&(tick, _)| tick);
            for (slot, (_, value)) in dest.iter_mut().zip(group) {
                slot.write(value);
            }
        }
    });

    // SAFETY: the groups' lengths add up to `total` and each one filled its own slots.
    unsafe { values.set_len(total) };
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smf(format: u8) -> Vec<u8> {
        let tracks: [&[u8]; 3] = [
            &[0x00, 0x90, 60, 1, 0x10, 0x90, 61, 1, 0x00, 0x90, 62, 1, 0x00, 0xFF, 0x2F, 0x00],
            &[0x00, 0xFF, 0x2F, 0x00],
            &[0x00, 0x91, 70, 1, 0x00, 0x91, 71, 1, 0x10, 0x91, 72, 1, 0x05, 0x91, 73, 1, 0x00, 0xFF, 0x2F, 0x00],
        ];
        let mut bytes = b"MThd\0\0\0\x06\0".to_vec();
        bytes.extend([format, 0, 3, 0, 96]);
        for track in tracks {
            bytes.extend(b"MTrk");
            bytes.extend((track.len() as u32).to_be_bytes());
            bytes.extend(track);
        }
        bytes
    }

    #[test]
    fn windows_merge_like_the_sequential_merge() {
        for format in [1, 2] {
            let bytes = smf(format);
            let file = MidiFile::from_slice(&bytes).unwrap();
            let expected: Vec<_> = file
                .merged()
                .unwrap()
                .map(|e| e.unwrap())
                .filter_map(|e| match e.event {
                    EventRef::Midi(m) => Some((e.tick, e.track, m.data1)),
                    _ => None,
                })
                .collect();

            for window in [1, 7, 1000] {
                let mut merged = file.merged_par(1, |index| index as u32).unwrap();
                let mut got: Vec<(u64, u32, u8)> = Vec::new();
                let mut end = 0;
                while !merged.is_done() {
                    end += window;
                    let lanes = merged
                        .window(end, |track, tick, _, event, emit| {
                            if let EventRef::Midi(m) = event {
                                emit.push(0, (tick, *track, m.data1));
                            }
                        })
                        .unwrap();
                    got.extend(&lanes[0]);
                }
                assert_eq!(got, expected, "format {format}, window {window}");
            }
        }
    }

    #[test]
    fn merge_matches_a_stable_sort_across_groups() {
        let mut seed = 1u64;
        let mut next = move |range: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) % range
        };

        // Dense ticks shared by many tracks, then spans too wide to count.
        for step in [3, 1 << 30] {
            let tracks: Vec<Vec<(u64, u32)>> = (0..300u32)
                .map(|track| {
                    let mut tick = 0;
                    (0..next(400) as u32)
                        .map(|index| {
                            tick += next(step);
                            (tick, track << 16 | index)
                        })
                        .collect()
                })
                .collect();
            let outs: Vec<_> = tracks.iter().map(Vec::as_slice).collect();

            let mut expected: Vec<_> = outs.iter().copied().flatten().copied().collect();
            expected.sort_by_key(|&(tick, _)| tick);
            let expected: Vec<_> = expected.into_iter().map(|(_, value)| value).collect();

            assert!(expected.len() > 16384 * 2, "needs several groups");
            assert_eq!(merge(&outs), expected);
        }
    }
}
