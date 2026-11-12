//! Chip-rate to host-rate conversion. The chip always runs at Phi 2; this is the
//! only place the host sample rate enters the signal path.
//!
//! The filter is a Kaiser-windowed sinc whose taps sit one *chip* sample apart, with
//! the tap count scaled by the chip-to-host ratio so the transition band lands where
//! the attenuation target requires. Spacing the taps one *host* sample apart instead
//! would be wrong however long the filter were made: a comb with taps every `R`
//! samples has a response periodic at `clock / R`, so it samples the signal itself at
//! the host rate and folds everything above host Nyquist into the passband before the
//! filter gets a chance to remove it. The whole purpose of the stage is to remove that
//! energy, so the grid has to be the chip's own.
//!
//! The taps are placed symmetrically
//! about the output time, so it has zero phase: a sine that goes in comes out at the
//! same instant rather than a millisecond later. The cost is that an output cannot be
//! produced until the newest sample its window needs has arrived, which delays the
//! stream by half the filter length.

use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Take the nearest chip sample. No filtering, so out-of-band energy aliases.
    Fast,
    /// Linear interpolation between the two straddling chip samples. No filtering.
    Interpolate,
    /// Anti-alias filter with interpolated taps.
    ResampleInterpolate,
    /// Anti-alias filter reading the nearest coefficient-table entry. The chip signal
    /// is always read at exact integer chip offsets; what this discards is the
    /// sub-chip-sample output phase, which `ResampleInterpolate` keeps.
    ResampleFast,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResampleError {
    /// The requested rates cannot be served: the ratio is too wide for the available
    /// history, or the host rate is not below the chip rate.
    UnsupportedRatio { clock_hz: f64, sample_rate: f64 },
}

impl std::fmt::Display for ResampleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedRatio {
                clock_hz,
                sample_rate,
            } => {
                let (clock_hz, sample_rate) = (*clock_hz, *sample_rate);
                if !clock_hz.is_finite() || !sample_rate.is_finite() || clock_hz <= 0.0 {
                    write!(
                        f,
                        "cannot resample {clock_hz} Hz to {sample_rate} Hz: \
                         the chip clock must be a finite rate above zero"
                    )
                } else if sample_rate <= 0.0 {
                    write!(
                        f,
                        "cannot resample {clock_hz:.0} Hz to {sample_rate:.0} Hz: \
                         the host sample rate must be above zero"
                    )
                } else if clock_hz / sample_rate < 1.0 {
                    write!(
                        f,
                        "cannot resample {clock_hz:.0} Hz to {sample_rate:.0} Hz: the host \
                         rate is above the chip rate, so there is nothing to decimate"
                    )
                } else {
                    write!(
                        f,
                        "cannot resample {clock_hz:.0} Hz to {sample_rate:.0} Hz: the \
                         anti-alias filter would need more than the {RINGSIZE} chip samples \
                         of history it has. Raise the host sample rate or use a cheaper method."
                    )
                }
            }
        }
    }
}

impl std::error::Error for ResampleError {}

/// History the tap window may reach back through. A power of two so the ring index
/// can be masked.
const RINGSIZE: usize = 16_384;
const MASK: i64 = RINGSIZE as i64 - 1;
/// Stopband attenuation of a 16-bit signal path.
const ATTENUATION: f64 = 96.327;
/// Passband edge as a fraction of the host Nyquist frequency.
const PASSBAND: f64 = 0.9;

/// Modified Bessel function of the first kind, order zero: `sum ((x/2)^k / k!)^2`.
fn bessel_0(x: f64) -> f64 {
    let q = x * x * 0.25;
    let mut acc = 1.0;
    let mut term = 1.0;
    for k in 1..=200u32 {
        term *= q / f64::from(k * k);
        let next = acc + term;
        if next == acc {
            break;
        }
        acc = next;
    }
    acc
}

