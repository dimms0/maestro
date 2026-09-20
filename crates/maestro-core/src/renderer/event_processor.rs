use crate::{
    audio_params::AudioParameters,
    event::{MaestroEvent, MaestroTimedEvent, sysex},
    renderer::config::EventProcessorConfig,
};

const SLOTS: usize = 16 * 128;

const NOTE_JITTER_MS: f32 = 5.0;

struct NoteJitter {
    offset: [u32; SLOTS],
    last_out: [u32; SLOTS],

    window: u32,
    channels: u32,

    rng: u64,
}

impl NoteJitter {
    fn new(port: u8, audio_params: &AudioParameters) -> Self {
        Self {
            offset: [0; SLOTS],
            last_out: [0; SLOTS],
            window: (NOTE_JITTER_MS * audio_params.sample_rate as f32 / 1000.0) as u32,
            channels: u16::from(audio_params.channels) as u32,
            rng: 0x6D61_6573_7472_6F00 ^ (port as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15),
        }
    }

    fn slot(channel: u8, key: u8) -> usize {
        (channel as usize & 15) * 128 + (key as usize & 127)
    }

    fn note_on(&mut self, channel: u8, key: u8, pos: u32) -> u32 {
        let slot = Self::slot(channel, key);
        self.offset[slot] = self.draw() * self.channels;

        self.place(slot, pos)
    }

    fn note_off(&mut self, channel: u8, key: u8, pos: u32) -> u32 {
        self.place(Self::slot(channel, key), pos)
    }

    fn place(&mut self, slot: usize, pos: u32) -> u32 {
        let moved = pos.wrapping_add(self.offset[slot]);
        let last = self.last_out[slot];

        // Positions wrap, so the distance between two of them is a wrapping one read as signed
        let placed = if (moved.wrapping_sub(last) as i32) < 0 {
            last
        } else {
            moved
        };

        self.last_out[slot] = placed;

        placed
    }

    fn draw(&mut self) -> u32 {
        // SplitMix64
        self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        let draw = ((z ^ (z >> 31)) >> 32) as u32;

        ((draw as u64 * self.window as u64) >> 32) as u32
    }

    fn reset(&mut self) {
        self.offset.fill(0);
        self.last_out.fill(0);
    }
}

pub struct PortEventProcessor {
    ignored: bool,
    bypassed: bool,

    bypassed_channels: [bool; 16],
    ignored_channels: [bool; 16],

    transpose: i8,
    key_low: u8,
    key_high: u8,
    velocity_threshold: u8,
    ignore_program_change: bool,
    ignore_sysex: bool,

    velocity_table: [u8; 128],

    missed_notes: [u64; SLOTS],

    jitter: Option<NoteJitter>,
}

impl PortEventProcessor {
    pub fn new(port: u8, config: EventProcessorConfig, audio_params: &AudioParameters) -> Self {
        let mut bypassed_channels: [bool; 16] = [false; 16];
        for c in config.bypass_channels {
            if let Some(v) = bypassed_channels.get_mut(c as usize) {
                *v = true;
            }
        }

        let mut ignored_channels: [bool; 16] = [false; 16];
        for c in config.ignore_channels {
            if let Some(v) = ignored_channels.get_mut(c as usize) {
                *v = true;
            }
        }

        let mut velocity_table: [u8; 128] = [0; 128];
        for (i, entry) in velocity_table.iter_mut().enumerate() {
            if config.fixed_velocity > 0 {
                *entry = config.fixed_velocity.min(127);
            } else {
                let norm = (i as f32 / 127.0).powf(config.velocity_curve);
                let m = 127.0 * norm * config.velocity_multiplier;
                *entry = (m.round() as u8).min(127);
            }
        }

        let low = config.key_range_low.min(127);
        let high = config.key_range_high.min(127).max(low);

        let ignored = config.ignore_ports.contains(&port);
        let bypassed = config.bypass_ports.contains(&port);

        Self {
            ignored,
            bypassed,
            bypassed_channels,
            ignored_channels,
            transpose: config.transpose.clamp(-24, 24),
            key_low: low.min(high),
            key_high: low.max(high),
            velocity_threshold: config.velocity_threshold.min(127),
            ignore_program_change: config.ignore_program_change,
            ignore_sysex: config.ignore_sysex,
            velocity_table,
            missed_notes: [0; SLOTS],
            jitter: config
                .note_jitter
                .then(|| NoteJitter::new(port, audio_params)),
        }
    }

