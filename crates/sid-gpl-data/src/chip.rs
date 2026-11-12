//! SID chip constants: reSID (Dag Lem), GPL-2.0. See docs/provenance.md.

/// Envelope rate-counter period per ADSR nibble.
pub const ENV_RATE_PERIOD: [u16; 16] = [
    9, 32, 63, 95, 149, 220, 267, 313, 392, 977, 1954, 3126, 3907, 11720, 19532, 31251,
];
/// (envelope counter value, exponential-counter period) loaded on falling phases.
pub const ENV_EXPONENTIAL_BREAKPOINTS: &[(u8, u16)] = &[
    (0, 1),
    (6, 30),
    (14, 16),
    (26, 8),
    (54, 4),
    (93, 2),
    (255, 1),
];
/// The 15-bit rate counter skips 0 when it wraps.
pub const ENV_RATE_COUNTER_SKIPS_ZERO: bool = true;
/// External filter low-pass pole frequency, rad/s.
pub const EXTFILT_W0_LOW: f64 = 104_858.0;
/// External filter high-pass pole frequency, rad/s.
pub const EXTFILT_W0_HIGH: f64 = 105.0;
/// Filter Q model: Q = FILTER_Q_MIN + res / FILTER_Q_STEPS.
pub const FILTER_Q_MIN: f64 = 0.707;
/// Divisor of the resonance register in the Q model.
pub const FILTER_Q_STEPS: f64 = 15.0;
/// DC added by the mixer, in voice-output units.
pub const MIXER_DC: f64 = -1.0 / 18.0;
/// Waveform DAC zero level.
pub const VOICE_WAVE_ZERO: i32 = 0x380;
/// Voice DC offset.
pub const VOICE_DC: i32 = 0x800 * 0xff;
/// Noise register bits wired to output bits 11 down to 4, in that order.
pub const NOISE_OUTPUT_TAPS: [u8; 8] = [22, 20, 16, 13, 11, 7, 4, 2];
/// Accumulator bit whose rising edge clocks the noise register.
pub const NOISE_CLOCK_BIT: u8 = 19;
/// Noise register value after reset and on release of the test bit.
pub const NOISE_SHIFT_RELOAD: u32 = 0x7f_ff_f8;
