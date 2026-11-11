//! One voice: oscillator, envelope, and the amplitude stage between them.

use crate::envelope::{Adsr, Envelope};
use crate::tables::WaveTables;
use crate::wave::Wave;

/// Divisor that maps a voice output onto roughly plus or minus one.
pub const FULL_SCALE: f64 = (0x0fff * 0xff) as f64;

/// The waveform's own zero level. It sits below the middle of the 12-bit range,
/// which is why a full-swing voice is not centred on zero.
const WAVE_ZERO: i32 = 0x380;
/// The DC the mixer adds per voice at full envelope.
const VOICE_DC: i32 = 0x800 * 0xff;

pub struct Voice {
    wave: Wave,
    env: Envelope,
    output: i32,
}

impl Voice {
    pub fn new(tables: &'static WaveTables) -> Self {
        Self {
            wave: Wave::new(tables),
            env: Envelope::default(),
            output: 0,
        }
    }

    /// One Phi 2 cycle. `ring_source_msb` is the ring/sync source oscillator's
    /// accumulator bit 23, sampled before either oscillator advanced.
    pub fn tick(&mut self, freq: u16, pw: u16, ctrl: u8, adsr: Adsr, ring_source_msb: bool) -> i32 {
        let wave = i32::from(self.wave.tick(freq, pw, ctrl, ring_source_msb));
        self.env.tick((ctrl & crate::wave::GATE) != 0, adsr);
        let level = i32::from(self.env.level());
        self.output = if level == 0 {
            0
        } else {
            (wave - WAVE_ZERO) * level + VOICE_DC
        };
        self.output
    }

    #[inline]
    pub fn output(&self) -> i32 {
        self.output
    }

    #[inline]
    pub fn wave_bit23(&self) -> bool {
        self.wave.accumulator() & 0x0080_0000 != 0
    }

    #[inline]
    pub fn msb_rising(&self) -> bool {
        self.wave.msb_rising()
    }

    #[inline]
    pub fn force_phase_zero(&mut self) {
        self.wave.force_phase_zero();
    }

    #[inline]
    pub fn envelope_level(&self) -> u8 {
        self.env.level()
    }

    #[inline]
    pub fn random_byte(&self) -> u8 {
        self.wave.random_byte()
    }

    pub fn reset(&mut self) {
        self.wave.reset();
        self.env.reset();
        self.output = 0;
    }
}
