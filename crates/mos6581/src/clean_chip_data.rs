//! Clean-room chip constants for the MOS 6581 SID.
//!
//! This file is clean-room work released under MIT OR Apache-2.0. Every value
//! below was derived only from the public, non-GPL documents listed here and
//! from the author's own arithmetic and design choices; no emulator source
//! code or emulator commentary was consulted. Each constant carries a
//! `Source:` line naming the document it comes from, or says plainly that it
//! is an unsourced own choice and why.
//!
//! Sources:
//! - MOS 6581 SID datasheet (Commodore/MOS Technology, 1982), as transcribed at
//!   <https://www.bellesondes.fr/wiki/doku.php?id=mos6581> (scan mirror:
//!   <https://www.cpcwiki.eu/imgs/9/9d/Mos_6581_sid.pdf>): envelope rates table,
//!   noise description, resonance description, AUDIO OUT pin notes, frequency
//!   formula.
//! - SID register reference, <https://www.oxyron.de/html/registers_sid.html>:
//!   noise LFSR feedback and output bits.
//! - Bob Yannes interview (1996),
//!   <http://www.landley.net/history/mirror/commodore/yannes.html>: envelope
//!   structure, piecewise exponential decay, noise clocking, volume-register
//!   playback.
//! - Commodore 64 board 250466 schematic (public redraws): R30 = 10 kOhm,
//!   C54 = 1 nF output low-pass; C55 = 10 uF output coupling capacitor.

/// Φ2 cycles between envelope counter steps, per attack/decay/release nibble.
///
/// Source: MOS 6581 datasheet, "Envelope Rates" table (Φ2 = 1.0 MHz): attack
/// 2 ms, 8 ms, 16 ms, 24 ms, 38 ms, 56 ms, 68 ms, 80 ms, 100 ms, 250 ms,
/// 500 ms, 800 ms, 1 s, 3 s, 5 s, 8 s, "from zero to peak".
/// Derivation: attack is 255 linear steps, so period = round(t_attack * 1 MHz / 255).
pub(crate) const ENV_RATE_PERIOD: [u16; 16] = [
    8, 31, 63, 94, 149, 220, 267, 314, 392, 980, 1961, 3137, 3922, 11765, 19608, 31373,
];

/// Decay/release divider breakpoints as (level, period in rate periods).
///
/// Source: Yannes interview: a second table on the envelope output slows the
/// envelope clock at set points during decay and release, "a piece-wise linear
/// approximation of an exponential"; datasheet table: decay/release times are
/// exactly 3x the attack times ("from peak to zero").
/// Derivation (own design of the breakpoints): the divider doubles each time the
/// level halves (104, 52, 26, 13, 6), giving a constant-slope log curve; the
/// first breakpoint (104) was chosen so a full 255 -> 0 sweep costs
/// 151*1 + 52*2 + 26*4 + 13*8 + 7*16 + 6*32 = 767 rate periods, i.e. 3.008x the
/// 255 periods of a full attack, matching the datasheet's 3x ratio.
pub(crate) const ENV_EXPONENTIAL_BREAKPOINTS: &[(u8, u16)] = &[
    (255, 1),
    (104, 2),
    (52, 4),
    (26, 8),
    (13, 16),
    (6, 32),
    (0, 1),
];

/// Whether the 15-bit rate counter skips 0 when it wraps past its maximum.
///
/// Source: unsourced own choice. No public non-GPL document describes the rate
/// counter's wrap behaviour. A plain binary counter wraps through 0, so this is
/// `false`; the value only matters when a period smaller than the current count
/// is written and the counter must run the full 2^15 cycle.
pub(crate) const ENV_RATE_COUNTER_SKIPS_ZERO: bool = false;

