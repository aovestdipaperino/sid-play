//! Oscillator: 24-bit phase accumulator, measured combination waveforms, noise.

use crate::chip_data;
use crate::tables::WaveTables;

pub const GATE: u8 = 1 << 0;
pub const OSC_SYNC: u8 = 1 << 1;
pub const RING_MOD: u8 = 1 << 2;
pub const TEST: u8 = 1 << 3;
pub const TRIANGLE: u8 = 1 << 4;
pub const SAWTOOTH: u8 = 1 << 5;
pub const PULSE: u8 = 1 << 6;
pub const NOISE: u8 = 1 << 7;

const ACC_MASK: u32 = 0x00ff_ffff;
const ACC_MSB: u32 = 0x0080_0000;
/// Accumulator bit whose rising edge clocks the noise register.
const NOISE_CLOCK: u32 = 1 << chip_data::NOISE_CLOCK_BIT;

/// What the 23-bit noise register is loaded with when the test bit is released.
const SHIFT_RELOAD: u32 = chip_data::NOISE_SHIFT_RELOAD;

/// The seven waveform selections that have a shape of their own. These are value
/// bindings, so a `match` on them compares bitmasks; writing `SAWTOOTH | TRIANGLE`
/// inline would be an or-pattern and would never match the combination.
const WAVE_SAW: u8 = SAWTOOTH;
const WAVE_TRI: u8 = TRIANGLE;
const WAVE_PULSE: u8 = PULSE;
const WAVE_SAW_TRI: u8 = SAWTOOTH | TRIANGLE;
const WAVE_PULSE_TRI: u8 = PULSE | TRIANGLE;
const WAVE_PULSE_SAW: u8 = PULSE | SAWTOOTH;
const WAVE_PULSE_SAW_TRI: u8 = PULSE | SAWTOOTH | TRIANGLE;

pub struct Wave {
    tables: &'static WaveTables,
    accumulator: u32,
    previous: u32,
    msb_rising: bool,
    /// 23-bit noise shift register.
    shift: u32,
    noise_shifts: u64,
    /// Test-bit state last cycle, so its edges are visible.
    test_prev: bool,
    output: u16,
}

impl Wave {
    pub fn new(tables: &'static WaveTables) -> Self {
        Self {
            tables,
            accumulator: 0,
            previous: 0,
            msb_rising: false,
            shift: SHIFT_RELOAD,
            noise_shifts: 0,
            test_prev: false,
            output: 0,
        }
    }

    #[inline]
    pub fn accumulator(&self) -> u32 {
        self.accumulator
    }

    #[inline]
    pub fn set_accumulator(&mut self, v: u32) {
        self.accumulator = v & ACC_MASK;
    }

    #[inline]
    pub fn msb_rising(&self) -> bool {
        self.msb_rising
    }

    /// How many times the noise register has shifted. A shift register that stops
    /// changing is locked up, and this counter is how the caller can tell.
    #[inline]
    pub fn noise_shifts(&self) -> u64 {
        self.noise_shifts
    }

    /// Hard sync. The caller resolves the chain in hardware order:
    /// voice 0 is the slave of voice 2, voice 1 of voice 0, voice 2 of voice 1,
    /// and skips a source that is itself being zeroed in the same cycle.
    #[inline]
    pub fn force_phase_zero(&mut self) {
        self.accumulator = 0;
        self.previous = 0;
        self.msb_rising = false;
    }