    pub fn process(&mut self, event: MaestroTimedEvent) -> Option<MaestroTimedEvent> {
        if self.ignored {
            return None;
        }
        if self.bypassed {
            return Some(event);
        }

        match event.event {
            MaestroEvent::NoteOn { channel, key, vel } => {
                if self.ignored_channels[channel as usize] {
                    return None;
                }

                if self.bypassed_channels[channel as usize] {
                    return Some(event);
                }

                if !self.key_in_range(key) {
                    return None;
                }

                let newkey = key as i16 + self.transpose as i16;
                if !(0..=127).contains(&newkey) {
                    return None;
                }
                let newkey = newkey as u8;
                let newvel = self.velocity_table[vel as usize];

                if newvel >= self.velocity_threshold && newvel > 0 {
                    Some(MaestroTimedEvent {
                        event: MaestroEvent::NoteOn {
                            channel,
                            key: newkey,
                            vel: newvel,
                        },
                        pos: self.jitter_note_on(channel, newkey, event.pos),
                    })
                } else {
                    self.missed_notes[channel as usize * 128 + key as usize] += 1;
                    None
                }
            }

            MaestroEvent::NoteOff { channel, key } => {
                if self.ignored_channels[channel as usize] {
                    return None;
                }

                if self.bypassed_channels[channel as usize] {
                    return Some(MaestroTimedEvent {
                        event: MaestroEvent::NoteOff { channel, key },
                        pos: event.pos,
                    });
                }

                if !self.key_in_range(key) {
                    return None;
                }

                let newkey = key as i16 + self.transpose as i16;
                if !(0..=127).contains(&newkey) {
                    return None;
                }
                let newkey = newkey as u8;

                let missed = &mut self.missed_notes[channel as usize * 128 + key as usize];
                if *missed > 0 {
                    *missed -= 1;
                    None
                } else {
                    Some(MaestroTimedEvent {
                        event: MaestroEvent::NoteOff {
                            channel,
                            key: newkey,
                        },
                        pos: self.jitter_note_off(channel, newkey, event.pos),
                    })
                }
            }

            MaestroEvent::ProgramChange { channel, .. } => {
                if self.ignore_program_change || self.ignored_channels[channel as usize] {
                    None
                } else {
                    Some(event)
                }
            }

            MaestroEvent::ControlChange { channel, .. }
            | MaestroEvent::PitchBendChange { channel, .. }
            | MaestroEvent::ChannelAftertouch { channel, .. }
            | MaestroEvent::PolyphonicAftertouch { channel, .. } => {
                if !self.ignored_channels[channel as usize] {
                    Some(event)
                } else {
                    None
                }
            }

            MaestroEvent::SystemReset => {
                self.reset();
                Some(event)
            }

            MaestroEvent::SystemExclusive { .. } => {
                if self.ignore_sysex {
                    sysex::release(&event.event);
                    None
                } else {
                    Some(event)
                }
            }
        }
    }

    fn key_in_range(&self, key: u8) -> bool {
        (self.key_low..=self.key_high).contains(&key)
    }

    fn jitter_note_on(&mut self, channel: u8, key: u8, pos: u32) -> u32 {
        match &mut self.jitter {
            Some(jitter) => jitter.note_on(channel, key, pos),
            None => pos,
        }
    }

    fn jitter_note_off(&mut self, channel: u8, key: u8, pos: u32) -> u32 {
        match &mut self.jitter {
            Some(jitter) => jitter.note_off(channel, key, pos),
            None => pos,
        }
    }