/// Output-stage low-pass corner, rad/s.
///
/// Source: C64 board 250466 schematic, R30 = 10 kOhm and C54 = 1 nF.
/// Derivation: w0 = 1 / (R C) = 1 / (1e4 * 1e-9) = 1.0e5 rad/s (about 15.9 kHz).
/// The nominal value is used with DT = 1e-6 uncompensated: forward Euler with
/// w0 * DT = 0.1 per step, run at the real PAL Φ2 (1.015 us per step), gives an
/// effective corner of -ln(0.9) / 1.015e-6 = about 1.04e5 rad/s, ~4 % high, which
/// is inaudible at 16 kHz.
pub(crate) const EXTFILT_W0_LOW: f64 = 1.0e5;

/// Output-stage high-pass corner, rad/s.
///
/// Source: C64 board 250466 schematic, C55 = 10 uF coupling capacitor; MOS 6581
/// datasheet, AUDIO OUT: "The recommended resistance is 1 KOhm".
/// Derivation: w0 = 1 / (R C) = 1 / (1e3 * 1e-5) = 100 rad/s (about 15.9 Hz).
/// w0 * DT = 1e-4, so Euler error is negligible; the DT vs real Φ2 mismatch
/// (1.5 % PAL, 2.2 % NTSC) only shifts the corner by that fraction.
pub(crate) const EXTFILT_W0_HIGH: f64 = 100.0;

/// Q at resonance register 0.
///
/// Source: MOS 6581 datasheet, register $17: "16 Resonance settings ranging
/// linearly from no resonance (0) to maximum resonance (15)".
/// Rationale (own choice of range): "no resonance" is taken as the Butterworth
/// value 1/sqrt(2), the highest Q with no response peak.
pub(crate) const FILTER_Q_MIN: f64 = core::f64::consts::FRAC_1_SQRT_2;

/// Resonance register steps per unit of Q.
///
/// Source: MOS 6581 datasheet, register $17 (linear 16-step scale); Yannes
/// interview: resonance came from a 4-bit weighted resistor ladder and NMOS
/// could not make the high-gain op-amps a strong resonant filter needs.
/// Rationale (own choice of range): with 8 steps per unit of Q, res = 15 gives
/// Q = 0.707 + 1.875 = 2.58, a peak of about +8 dB, a moderate maximum
/// consistent with Yannes' description of a weak resonance.
pub(crate) const FILTER_Q_STEPS: f64 = 8.0;

/// DC term added to the routed voice sum before the master volume.
///
/// Source: Yannes interview: "By stopping an Oscillator, it was possible to
/// apply a DC voltage to this D/A" and "Audio could then be created by having
/// the microprocessor write the Final Volume register in real-time".
/// Rationale (size and sign are an unsourced own choice): a non-zero DC is
/// needed for $D418 playback to be audible; 0.5 makes a full-scale 4-bit
/// volume sweep half a voice's normalised swing, loud but below a voice.
/// Positive sign is arbitrary, since the output high-pass removes steady DC
/// and the sign only sets the playback's phase.
pub(crate) const MIXER_DC: f64 = 0.5;

/// Waveform DAC value that represents zero signal.
///
/// Source: own reasoning on the 12-bit waveform DAC range 0..4095 (MOS 6581
/// datasheet, 12-bit waveform outputs). Rationale: a centred DAC; 2048 is the
/// midpoint code, so sawtooth, triangle and pulse at 50 % swing symmetrically.
pub(crate) const VOICE_WAVE_ZERO: i32 = 2048;

/// DC offset of a voice's output, in units of wave x level.
///
/// Source: unsourced own choice. Rationale: with a centred DAC the voice adds
/// no DC of its own; the DC that makes volume-register playback work is
/// modelled once, in the mixer (`MIXER_DC`), rather than per voice.
pub(crate) const VOICE_DC: i32 = 0;

/// Noise LFSR bits wired to waveform output bits 11, 10, ..., 4.
///
/// Source: <https://www.oxyron.de/html/registers_sid.html>: "The 8 bits of the
/// noise waveform are assembled from bits 20, 18, 14, 11, 9, 5, 2 and 0".
/// Derivation: listed high to low and mapped to the top 8 of the 12 output bits.
pub(crate) const NOISE_OUTPUT_TAPS: [u8; 8] = [20, 18, 14, 11, 9, 5, 2, 0];