    /// One Φ2 cycle. `ring_source_msb` is bit 23 of the ring/sync source oscillator.
    pub fn tick(&mut self, freq: u16, pw: u16, ctrl: u8, ring_source_msb: bool) -> u16 {
        let test = (ctrl & TEST) != 0;
        if test != self.test_prev {
            // Toggling test is how a tune resets the noise generator: the register
            // is cleared while test is applied and reloaded the moment it is released.
            self.shift = if test { 0 } else { SHIFT_RELOAD };
            self.test_prev = test;
        }

        self.previous = self.accumulator;
        if test {
            // Test holds the oscillator at zero rather than letting it run.
            self.accumulator = 0;
            self.msb_rising = false;
        } else {
            self.accumulator = (self.accumulator + u32::from(freq)) & ACC_MASK;
            self.msb_rising = (self.previous & ACC_MSB) == 0 && (self.accumulator & ACC_MSB) != 0;
        }

        // The noise register is clocked by the rising edge of `NOISE_CLOCK`,
        // so the frequency register sets the noise rate.
        if (self.previous & NOISE_CLOCK) == 0 && (self.accumulator & NOISE_CLOCK) != 0 {
            let bit0 = ((self.shift >> 22) ^ (self.shift >> 17)) & 1;
            self.shift = ((self.shift << 1) & 0x007f_ffff) | bit0;
            self.noise_shifts += 1;
        }

        self.output = self.waveform(pw, ctrl, ring_source_msb);
        self.output
    }

    fn waveform(&self, pw: u16, ctrl: u8, ring_source_msb: bool) -> u16 {
        let selected = ctrl & (TRIANGLE | SAWTOOTH | PULSE | NOISE);
        if selected == 0 {
            return 0;
        }
        // Noise overrides the other selections; there is no measured table for a
        // combination that includes it.
        if selected & NOISE != 0 {
            return self.noise_output();
        }

        let phase = (self.accumulator >> 12) as u16;
        let t = self.tables;

        match selected {
            WAVE_SAW => phase,
            WAVE_TRI => self.triangle(ring_source_msb),
            WAVE_PULSE => {
                if self.pulse_high(pw, ctrl) {
                    0x0fff
                } else {
                    0
                }
            }
            WAVE_SAW_TRI => u16::from(t.st[usize::from(phase)]) << 4,
            WAVE_PULSE_TRI => {
                let tri = self.triangle(ring_source_msb) >> 1;
                (u16::from(t.p_t[usize::from(tri)]) << 4) & self.pulse_mask(pw, ctrl)
            }
            WAVE_PULSE_SAW => {
                (u16::from(t.ps_[usize::from(phase)]) << 4) & self.pulse_mask(pw, ctrl)
            }
            WAVE_PULSE_SAW_TRI => {
                (u16::from(t.pst[usize::from(phase)]) << 4) & self.pulse_mask(pw, ctrl)
            }
            _ => 0,
        }
    }

    #[inline]
    fn triangle(&self, ring_source_msb: bool) -> u16 {
        // Ring modulation folds the source oscillator's phase into this one's
        // before the sign is applied, which is what the hardware's XOR does.
        let msb = if ring_source_msb {
            (self.accumulator ^ ACC_MSB) & ACC_MSB
        } else {
            self.accumulator & ACC_MSB
        };
        let shaped = if msb != 0 {
            !self.accumulator
        } else {
            self.accumulator
        };
        ((shaped >> 11) & 0x0fff) as u16
    }

    #[inline]
    fn pulse_mask(&self, pw: u16, ctrl: u8) -> u16 {
        if self.pulse_high(pw, ctrl) { 0x0fff } else { 0 }
    }

    #[inline]
    fn pulse_high(&self, pw: u16, ctrl: u8) -> bool {
        (ctrl & TEST) != 0 || ((self.accumulator >> 12) as u16) >= pw.min(4095)
    }

    fn noise_output(&self) -> u16 {
        let mut out = 0u16;
        for (i, &tap) in chip_data::NOISE_OUTPUT_TAPS.iter().enumerate() {
            out |= (((self.shift >> tap) & 1) as u16) << (11 - i);
        }
        out
    }

    /// The byte `$d41b` returns.
    pub fn random_byte(&self) -> u8 {
        (self.shift >> 15) as u8
    }

    pub fn reset(&mut self) {
        self.accumulator = 0;
        self.previous = 0;
        self.msb_rising = false;
        self.shift = SHIFT_RELOAD;
        self.noise_shifts = 0;
        self.test_prev = false;
        self.output = 0;
    }
}
