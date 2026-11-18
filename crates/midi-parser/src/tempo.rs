use crate::{DEFAULT_TEMPO, Division};

#[derive(Debug, Clone)]
pub struct TempoClock {
    ticks_per_quarter: u64,
    micros_per_quarter: u64,
    rate: u64,
    fixed_tick_rate: Option<u64>,
    remainder: u64,
}

impl TempoClock {
    pub fn new(division: Division, units_per_second: u64) -> Self {
        let (ticks_per_quarter, fixed_tick_rate) = match division {
            Division::Ppq(ppq) => (u64::from(ppq).max(1), None),
            Division::Smpte { fps, subframes } => {
                let rate = u64::from(fps) * u64::from(subframes);
                (1, Some(rate.max(1)))
            }
        };

        Self {
            ticks_per_quarter,
            micros_per_quarter: u64::from(DEFAULT_TEMPO),
            rate: units_per_second,
            fixed_tick_rate,
            remainder: 0,
        }
    }

    pub fn set_tempo(&mut self, micros_per_quarter: u32) {
        if self.fixed_tick_rate.is_none() && micros_per_quarter > 0 {
            self.micros_per_quarter = u64::from(micros_per_quarter);
        }
    }

    pub fn advance(&mut self, ticks: u64) -> u64 {
        if ticks == 0 {
            return 0;
        }

        let (numerator, denominator) = self.ratio(ticks);
        let total = numerator + u128::from(self.remainder);

        self.remainder = (total % denominator) as u64;
        (total / denominator) as u64
    }

    fn ratio(&self, ticks: u64) -> (u128, u128) {
        match self.fixed_tick_rate {
            Some(rate) => (u128::from(ticks) * u128::from(self.rate), u128::from(rate)),
            None => (
                u128::from(ticks) * u128::from(self.micros_per_quarter) * u128::from(self.rate),
                u128::from(self.ticks_per_quarter) * 1_000_000,
            ),
        }
    }
}

/// Every tempo change laid out up front, so any tick converts on its own
/// without walking the events before it. Agrees exactly with [`TempoClock`].
#[derive(Debug, Clone)]
pub struct TempoMap {
    /// (first tick, numerator reached at it, numerator per tick) per tempo.
    segments: Vec<(u64, u128, u128)>,
    denominator: u128,
}

impl TempoMap {
    /// `tempos` is (tick, microseconds per quarter), sorted by tick.
    pub fn new(division: Division, tempos: &[(u64, u32)], units_per_second: u64) -> Self {
        let rate = u128::from(units_per_second);
        let (denominator, per_tick) = match division {
            Division::Ppq(ppq) => (u128::from(ppq.max(1)) * 1_000_000, rate * u128::from(DEFAULT_TEMPO)),
            Division::Smpte { fps, subframes } => ((u128::from(fps) * u128::from(subframes)).max(1), rate),
        };

        let mut segments = vec![(0, 0, per_tick)];
        if division.ppq().is_some() {
            for &(tick, micros) in tempos.iter().filter(|(_, micros)| *micros > 0) {
                let (start, reached, per_tick) = *segments.last().unwrap();
                let next = (tick, reached + u128::from(tick - start) * per_tick, rate * u128::from(micros));
                if tick == start {
                    *segments.last_mut().unwrap() = next;
                } else {
                    segments.push(next);
                }
            }
        }

        Self { segments, denominator }
    }

    pub fn at(&self, tick: u64) -> u64 {
        self.position(self.segments.partition_point(|s| s.0 <= tick) - 1, tick)
    }

    /// [`TempoMap::at`] for ticks read in order, resuming from `hint`.
    pub fn at_from(&self, tick: u64, hint: &mut usize) -> u64 {
        if self.segments[*hint].0 > tick {
            *hint = 0;
        }
        while self.segments.get(*hint + 1).is_some_and(|s| s.0 <= tick) {
            *hint += 1;
        }
        self.position(*hint, tick)
    }

    /// The first tick in `from..=to` that lands at `units` or later, given `to` does.
    pub fn first_tick_at(&self, units: u64, mut from: u64, mut to: u64) -> u64 {
        while from < to {
            let mid = from + (to - from) / 2;
            if self.at(mid) >= units {
                to = mid;
            } else {
                from = mid + 1;
            }
        }
        from
    }

    fn position(&self, index: usize, tick: u64) -> u64 {
        let (start, reached, per_tick) = self.segments[index];
        ((reached + u128::from(tick - start) * per_tick) / self.denominator) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u64 = 48_000;

    #[test]
    fn tempo_map_agrees_with_the_clock() {
        let tempos = [(0, 400_000), (7, 0), (100, 250_000), (100, 333_333), (1_000, 1_234_567)];
        let map = TempoMap::new(Division::Ppq(480), &tempos, RATE);

        let mut clock = TempoClock::new(Division::Ppq(480), RATE);
        let (mut frames, mut hint) = (0, 0);
        for tick in 0..3_000u64 {
            if let Some(&(_, micros)) = tempos.iter().rev().find(|(at, _)| *at == tick) {
                clock.set_tempo(micros);
            }
            assert_eq!(map.at(tick), frames, "tick {tick}");
            assert_eq!(map.at_from(tick, &mut hint), frames, "tick {tick}");
            frames += clock.advance(1);
        }

        for units in [0, 1, 5_000, 50_000, frames] {
            let tick = map.first_tick_at(units, 0, 3_000);
            assert!(map.at(tick) >= units && (tick == 0 || map.at(tick - 1) < units));
        }
    }

    #[test]
    fn a_quarter_note_at_120_bpm_is_half_a_second() {
        let mut clock = TempoClock::new(Division::Ppq(96), RATE);
        assert_eq!(clock.advance(96), 24_000);
    }

    #[test]
    fn a_tempo_change_applies_from_where_it_lands() {
        let mut clock = TempoClock::new(Division::Ppq(96), RATE);
        assert_eq!(clock.advance(96), 24_000);

        clock.set_tempo(250_000);
        assert_eq!(clock.advance(96), 12_000);
    }

    #[test]
    fn ticks_that_do_not_divide_evenly_do_not_drift() {
        // 7 ticks at 480 ppq is 1/68.57th of a quarter note, so every single
        // step loses a fraction of a sample.
        let mut clock = TempoClock::new(Division::Ppq(480), RATE);
        let steps = 100_000u64;
        let total: u64 = (0..steps).map(|_| clock.advance(7)).sum();

        let mut reference = TempoClock::new(Division::Ppq(480), RATE);
        assert_eq!(total, reference.advance(steps * 7));
    }

    #[test]
    fn smpte_timing_ignores_tempo() {
        let mut clock = TempoClock::new(
            Division::Smpte {
                fps: 25,
                subframes: 40,
            },
            RATE,
        );

        clock.set_tempo(250_000);
        assert_eq!(clock.advance(1000), 48_000);
    }
}
