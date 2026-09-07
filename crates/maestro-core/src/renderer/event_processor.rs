use crate::{
    event::{MaestroEvent, MaestroTimedEvent},
    renderer::config::EventProcessorConfig,
};

pub struct PortEventProcessor {
    ignored: bool,
    bypassed: bool,

    bypassed_channels: [bool; 16],
    ignored_channels: [bool; 16],

    fixed_velocity: u8,
    transpose: i8,
    key_low: u8,
    key_high: u8,
    velocity_threshold: u8,
    ignore_program_change: bool,
    ignore_sysex: bool,

    velocity_table: [u8; 128],

    missed_notes: [u64; 16 * 128],
}

impl PortEventProcessor {
    pub fn new(port: u8, config: EventProcessorConfig) -> Self {
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
            let norm = (i as f32 / 127.0).powf(config.velocity_curve);
            let m = 127.0 * norm * config.velocity_multiplier;
            *entry = (m.round() as u8).min(127);
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
            fixed_velocity: config.fixed_velocity.min(127),
            transpose: config.transpose.clamp(-24, 24),
            key_low: low.min(high),
            key_high: low.max(high),
            velocity_threshold: config.velocity_threshold.min(127),
            ignore_program_change: config.ignore_program_change,
            ignore_sysex: config.ignore_sysex,
            velocity_table,
            missed_notes: [0; 16 * 128],
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

                let newvel = if self.fixed_velocity > 0 {
                    self.fixed_velocity
                } else {
                    self.velocity_table[vel as usize]
                };

                if newvel >= self.velocity_threshold && newvel > 0 {
                    Some(MaestroTimedEvent {
                        event: MaestroEvent::NoteOn {
                            channel,
                            key: newkey,
                            vel: newvel,
                        },
                        pos: event.pos,
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
                    // The matching NoteOn was suppressed, so swallow its NoteOff too.
                    *missed -= 1;
                    None
                } else {
                    Some(MaestroTimedEvent {
                        event: MaestroEvent::NoteOff {
                            channel,
                            key: newkey,
                        },
                        pos: event.pos,
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

            MaestroEvent::SystemExclusive(_) => {
                if self.ignore_sysex {
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

    fn reset(&mut self) {
        self.missed_notes.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut p = PortEventProcessor::new(0, cfg(1));
        // key 127 + 1 == 128, out of MIDI range, so the note must be dropped
        // rather than wrapping around to key 0.
        assert_eq!(p.process(note_on(127)), None);
        assert_eq!(p.process(note_off(127)), None);
    }

    #[test]
    fn transpose_below_key_0_drops_note() {
        let mut p = PortEventProcessor::new(0, cfg(-1));
        assert_eq!(p.process(note_on(0)), None);
    }

    #[test]
    fn transpose_within_range_shifts_key() {
        let mut p = PortEventProcessor::new(0, cfg(1));
        match p.process(note_on(126)).map(|e| e.event) {
            Some(MaestroEvent::NoteOn { key, .. }) => assert_eq!(key, 127),
            other => panic!("expected transposed NoteOn, got {other:?}"),
        }
    }

    #[test]
    fn bypassed_channel_note_off_is_untransposed() {
        // Channel 9 is bypassed by default; its notes pass through raw, so its
        // note-offs must stay untransposed to match.
        let mut p = PortEventProcessor::new(
            0,
            EventProcessorConfig {
                transpose: 12,
                ..Default::default()
            },
        );
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
        let mut p = PortEventProcessor::new(0, cfg(2));
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
        let mut p = PortEventProcessor::new(0, range_cfg(48, 72));
        assert_eq!(p.process(note_on(47)), None);
        assert_eq!(p.process(note_on(73)), None);
        // Their note-offs fail the same test, so no note is left hanging.
        assert_eq!(p.process(note_off(47)), None);
        assert_eq!(p.process(note_off(73)), None);
    }

    #[test]
    fn keys_on_the_range_bounds_pass() {
        let mut p = PortEventProcessor::new(0, range_cfg(48, 72));
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
        let mut p = PortEventProcessor::new(0, range_cfg(72, 48));
        assert!(p.process(note_on(60)).is_some());
        assert_eq!(p.process(note_on(80)), None);
    }

    #[test]
    fn the_key_range_is_applied_before_transposing() {
        let mut p = PortEventProcessor::new(
            0,
            EventProcessorConfig {
                bypass_channels: Box::new([]),
                transpose: 12,
                key_range_low: 48,
                key_range_high: 72,
                ..Default::default()
            },
        );
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
        let mut p = PortEventProcessor::new(
            0,
            EventProcessorConfig {
                key_range_low: 48,
                key_range_high: 72,
                ..Default::default()
            },
        );
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

        let mut pass = PortEventProcessor::new(0, cfg(0));
        assert!(pass.process(program_change()).is_some());

        let mut drop = PortEventProcessor::new(
            0,
            EventProcessorConfig {
                bypass_channels: Box::new([]),
                ignore_program_change: true,
                ..Default::default()
            },
        );
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
}