/// Kaiser-windowed low-pass at the chip rate, normalised to unity gain at DC.
///
/// `h(x)` is stored for integer `x` from `-M` to `M+1`, so index `centre + j` holds
/// the coefficient for a tap `j` chip samples from the output time. The extra `M+1`
/// entry is the window's edge value, which the fractional-phase interpolation reads.
///
/// Taps are one *chip* sample apart, not one host sample apart. A comb spaced a host
/// sample apart samples the signal itself at the host rate, and any energy above host
/// Nyquist aliases into the filter's own passband before the filter can act on it -
/// which is precisely the energy an anti-alias filter exists to remove.
fn build_fir(cutoff: f64, m: usize) -> Vec<f64> {
    let beta = 0.1102 * (ATTENUATION - 8.7);
    let denom = bessel_0(beta);
    let centre = m as f64;
    let mut fir = vec![0.0; 2 * m + 2];
    for (k, slot) in fir.iter_mut().enumerate() {
        let x = k as f64 - centre;
        if x.abs() > centre {
            continue;
        }
        let sinc = if x == 0.0 {
            2.0 * cutoff
        } else {
            (2.0 * PI * cutoff * x).sin() / (PI * x)
        };
        let arg = (1.0 - (x / centre).powi(2)).max(0.0).sqrt();
        *slot = sinc * (bessel_0(beta * arg) / denom);
    }
    let sum: f64 = fir[..2 * m + 1].iter().sum();
    for s in fir[..2 * m + 1].iter_mut() {
        *s /= sum;
    }
    fir
}

pub struct Resampler {
    /// Zero-phase coefficient table, or empty for the unfiltered methods.
    fir: Vec<f64>,
    /// Index of the `x = 0` coefficient, and half the tap span in chip samples.
    centre: usize,
    filtered: bool,
    ring: Vec<f64>,
    /// Ring index of the most recently written sample.
    pos: usize,
    /// How many chip samples have been fed. Chip time of the newest one is `cycles - 1`.
    cycles: i64,
    /// Chip cycles per host sample. Always >= 1.
    cycles_per_sample: f64,
    /// Chip time of the next host sample to emit. Starts at zero so host sample `j`
    /// corresponds to chip time `j * cycles_per_sample`.
    next_out: f64,
    /// How far the newest sample an output's window needs lies ahead of the output time.
    delay: f64,
    /// Interpolate between adjacent values - of the signal for the unfiltered
    /// methods, of the coefficient table for the filtered ones.
    interpolate: bool,
}

impl Resampler {
    pub fn new(clock_hz: f64, sample_rate: f64, method: Method) -> Result<Self, ResampleError> {
        let bad = !clock_hz.is_finite()
            || !sample_rate.is_finite()
            || clock_hz <= 0.0
            || sample_rate <= 0.0;
        let ratio = if bad { 0.0 } else { clock_hz / sample_rate };
        if bad || ratio < 1.0 {
            return Err(ResampleError::UnsupportedRatio {
                clock_hz,
                sample_rate,
            });
        }

        let filtered = matches!(method, Method::ResampleInterpolate | Method::ResampleFast);
        let (centre, fir) = if filtered {
            // Kaiser order for the required transition, expressed at the host rate,
            // then spread over the chip axis one tap per chip sample.
            let dw = (1.0 - PASSBAND) * PI;
            let order = ((ATTENUATION - 7.95) / (2.285 * dw) + 0.5) as usize;
            // The tap count is spread over the chip axis, so a ratio this wide cannot
            // be represented as a tap count at all. Reject it here rather than let the
            // cast below saturate: a wrapped count would build a one-tap filter that
            // silently stops nothing.
            if ratio > RINGSIZE as f64 {
                return Err(ResampleError::UnsupportedRatio {
                    clock_hz,
                    sample_rate,
                });
            }
            let span = ((order as f64 * ratio) as usize | 1) + 1;
            let m = span / 2;
            let cutoff = PASSBAND * 0.5 * sample_rate / clock_hz;
            // The oldest tap sits `centre` chip samples behind the output time and the
            // newest `centre` ahead of it, and the output time itself can lag the
            // newest chip sample by up to one host period. Saturating so that no
            // accepted ratio can panic on the arithmetic.
            let need = 2usize
                .saturating_mul(m)
                .saturating_add(2)
                .saturating_add(ratio.ceil() as usize)
                .saturating_add(1);
            if need > RINGSIZE {
                return Err(ResampleError::UnsupportedRatio {
                    clock_hz,
                    sample_rate,
                });
            }
            (m, build_fir(cutoff, m))
        } else {
            (0, Vec::new())
        };

        Ok(Self {
            fir,
            centre,
            filtered,
            ring: vec![0.0; RINGSIZE],
            pos: 0,
            cycles: 0,
            cycles_per_sample: ratio,
            next_out: 0.0,
            delay: centre as f64,
            interpolate: matches!(method, Method::Interpolate | Method::ResampleInterpolate),
        })
    }

