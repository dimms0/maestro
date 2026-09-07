use midi_parser::Division;
use std::sync::{Arc, Mutex};

use crate::{
    audio_params::AudioParameters,
    error::RealtimeEngineError,
    event::{MaestroEvent, MaestroTimedEvent, MidiTranslator},
    realtime::{RealtimeEngineOptions, event_sender::nps::NpsTracker},
    renderer::{RealtimeClock, sender::PortEventSender},
    tempo::TempoClock,
};

mod nps;

pub use crate::event::ump::UMP_GROUPS;

struct TickState {
    tempo: TempoClock,
    pos: u64,
    anchor: Option<u64>,
}

impl TickState {
    fn new(sample_rate: u32) -> Self {
        Self {
            tempo: TempoClock::new(Division::Ppq(96), sample_rate),
            pos: 0,
            anchor: None,
        }
    }
}

impl Clone for RealtimeEventSender {
    fn clone(&self) -> Self {
        Self {
            ports: self.ports.clone(),
            nps: self.nps.clone(),
            precision: self.precision,
            clock: self.clock.clone(),
            translator: MidiTranslator::new(),
            tick: Mutex::new(TickState::new(self.sample_rate)),
            sample_rate: self.sample_rate,
            channels: self.channels,
        }
    }
}

pub struct RealtimeEventSender {
    ports: Box<[Arc<PortEventSender>]>,

    nps: Option<NpsTracker>,
    precision: bool,

    clock: Arc<RealtimeClock>,
    translator: MidiTranslator,
    tick: Mutex<TickState>,
    sample_rate: u32,
    channels: u64,
}

impl RealtimeEventSender {
    pub(super) fn new(
        options: &RealtimeEngineOptions,
        params: &AudioParameters,
        clock: Arc<RealtimeClock>,
        port_senders: Box<[Arc<PortEventSender>]>,
    ) -> Result<Self, RealtimeEngineError> {
        let ports = options.ports.unwrap_or(1);

        let nps = if let Some(nps) = options.config.max_nps {
            Some(
                NpsTracker::new(ports as usize * 16, nps as u64)
                    .map_err(RealtimeEngineError::Thread)?,
            )
        } else {
            None
        };

        let sample_rate = params.sample_rate;

        Ok(Self {
            ports: port_senders,
            nps,
            precision: options.config.precision_playback,
            clock,
            translator: MidiTranslator::new(),
            tick: Mutex::new(TickState::new(sample_rate)),
            sample_rate,
            channels: u16::from(params.channels) as u64,
        })
    }

    fn filter(&self, port: u8, event: MaestroEvent) -> Option<MaestroEvent> {
        if let Some(nps) = &self.nps {
            let tracked = |channel: u8| port as usize * 16 + channel as usize;

            match event {
                MaestroEvent::NoteOn { channel, key, vel }
                    if !nps.note_on(tracked(channel), key, vel) =>
                {
                    return None;
                }
                MaestroEvent::NoteOff { channel, key } if !nps.note_off(tracked(channel), key) => {
                    return None;
                }

                _ => {}
            }
        }

        Some(event)
    }

    fn push(&self, port: u8, pos: u64, event: MaestroEvent) {
        if let Some(ev) = self.filter(port, event) {
            self.ports[port as usize].send(MaestroTimedEvent { event: ev, pos });
        }
    }

    fn stamp(&self) -> u64 {
        if self.precision {
            self.clock.get_position()
        } else {
            0
        }
    }

    fn tick_stamp(&self, ticks: u64) -> u64 {
        let mut tick = self.tick.lock().unwrap();
        let anchor = *tick.anchor.get_or_insert_with(|| self.clock.get_position());

        let samples = tick.tempo.advance(ticks);
        tick.pos += samples;

        anchor + tick.pos * self.channels
    }

    pub fn set_tick_division(&self, division: Division) {
        *self.tick.lock().unwrap() = TickState {
            tempo: TempoClock::new(division, self.sample_rate),
            pos: 0,
            anchor: None,
        };
    }

    pub fn set_tick_tempo(&self, micros_per_quarter: u32) {
        self.tick
            .lock()
            .unwrap()
            .tempo
            .set_tempo(micros_per_quarter);
    }

    pub fn advance_ticks(&self, ticks: u64) {
        self.tick_stamp(ticks);
    }

    pub fn process_short(&self, port: u8, short: u32, delta_ticks: Option<u64>) {
        let pos = if let Some(ticks) = delta_ticks {
            self.tick_stamp(ticks)
        } else {
            self.stamp()
        };

        self.translator.short(port, short, |event| {
            self.push(port, pos, event);
        });
    }