    pub(super) fn reset(&mut self) {
        self.missed_notes.fill(0);

        if let Some(jitter) = &mut self.jitter {
            jitter.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const CHANNELS: u32 = 2;
    /// 240 frames at 48 kHz, i.e. a key struck 200 times a second: squarely in
    /// the range where the chop grid is heard as a tone.
    const CHOP: u32 = 240;

    fn proc(config: EventProcessorConfig) -> PortEventProcessor {
        PortEventProcessor::new(0, config, &AudioParameters::default())
    }

    fn jitter_cfg(note_jitter: bool) -> EventProcessorConfig {
        EventProcessorConfig {
            bypass_channels: Box::new([]),
            note_jitter,
            ..Default::default()
        }
    }

    /// [`NOTE_JITTER_MS`] in samples, i.e. the widest a note is ever moved.
    fn window() -> u32 {
        (NOTE_JITTER_MS * RATE as f32 / 1000.0) as u32 * CHANNELS
    }

    /// Where the `i`th strike of a run is written, in samples. A second in, so
    /// that the run is not up against position zero and the first strike reads
    /// as the isolated note it is.
    fn strike(i: u32, gap: u32) -> u32 {
        (RATE + i * gap) * CHANNELS
    }

    /// One key struck `strikes` times, `gap` frames apart, each note `length`
    /// frames long — a chopped note as a dense MIDI writes one. Returns the
    /// position each note came out at.
    fn chop(p: &mut PortEventProcessor, strikes: u32, gap: u32, length: u32) -> Vec<(u32, u32)> {
        (0..strikes)
            .map(|i| {
                let pos = strike(i, gap);
                let on = MaestroTimedEvent { pos, ..note_on(60) };
                let off = MaestroTimedEvent {
                    pos: pos + length * CHANNELS,
                    ..note_off(60)
                };

                (
                    p.process(on).expect("a note on was dropped").pos,
                    p.process(off).expect("a note off was dropped").pos,
                )
            })
            .collect()
    }

    fn cfg(transpose: i8) -> EventProcessorConfig {
        EventProcessorConfig {
            bypass_channels: Box::new([]),
            transpose,
            ..Default::default()
        }
    }

    fn note_on(key: u8) -> MaestroTimedEvent {
        MaestroTimedEvent {
            event: MaestroEvent::NoteOn {
                channel: 0,
                key,
                vel: 100,
            },
            pos: 0,
        }
    }

    fn note_off(key: u8) -> MaestroTimedEvent {
        MaestroTimedEvent {
            event: MaestroEvent::NoteOff { channel: 0, key },
            pos: 0,
        }
    }

    #[test]
    fn transpose_past_key_127_drops_note() {
        let mut p = proc(cfg(1));
        // key 127 + 1 == 128, out of MIDI range, so the note must be dropped
        // rather than wrapping around to key 0.
        assert_eq!(p.process(note_on(127)), None);
        assert_eq!(p.process(note_off(127)), None);
    }

    #[test]
    fn transpose_below_key_0_drops_note() {
        let mut p = proc(cfg(-1));
        assert_eq!(p.process(note_on(0)), None);
    }

    #[test]
    fn transpose_within_range_shifts_key() {
        let mut p = proc(cfg(1));
        match p.process(note_on(126)).map(|e| e.event) {
            Some(MaestroEvent::NoteOn { key, .. }) => assert_eq!(key, 127),
            other => panic!("expected transposed NoteOn, got {other:?}"),
        }
    }

    #[test]
    fn bypassed_channel_note_off_is_untransposed() {
        // Channel 9 is bypassed by default; its notes pass through raw, so its
        // note-offs must stay untransposed to match.
        let mut p = proc(EventProcessorConfig {
            transpose: 12,
            ..Default::default()
        });
        let ev = MaestroTimedEvent {
            event: MaestroEvent::NoteOff {
                channel: 9,
                key: 40,
            },
            pos: 0,
        };
        match p.process(ev).map(|e| e.event) {
            Some(MaestroEvent::NoteOff { key, .. }) => assert_eq!(key, 40),
            other => panic!("expected raw NoteOff, got {other:?}"),
        }
    }

    #[test]
    fn note_off_within_range_shifts_key() {
        // With no preceding dropped note, an in-range NoteOff passes through
        // transposed.
        let mut p = proc(cfg(2));
        match p.process(note_off(50)).map(|e| e.event) {
            Some(MaestroEvent::NoteOff { key, .. }) => assert_eq!(key, 52),
            other => panic!("expected transposed NoteOff, got {other:?}"),
        }
    }

    fn range_cfg(low: u8, high: u8) -> EventProcessorConfig {
        EventProcessorConfig {
            bypass_channels: Box::new([]),
            key_range_low: low,
            key_range_high: high,
            ..Default::default()
        }
    }

    #[test]
    fn keys_outside_the_range_are_dropped() {
        let mut p = proc(range_cfg(48, 72));
        assert_eq!(p.process(note_on(47)), None);
        assert_eq!(p.process(note_on(73)), None);
        // Their note-offs fail the same test, so no note is left hanging.
        assert_eq!(p.process(note_off(47)), None);
        assert_eq!(p.process(note_off(73)), None);
    }

    #[test]
    fn keys_on_the_range_bounds_pass() {
        let mut p = proc(range_cfg(48, 72));
        for key in [48, 60, 72] {
            match p.process(note_on(key)).map(|e| e.event) {
                Some(MaestroEvent::NoteOn { key: got, .. }) => assert_eq!(got, key),
                other => panic!("expected NoteOn {key}, got {other:?}"),
            }
            match p.process(note_off(key)).map(|e| e.event) {
                Some(MaestroEvent::NoteOff { key: got, .. }) => assert_eq!(got, key),
                other => panic!("expected NoteOff {key}, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_reversed_range_is_normalized() {
        // Low above high describes the same span rather than muting everything.
        let mut p = proc(range_cfg(72, 48));
        assert!(p.process(note_on(60)).is_some());
        assert_eq!(p.process(note_on(80)), None);
    }

    #[test]
    fn the_key_range_is_applied_before_transposing() {
        let mut p = proc(EventProcessorConfig {
            bypass_channels: Box::new([]),
            transpose: 12,
            key_range_low: 48,
            key_range_high: 72,
            ..Default::default()
        });
        // 72 is in range and transposes out to 84; 40 is out of range even
        // though transposing would have brought it in.
        match p.process(note_on(72)).map(|e| e.event) {
            Some(MaestroEvent::NoteOn { key, .. }) => assert_eq!(key, 84),
            other => panic!("expected transposed NoteOn, got {other:?}"),
        }
        assert_eq!(p.process(note_on(40)), None);
    }

    #[test]
    fn bypassed_channels_ignore_the_key_range() {
        // Channel 9 is bypassed by default, so its notes pass through raw.
        let mut p = proc(EventProcessorConfig {
            key_range_low: 48,
            key_range_high: 72,
            ..Default::default()
        });
        let ev = MaestroTimedEvent {
            event: MaestroEvent::NoteOn {
                channel: 9,
                key: 20,
                vel: 100,
            },
            pos: 0,
        };
        assert!(p.process(ev).is_some());
    }

    #[test]
    fn program_change_is_dropped_only_when_ignored() {
        let program_change = || MaestroTimedEvent {
            event: MaestroEvent::ProgramChange {
                channel: 0,
                program: 42,
            },
            pos: 0,
        };

        let mut pass = proc(cfg(0));
        assert!(pass.process(program_change()).is_some());

        let mut drop = proc(EventProcessorConfig {
            bypass_channels: Box::new([]),
            ignore_program_change: true,
            ..Default::default()
        });
        assert_eq!(drop.process(program_change()), None);
        // Other channel messages are unaffected by the program change filter.
        let cc = MaestroTimedEvent {
            event: MaestroEvent::ControlChange {
                channel: 0,
                param: 7,
                val: 100,
            },
            pos: 0,
        };
        assert!(drop.process(cc).is_some());
    }

    #[test]
    fn a_keys_events_keep_the_order_they_came_in() {
        // The one thing the synths cannot recover from. They place events by
        // the position they carry, so a note off that overtakes its note on
        // leaves the note playing forever, and a note on overtaken by the note
        // off before it is silenced the moment it starts.
        let mut p = proc(jitter_cfg(true));
        let mut last = 0;

        for (on, off) in chop(&mut p, 400, CHOP, 2) {
            assert!(on >= last, "a note on at {on} followed an event at {last}");
            assert!(
                off >= on,
                "a note off at {off} landed before its note on at {on}"
            );
            last = off;
        }
    }

    #[test]
    fn strikes_with_no_room_between_them_are_held_in_order_rather_than_crossing() {
        // Notes nearly as long as the gap between them leave nowhere to move
        // to, so the clamp takes over and holds a strike back to where the one
        // before it ended. A note trimmed by a few frames the synths can take;
        // a crossed pair they cannot.
        let mut p = proc(jitter_cfg(true));
        let mut last = 0;

        for (on, off) in chop(&mut p, 400, CHOP, CHOP - 1) {
            assert!(on >= last, "a note on at {on} followed an event at {last}");
            assert!(
                off >= on,
                "a note off at {off} landed before its note on at {on}"
            );
            last = off;
        }
    }

    #[test]
    fn a_jittered_note_keeps_the_length_it_was_written_with() {
        // The displacement is drawn at the note on and replayed on the note
        // off, so a note moves whole instead of being stretched or clipped.
        let mut p = proc(jitter_cfg(true));
        let length = 2 * CHANNELS;

        for (on, off) in chop(&mut p, 400, CHOP, 2) {
            assert_eq!(off - on, length, "a note came out {} long", off - on);
        }
    }

    #[test]
    fn a_steady_chop_comes_out_unevenly_spaced() {
        // The grid the repeats share is what turns into a tone, so the spacing
        // has to stop being the same every time.
        let mut p = proc(jitter_cfg(true));
        let starts: Vec<u32> = chop(&mut p, 400, CHOP, 2)
            .iter()
            .map(|(on, _)| *on)
            .collect();

        let spacings: Vec<u32> = starts.windows(2).map(|w| w[1] - w[0]).collect();
        let on_the_grid = spacings.iter().filter(|s| **s == CHOP * CHANNELS).count();

        assert!(
            on_the_grid < spacings.len() / 10,
            "{on_the_grid} of {} gaps came out on the original grid",
            spacings.len()
        );
    }

    #[test]
    fn a_note_is_spread_evenly_over_the_window() {
        // An even spread is what nulls the harmonics of a steady chop. Bunched
        // up at either end and the tone survives with a phase shift on it.
        let mut p = proc(jitter_cfg(true));
        let offsets: Vec<u32> = chop(&mut p, 2000, CHOP, 2)
            .iter()
            .enumerate()
            .map(|(i, (on, _))| on - strike(i as u32, CHOP))
            .collect();

        assert!(
            offsets.iter().all(|o| *o < window()),
            "a note was moved further than the window allows"
        );

        let mean = offsets.iter().map(|o| *o as u64).sum::<u64>() / offsets.len() as u64;
        let expected = (window() / 2) as u64;
        assert!(
            mean.abs_diff(expected) < expected / 10,
            "offsets averaged {mean}, not the {expected} an even spread gives"
        );
    }

    #[test]
    fn notes_are_moved_whether_or_not_the_key_repeats() {
        // The grid that turns into a tone is one many notes share, not one key
        // on its own, so the switch does not wait for a key to be struck twice:
        // notes far too far apart to ever retrigger are spread just the same.
        let mut p = proc(jitter_cfg(true));
        let gap = RATE / 2; // half a second, nothing like a chop

        let moved = chop(&mut p, 50, gap, 2)
            .iter()
            .enumerate()
            .filter(|(i, (on, _))| *on != strike(*i as u32, gap))
            .count();

        assert!(moved > 40, "only {moved} of 50 isolated notes were moved");
    }

    #[test]
    fn jitter_switched_off_leaves_every_event_where_it_was() {
        let mut p = proc(jitter_cfg(false));

        for (i, (on, off)) in chop(&mut p, 50, CHOP, 2).iter().enumerate() {
            let pos = strike(i as u32, CHOP);
            assert_eq!(*on, pos);
            assert_eq!(*off, pos + 2 * CHANNELS);
        }
    }

    #[test]
    fn jittered_positions_stay_on_frame_boundaries() {
        // Positions count interleaved samples, and a note landing part way into
        // a frame would only be truncated onto the frame below it anyway.
        let mut p = proc(jitter_cfg(true));

        for (on, off) in chop(&mut p, 400, CHOP, 2) {
            assert_eq!(on % CHANNELS, 0, "a note on landed at {on}");
            assert_eq!(off % CHANNELS, 0, "a note off landed at {off}");
        }
    }

    #[test]
    fn events_other_than_notes_are_not_moved() {
        // Only note starts sit on the chop grid. Moving a controller off it
        // would just decouple it from the notes it was written for.
        let mut p = proc(jitter_cfg(true));
        chop(&mut p, 10, CHOP, 2);

        let cc = MaestroTimedEvent {
            event: MaestroEvent::ControlChange {
                channel: 0,
                param: 64,
                val: 127,
            },
            pos: strike(10, CHOP),
        };

        assert_eq!(p.process(cc), Some(cc));
    }

    #[test]
    fn a_bypassed_channel_is_left_on_its_grid() {
        // Channel 10 is bypassed out of the box, and bypass means the events
        // pass through untouched. Drums are struck, not chopped, so there is
        // nothing there for the jitter to undo anyway.
        let mut p = proc(EventProcessorConfig {
            note_jitter: true,
            ..Default::default()
        });

        for i in 0..50 {
            let pos = strike(i, CHOP);
            let on = MaestroTimedEvent {
                pos,
                event: MaestroEvent::NoteOn {
                    channel: 9,
                    key: 38,
                    vel: 100,
                },
            };

            assert_eq!(p.process(on).map(|e| e.pos), Some(pos));
        }
    }

    #[test]
    fn a_transposed_note_is_paired_up_under_the_key_it_ends_on() {
        // The displacement is keyed by the transposed key, which is the one
        // both halves of the pair carry by the time they leave the processor.
        // Keyed by the incoming one, a note off would pick up a stranger's
        // offset and could be thrown clear of its note on.
        let mut p = proc(EventProcessorConfig {
            bypass_channels: Box::new([]),
            transpose: 12,
            note_jitter: true,
            ..Default::default()
        });

        let length = 2 * CHANNELS;
        let mut last = 0;

        for (on, off) in chop(&mut p, 400, CHOP, 2) {
            assert!(on >= last, "a note on at {on} followed an event at {last}");
            assert_eq!(off - on, length, "a note came out {} long", off - on);
            last = off;
        }
    }

    #[test]
    fn a_reset_stops_holding_events_back() {
        // The furthest position handed out has to go with the reset. A rewind
        // restarts the timeline behind it, and every note of that key would be
        // held at its old position until playback caught up with it.
        let mut p = proc(jitter_cfg(true));
        let played = chop(&mut p, 100, CHOP, 2);
        p.reset();

        let rewound = MaestroTimedEvent {
            pos: 1000 * CHANNELS,
            ..note_on(60)
        };
        let placed = p.process(rewound).expect("a note on was dropped").pos;

        assert!(
            (rewound.pos..rewound.pos + window()).contains(&placed),
            "a note rewound to {} came out at {placed}, up near the {} the reset should have cleared",
            rewound.pos,
            played.last().unwrap().1,
        );
    }
}