    /// One chip-rate sample. Call exactly once per Phi 2 cycle.
    pub fn input(&mut self, sample: f64) {
        self.pos = (self.pos + 1) & MASK as usize;
        self.ring[self.pos] = sample;
        self.cycles += 1;
    }

    /// Call once per Phi 2 cycle, after [`Resampler::input`]. Returns `Some` once per
    /// host sample period.
    pub fn output(&mut self) -> Option<f64> {
        if self.cycles == 0 {
            return None;
        }
        let now = self.cycles as f64 - 1.0;
        if self.next_out + self.delay > now {
            return None;
        }
        let t = self.next_out;
        self.next_out += self.cycles_per_sample;

        if !self.filtered {
            return Some(self.sample_at(t));
        }

        // The window is centred on the output time, so the filter has zero phase and
        // the price is latency rather than a tone arriving late.
        let n0 = t.floor() as i64;
        let phi = t - t.floor();
        let centre = self.centre as i64;
        let newest = self.cycles - 1;
        let last = self.fir.len() - 1;
        let mut acc = 0.0;
        for j in -centre..=centre {
            // Tap `j` sits at chip offset `j` from the output time, whose fractional
            // part `phi` falls between coefficients, so the table is read at `k - phi`.
            let k = (centre + j) as usize;
            let c = if self.interpolate {
                // Catmull-Rom through four table entries. Linear reads of the table
                // leave a few tens of dB of passband ripple, because a windowed sinc
                // has real curvature between adjacent taps.
                let y0 = self.fir[(k + 1).min(last)];
                let y1 = self.fir[k];
                let y2 = self.fir[k.saturating_sub(1)];
                let y3 = self.fir[k.saturating_sub(2)];
                let t2 = phi * phi;
                0.5 * (2.0 * y1
                    + (-y0 + y2) * phi
                    + (2.0 * y0 - 5.0 * y1 + 4.0 * y2 - y3) * t2
                    + (-y0 + 3.0 * y1 - 3.0 * y2 + y3) * t2 * phi)
            } else {
                self.fir[k]
            };
            let back = newest - (n0 + j);
            acc += c * self.ring[((self.pos as i64 - back) & MASK) as usize];
        }
        Some(acc)
    }