    pub fn process_long(&self, port: u8, long: &[u8], delta_ticks: Option<u64>) {
        let pos = if let Some(ticks) = delta_ticks {
            self.tick_stamp(ticks)
        } else {
            self.stamp()
        };

        self.translator.long(port, long, |event| {
            self.push(port, pos, event);
        });
    }

    pub fn process_ump(&self, words: &[u32], delta_ticks: Option<u64>) {
        let pos = if let Some(ticks) = delta_ticks {
            self.tick_stamp(ticks)
        } else {
            self.stamp()
        };

        self.translator.ump(words, |group, event| {
            self.push(group, pos, event);
        });
    }

    pub fn reset(&self) {
        for port in &self.ports {
            port.reset();
        }

        self.translator.reset();
        if let Some(nps) = &self.nps {
            nps.reset();
        }

        let mut tick = self.tick.lock().unwrap();
        tick.pos = 0;
        tick.anchor = None;
    }
}

// #[cfg(test)]
// mod tests {
//     use std::thread;

//     use super::*;
//     use crate::{
//         audio_params::{AudioParameters, ChannelCount},
//         realtime::config::RealtimeConfig,
//     };

//     fn sender_with(precision: bool) -> RealtimeEventSender {
//         RealtimeEventSender::new(
//             &RealtimeEngineOptions {
//                 config: RealtimeConfig {
//                     max_nps: None,
//                     precision_playback: precision,
//                     ..Default::default()
//                 },
//                 ..Default::default()
//             },
//             &AudioParameters::default(),
//             Arc::new(RealtimeClock::new(&AudioParameters::default())),
//         )
//         .unwrap()
//     }

//     fn sender() -> RealtimeEventSender {
//         sender_with(false)
//     }

//     /// A sender that asked for 44.1 kHz and was handed `params` by the device,
//     /// which is what the engine does whenever the two disagree.
//     fn sender_on(params: AudioParameters) -> RealtimeEventSender {
//         RealtimeEventSender::new(
//             &RealtimeEngineOptions {
//                 config: RealtimeConfig {
//                     max_nps: None,
//                     ..Default::default()
//                 },
//                 audio_params: AudioParameters {
//                     channels: ChannelCount::Stereo,
//                     sample_rate: 44_100,
//                 },
//                 ..Default::default()
//             },
//             &params,
//             Arc::new(RealtimeClock::new(&params)),
//         )
//         .unwrap()
//     }

//     /// How far apart two tick-stamped events ended up.
//     fn tick_spacing(sender: &RealtimeEventSender) -> u64 {
//         let events = queued(sender);
//         assert_eq!(events.len(), 2);
//         events[1].pos - events[0].pos
//     }

//     fn queued(sender: &RealtimeEventSender) -> Vec<QueuedEvent> {
//         let mut events = Vec::new();

//         while let Ok(event) = sender.shared.queue_rcv.try_recv() {
//             events.push(event);
//         }

//         events
//     }

//     fn drain(sender: &RealtimeEventSender) -> Vec<(u8, MaestroEvent)> {
//         queued(sender)
//             .into_iter()
//             .map(|QueuedEvent { port, event, .. }| (port, event))
//             .collect()
//     }

//     fn note_on(key: u8) -> u32 {
//         0x90 | (key as u32) << 8 | 100 << 16
//     }

//     #[test]
//     fn events_reach_the_queue_in_the_order_they_were_sent() {
//         let sender = sender();

//         for key in 0..16 {
//             sender.process_short(1, note_on(key), None);
//         }

//         let events = drain(&sender);
//         assert_eq!(events.len(), 16);

//         for (key, (port, event)) in events.into_iter().enumerate() {
//             assert_eq!(port, 1);
//             assert_eq!(
//                 event,
//                 MaestroEvent::NoteOn {
//                     channel: 0,
//                     key: key as u8,
//                     vel: 100
//                 }
//             );
//         }
//     }

//     #[test]
//     fn a_long_message_is_split_into_the_events_it_holds() {
//         let sender = sender();

//         // One status byte and two note pairs, the second on running status.
//         sender.process_long(0, &[0x90, 60, 100, 62, 110], None);

//         let events = drain(&sender);
//         assert_eq!(events.len(), 2);
//         assert_eq!(
//             events[1].1,
//             MaestroEvent::NoteOn {
//                 channel: 0,
//                 key: 62,
//                 vel: 110
//             }
//         );
//     }

//     #[test]
//     fn threads_can_send_through_one_sender_at_once() {
//         let sender = sender();

