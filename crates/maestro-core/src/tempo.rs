use midi_parser::Division;

pub const DEFAULT_TEMPO: u32 = 500_000; // 120bpm

#[derive(Debug, Clone)]
pub(crate) struct TempoClock {
    ticks_per_quarter: u64,
    micros_per_quarter: u64,
    sample_rate: u64,
    fixed_tick_rate: Option<u64>,
    remainder: u64,
}

impl TempoClock {
    pub fn new(division: Division, sample_rate: u32) -> Self {
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
            sample_rate: u64::from(sample_rate),
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
            Some(rate) => (
                u128::from(ticks) * u128::from(self.sample_rate),
                u128::from(rate),
            ),
            None => (
                u128::from(ticks)
                    * u128::from(self.micros_per_quarter)
                    * u128::from(self.sample_rate),
                u128::from(self.ticks_per_quarter) * 1_000_000,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

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