    /// The chip-rate signal at fractional chip time `u`, interpolated or truncated to
    /// the nearest sample. Times before the first sample read as the silence the ring
    /// was initialised with.
    fn sample_at(&self, u: f64) -> f64 {
        let i = u.floor();
        let now = self.cycles - 1;
        // Chip time `i` is `back` samples behind the newest one.
        let back = (now as f64 - i).round() as i64;
        let p = self.pos as i64 - back;
        let a = self.ring[(p & MASK) as usize];
        if self.interpolate {
            let b = self.ring[((p + 1) & MASK) as usize];
            a + (b - a) * (u - i)
        } else {
            a
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOCK: f64 = 985_248.0;

    /// Feed a sine at chip rate and collect host samples.
    fn pump(r: &mut Resampler, hz: f64, cycles: usize) -> Vec<f64> {
        let mut out = Vec::new();
        for i in 0..cycles {
            r.input((2.0 * std::f64::consts::PI * hz * i as f64 / CLOCK).sin());
            out.extend(r.output());
        }
        out
    }

    /// SNR against the ideal sine, compared at **absolute** host indices.
    ///
    /// Host sample `i` corresponds to chip time `i * cycles_per_sample`, so the
    /// reference must be evaluated at the sample's own index. Slicing a tail and
    /// re-basing the reference to zero instead compares against a tone that is
    /// hundreds of samples out of phase, which reads as distortion.
    fn snr(samples: &[f64], from: usize, hz: f64, sample_rate: f64) -> f64 {
        let mut err = 0.0;
        let mut sig = 0.0;
        for (i, &s) in samples.iter().enumerate().skip(from) {
            let ideal = (2.0 * std::f64::consts::PI * hz * i as f64 / sample_rate).sin();
            err += (s - ideal) * (s - ideal);
            sig += ideal * ideal;
        }
        10.0 * (sig / err.max(1e-30)).log10()
    }

    /// The delay, in host samples, that best aligns the output with the ideal.
    fn best_lag(samples: &[f64], from: usize, hz: f64, sample_rate: f64) -> i64 {
        let mut best = (0i64, f64::NEG_INFINITY);
        for lag in -400i64..400 {
            let mut err = 0.0;
            let mut sig = 0.0;
            for (i, &s) in samples.iter().enumerate().skip(from) {
                let j = i as i64 - lag;
                if j < 0 {
                    continue;
                }
                let ideal = (2.0 * std::f64::consts::PI * hz * j as f64 / sample_rate).sin();
                err += (s - ideal) * (s - ideal);
                sig += ideal * ideal;
            }
            let db = sig / err.max(1e-30);
            if db > best.1 {
                best = (lag, db);
            }
        }
        best.0
    }

    fn rms(v: &[f64]) -> f64 {
        (v.iter().map(|s| s * s).sum::<f64>() / v.len() as f64).sqrt()
    }

    #[test]
    fn a_sine_comes_out_as_the_same_sine() {
        let mut r = Resampler::new(CLOCK, 44_100.0, Method::ResampleInterpolate).unwrap();
        let s = pump(&mut r, 1_000.0, 100_000);
        assert!(s.len() > 4_000, "only {} samples came out", s.len());
        let from = s.len() / 2;
        let db = snr(&s, from, 1_000.0, 44_100.0);
        assert!(db > 60.0, "reconstruction SNR is only {db:.1} dB");
    }

    #[test]
    fn dc_passes_at_unity_gain() {
        let mut r = Resampler::new(CLOCK, 44_100.0, Method::ResampleInterpolate).unwrap();
        let mut last = 0.0;
        for _ in 0..60_000 {
            r.input(0.25);
            if let Some(s) = r.output() {
                last = s;
            }
        }
        assert!((last - 0.25).abs() < 1e-6, "dc gain was {last}");
    }

    #[test]
    fn energy_above_nyquist_does_not_fold_back() {
        // 300 kHz at a 1 MHz chip rate would alias straight into the audio band
        // without the anti-alias filter.
        let mut r = Resampler::new(CLOCK, 44_100.0, Method::ResampleInterpolate).unwrap();
        let s = pump(&mut r, 300_000.0, 100_000);
        let tail = &s[s.len() / 2..];
        assert!(
            rms(tail) < 0.01,
            "out-of-band energy reached the output at rms {}",
            rms(tail)
        );
    }

    #[test]
    fn energy_just_above_nyquist_does_not_fold_into_the_audio_band() {
        // 24 kHz is only 1.09x host Nyquist, so it folds to about 1.95 kHz - right
        // in the middle of the passband. A filter that only stops far-out energy
        // passes this one and the chip hisses.
        let mut r = Resampler::new(CLOCK, 44_100.0, Method::ResampleInterpolate).unwrap();
        let s = pump(&mut r, 24_000.0, 200_000);
        let tail = &s[s.len() / 2..];
        assert!(
            rms(tail) < 1e-3,
            "24 kHz folded into the audio band at rms {} (input amplitude 0.707)",
            rms(tail)
        );
    }

    #[test]
    fn an_impossible_ratio_is_refused_at_construction() {
        let e = Resampler::new(CLOCK, 4_000.0, Method::ResampleInterpolate);
        assert!(matches!(e, Err(ResampleError::UnsupportedRatio { .. })));
    }

    #[test]
    fn a_degenerate_rate_is_refused_not_panicked() {
        assert!(Resampler::new(CLOCK, 0.0, Method::Fast).is_err());
        assert!(Resampler::new(CLOCK, -44_100.0, Method::Fast).is_err());
        assert!(Resampler::new(0.0, 44_100.0, Method::Fast).is_err());
        assert!(Resampler::new(CLOCK, f64::NAN, Method::Fast).is_err());
        assert!(Resampler::new(CLOCK, 2_000_000.0, Method::Fast).is_err());
    }

    /// A positive rate so small that the ratio overflows a tap count must come back
    /// as an error. Left unchecked the cast saturates and the `+ 1` overflows: a panic
    /// in a debug build, and in release a wrapped count that builds a one-tap filter
    /// stopping nothing at all.
    #[test]
    fn an_absurdly_wide_ratio_is_refused_before_the_tap_count_overflows() {
        for rate in [1e-30, 1e-12, 1e-6, 1.0] {
            assert!(
                Resampler::new(CLOCK, rate, Method::ResampleInterpolate).is_err(),
                "a {rate} Hz host rate against a {CLOCK} Hz chip must be refused"
            );
            assert!(
                Resampler::new(CLOCK, rate, Method::ResampleFast).is_err(),
                "a {rate} Hz host rate must be refused for the fast filter too"
            );
        }
        // The unfiltered methods hold the ratio as a float and need no tap table, so
        // they still accept these rates rather than inventing a limit.
        assert!(Resampler::new(CLOCK, 1e-30, Method::Fast).is_ok());
    }

    #[test]
    fn the_error_message_names_a_host_rate_above_the_chip_rate() {
        let msg = match Resampler::new(CLOCK, 2_000_000.0, Method::Fast) {
            Ok(_) => panic!("a host rate above the chip rate must be refused"),
            Err(e) => e.to_string(),
        };
        assert!(
            msg.contains("above the chip rate"),
            "message blames the wrong thing: {msg}"
        );
    }

    #[test]
    fn the_fast_method_produces_the_right_number_of_samples() {
        let mut r = Resampler::new(CLOCK, 44_100.0, Method::Fast).unwrap();
        // One full second of PAL chip cycles.
        let s = pump(&mut r, 1_000.0, 985_248);
        assert!(
            s.len() > 44_000 && s.len() < 44_200,
            "one second of Phi 2 gave {} samples, expected about 44100",
            s.len()
        );
    }

    #[test]
    fn every_method_emits_at_the_host_rate() {
        for m in [
            Method::Fast,
            Method::Interpolate,
            Method::ResampleInterpolate,
            Method::ResampleFast,
        ] {
            // The filtered methods hold back by half their tap span before the first
            // output, so the absolute count over one second is short by that warm-up.
            // The rate is what must be exact, and it is measured by differencing.
            let mut short = Resampler::new(CLOCK, 44_100.0, m).unwrap();
            let mut long = Resampler::new(CLOCK, 44_100.0, m).unwrap();
            let a = pump(&mut short, 1_000.0, 98_525).len() as f64;
            let b = pump(&mut long, 1_000.0, 985_248).len() as f64;
            // The difference spans exactly 0.9 s of chip cycles, warm-up included in
            // neither term's advantage.
            let per_second = (b - a) / 0.9;
            assert!(
                (per_second - 44_100.0).abs() < 2.0,
                "{m:?} emitted {a} samples in 0.1s and {b} in 1s, so its rate is \
                 {per_second} per second rather than 44100"
            );
            assert!(
                b > 44_000.0 && b <= 44_100.0,
                "{m:?} produced {b} samples in one second"
            );
        }
    }

    #[test]
    fn the_interpolating_method_is_closer_than_the_fast_one() {
        let mut fast = Resampler::new(CLOCK, 44_100.0, Method::Fast).unwrap();
        let mut lerp = Resampler::new(CLOCK, 44_100.0, Method::Interpolate).unwrap();
        let f = pump(&mut fast, 5_000.0, 200_000);
        let i = pump(&mut lerp, 5_000.0, 200_000);
        let fs = snr(&f, f.len() / 2, 5_000.0, 44_100.0);
        let is = snr(&i, i.len() / 2, 5_000.0, 44_100.0);
        assert!(
            is > fs,
            "interpolating {is:.1} dB is no better than fast {fs:.1} dB"
        );
    }

    #[test]
    fn the_filter_does_not_shift_a_tone_in_time() {
        // Zero phase is the reason the taps sit symmetrically about the output time.
        // A causal window would delay 1 kHz by half the filter length, about 62 host
        // samples, and the SNR test above would collapse without saying why.
        let mut r = Resampler::new(CLOCK, 44_100.0, Method::ResampleInterpolate).unwrap();
        let s = pump(&mut r, 1_000.0, 200_000);
        let lag = best_lag(&s, s.len() / 2, 1_000.0, 44_100.0);
        assert!(
            lag.abs() < 2,
            "output is best matched at lag {lag} host samples, so the filter is not zero phase"
        );
    }
}