//         thread::scope(|s| {
//             for _ in 0..4 {
//                 s.spawn(|| {
//                     for key in 0..64 {
//                         sender.process_short(0, note_on(key), None);
//                     }
//                 });
//             }
//         });

//         assert_eq!(sender.dropped_events(), 0);
//         assert_eq!(drain(&sender).len(), 4 * 64);
//     }

//     #[test]
//     fn an_event_is_stamped_with_the_moment_it_was_sent() {
//         let sender = sender_with(true);
//         let params = AudioParameters::default();
//         let rate = params.sample_rate as u64;
//         let channels = u16::from(params.channels) as u64;

//         // A player sending two events 20 ms apart. Whenever the dispatcher gets
//         // around to draining them, they have to stay 20 ms apart.
//         sender.process_short(0, note_on(60), None);
//         let sent = Instant::now();
//         thread::sleep(Duration::from_millis(20));
//         let gap = sent.elapsed();
//         sender.process_short(0, note_on(62), None);

//         let events = queued(&sender);
//         assert_eq!(events.len(), 2);

//         let spacing = events[1].pos - events[0].pos;
//         let expected = gap.as_micros() as u64 * rate * channels / 1_000_000;
//         let tolerance = 3 * rate * channels / 1000; // 3 ms
//         assert!(
//             spacing.abs_diff(expected) < tolerance,
//             "events sent {expected} samples apart were stamped {spacing} apart"
//         );
//     }

//     #[test]
//     fn the_events_of_one_message_share_a_stamp() {
//         let sender = sender_with(true);

//         // Two note pairs in a single packet, the second on running status. They
//         // were sent together, so nothing may put them at different positions.
//         sender.process_long(0, &[0x90, 60, 100, 62, 110], None);

//         let events = queued(&sender);
//         assert_eq!(events.len(), 2);
//         assert_eq!(events[0].pos, events[1].pos);
//     }

//     #[test]
//     fn without_precision_playback_events_carry_no_position() {
//         let sender = sender();

//         sender.process_short(0, note_on(60), None);
//         thread::sleep(Duration::from_millis(5));
//         sender.process_short(0, note_on(62), None);

//         assert!(queued(&sender).iter().all(|e| e.pos == 0));
//     }

//     #[test]
//     fn a_tick_stamp_is_measured_against_the_stream_that_was_opened() {
//         // Asked for 44.1 kHz, given a 48 kHz device. A tick stamp is a position
//         // in the audio the renderer produces, so a quarter note at 120 bpm has
//         // to come out as 24000 of that device's frames, not 22050 of the ones
//         // that were asked for.
//         let params = AudioParameters {
//             channels: ChannelCount::Stereo,
//             sample_rate: 48_000,
//         };
//         let sender = sender_on(params);
//         sender.set_tick_division(Division::Ppq(96));

//         sender.process_short(0, note_on(60), Some(0));
//         sender.process_short(0, note_on(62), Some(96));

//         assert_eq!(tick_spacing(&sender), 24_000 * 2);
//     }

//     #[test]
//     fn ticks_with_no_event_to_stamp_still_move_the_clock() {
//         let sender = sender_on(AudioParameters::default());
//         sender.set_tick_division(Division::Ppq(96));

//         // A player that has a batch to skip over — a tempo change, a track
//         // name — cannot lose the ticks it carried.
//         sender.process_short(0, note_on(60), Some(0));
//         sender.advance_ticks(48);
//         sender.advance_ticks(48);
//         sender.process_short(0, note_on(62), Some(0));

//         assert_eq!(tick_spacing(&sender), 24_000 * 2);
//     }

//     #[test]
//     fn a_tempo_change_only_applies_to_the_ticks_after_it() {
//         let sender = sender_on(AudioParameters::default());
//         sender.set_tick_division(Division::Ppq(96));

//         sender.process_short(0, note_on(60), Some(0));

//         // A quarter note at the default 120 bpm, then the tempo doubles and
//         // another quarter note follows. The ticks paid before the change keep
//         // the tempo they elapsed under.
//         sender.advance_ticks(96);
//         sender.set_tick_tempo(250_000);
//         sender.process_short(0, note_on(62), Some(96));

//         assert_eq!(tick_spacing(&sender), (24_000 + 12_000) * 2);
//     }

//     #[test]
//     fn the_note_limiter_leaves_the_events_it_lets_through_alone() {
//         let shared = sender().shared;
//         let event = MaestroEvent::NoteOn {
//             channel: 3,
//             key: 60,
//             vel: 100,
//         };

//         assert_eq!(shared.filter(0, event.clone()), Some(event));
//     }
// }
