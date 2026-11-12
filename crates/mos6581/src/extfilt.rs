//! The C64's output stage: a low-pass and a high-pass STC network between the
//! SID's pin and the line out. The buffer transistor is not modelled.

use crate::chip_data;

/// Low-pass pole, rad/s, from the C64 audio output stage: R = 10 kΩ with C = 1 nF
/// (board 250466, R30/C54).
const W0_LOW: f64 = chip_data::EXTFILT_W0_LOW;
/// High-pass pole, rad/s: R = 1 kΩ with the 10 µF coupling capacitor (C55).
const W0_HIGH: f64 = chip_data::EXTFILT_W0_HIGH;
/// The poles above are in rad/s and the chip is clocked at roughly 1e6 Hz, so
/// one cycle advances the network by 1e-6 seconds.
const DT: f64 = 1.0e-6;

#[derive(Debug, Clone, Copy)]
pub struct ExtFilter {
    low: f64,
    high: f64,
    out: f64,
    enabled: bool,
}

impl Default for ExtFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtFilter {
    pub fn new() -> Self {
        Self {
            low: 0.0,
            high: 0.0,
            out: 0.0,
            enabled: true,
        }
    }

    /// Turning the stage off bypasses the low-pass only. The high-pass keeps
    /// running, because it is what removes any steady offset the chip hands
    /// over (the voice and mixer DC terms from `chip_data`); nothing upstream
    /// removes it.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    pub fn clock(&mut self, input: f64) {
        let after_low = if self.enabled {
            self.low += W0_LOW * (input - self.low) * DT;
            self.low
        } else {
            input
        };
        self.high += W0_HIGH * (after_low - self.high) * DT;
        self.out = after_low - self.high;
    }

    #[inline]
    pub fn output(&self) -> f64 {
        self.out
    }

    pub fn reset(&mut self) {
        self.low = 0.0;
        self.high = 0.0;
        self.out = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOCK: f64 = 985_248.0;

    fn settle(input: f64, cycles: usize) -> f64 {
        let mut f = ExtFilter::new();
        for _ in 0..cycles {
            f.clock(input);
        }
        f.output()
    }

    #[test]
    fn a_dc_input_is_blocked() {
        assert!(
            settle(0.5, 400_000).abs() < 1e-3,
            "DC passed the output stage"
        );
    }

    #[test]
    fn audio_band_survives() {
        let mut f = ExtFilter::new();
        let mut peak = 0.0f64;
        for i in 0..200_000 {
            let s = (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / CLOCK).sin();
            f.clock(s);
            if i > 100_000 {
                peak = peak.max(f.output().abs());
            }
        }
        assert!(peak > 0.7, "1 kHz was attenuated to {peak}");
    }

    #[test]
    fn very_high_frequency_is_rolled_off() {
        let mut f = ExtFilter::new();
        let mut peak = 0.0f64;
        for i in 0..200_000 {
            let s = (2.0 * std::f64::consts::PI * 300_000.0 * i as f64 / CLOCK).sin();
            f.clock(s);
            if i > 100_000 {
                peak = peak.max(f.output().abs());
            }
        }
        assert!(peak < 0.5, "300 kHz came through at {peak}");
    }

    #[test]
    fn disabling_it_still_removes_the_dc_offset() {
        let mut f = ExtFilter::new();
        f.set_enabled(false);
        for _ in 0..400_000 {
            f.clock(0.5);
        }
        assert!(f.output().abs() < 1e-3, "bypass left DC at {}", f.output());
    }

    #[test]
    fn disabling_it_still_passes_audio() {
        let mut f = ExtFilter::new();
        f.set_enabled(false);
        let mut peak = 0.0f64;
        for i in 0..200_000 {
            let s = (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / CLOCK).sin();
            f.clock(s);
            if i > 100_000 {
                peak = peak.max(f.output().abs());
            }
        }
        assert!(peak > 0.7, "bypassed stage killed 1 kHz: {peak}");
    }

    #[test]
    fn the_stage_removes_the_dc_a_quiet_chip_sits_on() {
        // An arbitrary steady offset under a tone: whatever DC the chip hands
        // this stage, the high-pass must remove it.
        let mut f = ExtFilter::new();
        let mut last = 0.0f64;
        for i in 0..2_000_000 {
            let s = 2.0 + (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / CLOCK).sin();
            f.clock(s);
            last = f.output();
        }
        let centred = last - (2.0 * std::f64::consts::PI * 1000.0 * 1_999_999.0 / CLOCK).sin();
        assert!(
            centred.abs() < 5e-2,
            "the 2.0 DC offset survived: {centred}"
        );
    }

    #[test]
    fn reset_clears_the_stored_state() {
        let mut f = ExtFilter::new();
        for _ in 0..10_000 {
            f.clock(0.25);
        }
        f.reset();
        assert_eq!(f.output(), 0.0);
    }
}