/// Phase accumulator bit whose rising edge shifts the noise LFSR.
///
/// Source: unsourced own choice, constrained by the Yannes interview ("clocked
/// by one of the intermediate bits of the accumulator") and the datasheet
/// (noise ranges "from a low rumbling to hissing white noise"; Fout =
/// Fn * Fclk / 16777216). Rationale: bit k rises Fn * Fclk / 2^(k+1) times a
/// second; bit 19 is the highest (slowest) bit whose shift rate at Fn = 65535
/// (about 62.5 kHz) still exceeds twice 20 kHz, so the top setting is
/// full-band hiss while low settings rumble.
pub(crate) const NOISE_CLOCK_BIT: u8 = 19;

/// Noise LFSR value after reset and after TEST is released.
///
/// Source: unsourced own choice. Rationale: with XOR feedback only the
/// all-zero state locks up; all ones (0x7F_FFFF, 23 bits) is a valid,
/// maximally distant-from-lockup seed.
pub(crate) const NOISE_SHIFT_RELOAD: u32 = 0x7F_FFFF;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_periods_increase_and_fit_15_bits() {
        for w in ENV_RATE_PERIOD.windows(2) {
            assert!(w[0] < w[1], "{} !< {}", w[0], w[1]);
        }
        assert!(ENV_RATE_PERIOD.iter().all(|&p| p > 0 && p < (1 << 15)));
    }

    #[test]
    fn breakpoints_are_well_formed() {
        let levels: Vec<u8> = ENV_EXPONENTIAL_BREAKPOINTS.iter().map(|e| e.0).collect();
        for (i, a) in levels.iter().enumerate() {
            assert!(!levels[i + 1..].contains(a), "duplicate level {a}");
        }
        assert!(levels.contains(&0));
        assert!(levels.contains(&255));
        assert!(ENV_EXPONENTIAL_BREAKPOINTS.iter().all(|e| e.1 >= 1));
    }

    #[test]
    fn full_decay_is_three_times_attack() {
        // Walk 255 -> 0, loading the divider whenever a breakpoint level is reached.
        let mut period = 1u32;
        let mut total = 0u32;
        let mut level = 255u8;
        while level > 0 {
            if let Some(e) = ENV_EXPONENTIAL_BREAKPOINTS.iter().find(|e| e.0 == level) {
                period = u32::from(e.1);
            }
            total += period;
            level -= 1;
        }
        let ratio = f64::from(total) / 255.0;
        assert!((ratio - 3.0).abs() < 0.05, "decay/attack ratio {ratio}");
    }

    #[test]
    fn q_positive_and_increasing() {
        let q = |res: u8| FILTER_Q_MIN + f64::from(res) / FILTER_Q_STEPS;
        assert!(q(0) > 0.0);
        for r in 0..15 {
            assert!(q(r + 1) > q(r));
        }
    }

    #[test]
    fn noise_taps_distinct_in_range_high_to_low() {
        assert!(NOISE_OUTPUT_TAPS.iter().all(|&b| b < 23));
        for w in NOISE_OUTPUT_TAPS.windows(2) {
            assert!(w[0] > w[1]);
        }
        assert!(NOISE_CLOCK_BIT < 24);
    }

    #[test]
    fn noise_reload_valid() {
        assert!(NOISE_SHIFT_RELOAD != 0 && NOISE_SHIFT_RELOAD < (1 << 23));
    }

    #[test]
    fn extfilt_corners_sane() {
        assert!(EXTFILT_W0_LOW > 0.0 && EXTFILT_W0_HIGH > 0.0);
        assert!(EXTFILT_W0_LOW > 100.0 * EXTFILT_W0_HIGH);
        // Forward Euler stability at DT = 1e-6.
        assert!(EXTFILT_W0_LOW * 1.0e-6 < 1.0);
    }

    #[test]
    fn voice_constants_in_range() {
        assert!((0..4096).contains(&VOICE_WAVE_ZERO));
        assert_eq!(VOICE_DC, 0);
        assert!(MIXER_DC != 0.0);
    }
}
