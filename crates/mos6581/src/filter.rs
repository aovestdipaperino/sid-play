//! The SID's filter: a two-integrator state-variable biquad, 12 dB per octave,
//! with the mixer DC and the routing rules the hardware has.

use crate::Model;
use crate::chip_data;
use crate::tables::cutoff_hz;

pub const ROUTE_VOICE_1: u8 = 1 << 0;
pub const ROUTE_VOICE_2: u8 = 1 << 1;
pub const ROUTE_VOICE_3: u8 = 1 << 2;
pub const ROUTE_EXT_IN: u8 = 1 << 3;

pub const TAP_LOW_PASS: u8 = 1 << 0;
pub const TAP_BAND_PASS: u8 = 1 << 1;
pub const TAP_HIGH_PASS: u8 = 1 << 2;
/// No tap enabled: every routed voice is muted, which is what the mode bits
/// mean on hardware.
pub const TAP_NONE: u8 = 0;

/// The mixer's own DC offset, in the same normalised domain as the voices
/// (`voice::FULL_SCALE`); value from `chip_data`.
const MIXER_DC: f64 = chip_data::MIXER_DC;

pub struct Filter {
    band: f64,
    low: f64,
    /// 2*pi*f0/clock, per cycle.
    w0: f64,
    inv_q: f64,
    routed: u8,
    taps: u8,
    voice3_off: bool,
    ext_in: f64,
    volume: f64,
    out: f64,
    model: Model,
}

impl Filter {
    pub fn new(model: Model) -> Self {
        Self {
            band: 0.0,
            low: 0.0,
            w0: 0.0,
            inv_q: 1.0 / chip_data::FILTER_Q_MIN,
            routed: 0,
            taps: TAP_LOW_PASS,
            voice3_off: false,
            ext_in: 0.0,
            volume: 0.0,
            out: 0.0,
            model,
        }
    }

    pub fn set_fc(&mut self, fc: u16, clock_hz: f64) {
        let hz = cutoff_hz(self.model, fc).min(clock_hz / 16.0);
        self.w0 = 2.0 * std::f64::consts::PI * hz / clock_hz;
    }

    /// Resonance nibble to the damping coefficient the difference equation
    /// needs, which is `1/Q` rather than `Q`: Q runs from its minimum at no resonance
    /// to its maximum at full, so the coefficient shrinks as resonance is added.
    /// Inverting this makes the filter *quieter* as resonance increases.
    pub fn set_resonance(&mut self, res: u8) {
        self.inv_q =
            1.0 / (chip_data::FILTER_Q_MIN + f64::from(res & 0x0f) / chip_data::FILTER_Q_STEPS);
    }

    #[inline]
    pub fn set_mode(&mut self, routed: u8, taps: u8) {
        self.routed = routed & 0x0f;
        self.taps = taps & 0x07;
    }

    #[inline]
    pub fn set_voice3_off(&mut self, off: bool) {
        self.voice3_off = off;
    }

    #[inline]
    pub fn set_ext_in(&mut self, sample: f64) {
        self.ext_in = sample;
    }

    #[inline]
    pub fn set_volume(&mut self, vol: u8) {
        self.volume = f64::from(vol & 0x0f) / 15.0;
    }

    #[inline]
    pub fn is_routed(&self, route_bit: u8) -> bool {
        self.routed & route_bit != 0
    }

    /// One Φ2 cycle with the three normalised voice outputs.
    pub fn clock(&mut self, v1: f64, v2: f64, v3: f64) {
        // Voice 3 is only killed by voice3off when it is not going through the
        // filter; routed, it still sounds.
        let v3 = if self.voice3_off && !self.is_routed(ROUTE_VOICE_3) {
            0.0
        } else {
            v3
        };

        let mut input = 0.0;
        if self.is_routed(ROUTE_VOICE_1) {
            input += v1;
        }
        if self.is_routed(ROUTE_VOICE_2) {
            input += v2;
        }
        if self.is_routed(ROUTE_VOICE_3) {
            input += v3;
        }
        if self.is_routed(ROUTE_EXT_IN) {
            input += self.ext_in;
        }

        // Both integrators subtract. With either sign flipped the loop is
        // unstable and the output diverges, so the pair is load-bearing rather
        // than stylistic. The two increments are computed from the pre-update
        // state, matching two coupled integrators.
        //
        // The loop is stable while w0 < 1/Q; the clamp in `set_fc` keeps w0
        // under 0.4, inside the smallest 1/Q this filter ever uses.
        let high = self.band * self.inv_q - self.low - input;
        let d_band = -self.w0 * high;
        let d_low = -self.w0 * self.band;
        self.band += d_band;
        self.low += d_low;

        let mut filtered = 0.0;
        if self.taps & TAP_LOW_PASS != 0 {
            filtered += self.low;
        }
        if self.taps & TAP_BAND_PASS != 0 {
            filtered += self.band;
        }
        if self.taps & TAP_HIGH_PASS != 0 {
            filtered += high;
        }

        let mut unfiltered = 0.0;
        if !self.is_routed(ROUTE_VOICE_1) {
            unfiltered += v1;
        }
        if !self.is_routed(ROUTE_VOICE_2) {
            unfiltered += v2;
        }
        if !self.is_routed(ROUTE_VOICE_3) {
            unfiltered += v3;
        }
        if !self.is_routed(ROUTE_EXT_IN) {
            unfiltered += self.ext_in;
        }

        self.out = (unfiltered + filtered + MIXER_DC) * self.volume;
    }

    #[inline]
    pub fn output(&self) -> f64 {
        self.out
    }

    pub fn reset(&mut self) {
        self.band = 0.0;
        self.low = 0.0;
        self.out = 0.0;
        self.voice3_off = false;
        self.ext_in = 0.0;
    }
}
