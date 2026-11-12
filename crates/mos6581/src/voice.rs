//! One voice: oscillator, envelope, and the amplitude stage between them.

use crate::chip_data;
use crate::envelope::{Adsr, Envelope};
use crate::tables::WaveTables;
use crate::wave::Wave;

/// Divisor that puts a voice output into the range a mixer expects.
///
/// A full-envelope voice swings 293_760 .. 1_337_985, i.e. 0.28 .. 1.28 of this
/// constant, and never goes negative: the DC term offsets the whole swing
/// positive. Three voices sum to roughly 0.85 .. 3.8 in these units. The offset
/// is removed downstream by the output stage's high-pass, not here.
pub const FULL_SCALE: f64 = (0x0fff * 0xff) as f64;

/// The waveform's own zero level. It sits below the middle of the 12-bit range,
/// which is why a full-swing voice is not centred on zero.
const WAVE_ZERO: i32 = chip_data::VOICE_WAVE_ZERO;
/// The DC each voice carries on the mixer. It is present at every envelope
/// level, including silence, which is what keeps the output continuous as an
/// envelope starts and stops; the output stage's high-pass removes it.
const VOICE_DC: i32 = chip_data::VOICE_DC;

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
        // The DC term is unconditional. At level 0 the product vanishes and the
        // voice sits at VOICE_DC, one cycle before level 1 puts it at
        // VOICE_DC +/- 3199 - so an envelope starting or stopping is continuous.
        // Zeroing the output here would step by half a million at the end of
        // every release, which is a click.
        self.output = (wave - WAVE_ZERO) * level + VOICE_DC;
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
