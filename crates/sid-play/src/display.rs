//! The terminal display: a progress bar and a spectrum analyser, both drawn in
//! Unicode braille. Each braille character is a grid of dots two wide and four
//! tall, so one character holds two spectrum bands at four height steps each,
//! and one character of the progress bar advances in two steps. A band's
//! recent peak stays above its bar as a single dot and sinks slowly.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::time::{Duration, Instant};

/// The blank braille pattern; every other pattern is this plus dot bits.
const BRAILLE: u32 = 0x2800;
/// The dot bits of a braille cell's left and right columns, top to bottom
/// (dots 1, 2, 3, 7 and 4, 5, 6, 8 in braille numbering).
const LEFT: [u8; 4] = [0x01, 0x02, 0x04, 0x40];
const RIGHT: [u8; 4] = [0x08, 0x10, 0x20, 0x80];
/// Progress bar cells: played, half played, and still to come (a bottom line).
const BAR_FULL: char = '⣿';
const BAR_HALF: char = '⣇';
const BAR_TRACK: char = '⣀';

/// Samples per analysis window: 186 ms at 44.1 kHz, 5.4 Hz per bin, fine
/// enough that the narrow bands at the bottom of a 128-band log axis still
/// land on bins of their own for most of the range.
const FFT_SIZE: usize = 8192;
/// The loudest and quietest levels the analyser shows, in dB below a
/// full-scale sine. Three voices share the output and no single band gets near
/// full scale, so the top of the display sits below it.
const CEILING_DB: f32 = 18.0;
const FLOOR_DB: f32 = 70.0;
/// How fast a band falls once its level drops, and how fast its peak marker
/// sinks, in fractions of the full height per second.
const FALL_PER_SECOND: f32 = 1.6;
const PEAK_FALL_PER_SECOND: f32 = 0.35;
/// How long a peak marker holds before it starts to sink.
const PEAK_HOLD: f32 = 0.6;

/// In-place radix-2 FFT over `re`/`im`, whose length must be a power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * std::f32::consts::PI / len as f32;
        let (w_im, w_re) = angle.sin_cos();
        for start in (0..n).step_by(len) {
            let (mut c_re, mut c_im) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let t_re = re[b] * c_re - im[b] * c_im;
                let t_im = re[b] * c_im + im[b] * c_re;
                re[b] = re[a] - t_re;
                im[b] = im[a] - t_im;
                re[a] += t_re;
                im[a] += t_im;
                let next = c_re * w_re - c_im * w_im;
                c_im = c_re * w_im + c_im * w_re;
                c_re = next;
            }
        }
        len <<= 1;
    }
}

/// Turns the most recent samples into per-band levels between 0 and 1.
pub struct Analyzer {
    window: Vec<f32>,
    /// Inclusive FFT bin range of each band, low to high.
    bands: Vec<(usize, usize)>,
    /// What the display shows: rises at once, falls at `FALL_PER_SECOND`.
    levels: Vec<f32>,
    peaks: Vec<f32>,
    /// Seconds each peak has held.
    peak_age: Vec<f32>,
    /// The newest samples, plus `delay` samples the listener has not heard yet.
    history: VecDeque<f32>,
    delay: usize,
}

impl Analyzer {
    /// `bands` log-spaced bands from `low` to `high` Hz. The analysed window
    /// ends `delay` samples before the newest one fed, so a display can match
    /// what is coming out of a buffered audio device rather than what was just
    /// rendered into it.
    pub fn new(sample_rate: f32, bands: usize, low: f32, high: f32, delay: usize) -> Self {
        let bin_hz = sample_rate / FFT_SIZE as f32;
        let top_bin = FFT_SIZE / 2 - 1;
        let edge = |i: usize| low * (high / low).powf(i as f32 / bands as f32);
        // A band narrower than a bin shares the bin nearest it with its
        // neighbours, rather than being pushed up to a bin above its range.
        let ranges = (0..bands)
            .map(|b| {
                let lo = ((edge(b) / bin_hz).round() as usize).clamp(1, top_bin);
                let hi = ((edge(b + 1) / bin_hz).round() as usize)
                    .saturating_sub(1)
                    .clamp(lo, top_bin);
                (lo, hi)
            })
            .collect();
        let window = (0..FFT_SIZE)
            .map(|i| {
                let x = std::f32::consts::TAU * i as f32 / (FFT_SIZE - 1) as f32;
                0.5 - 0.5 * x.cos()
            })
            .collect();
        Self {
            window,
            bands: ranges,
            levels: vec![0.0; bands],
            peaks: vec![0.0; bands],
            peak_age: vec![0.0; bands],
            history: VecDeque::with_capacity(FFT_SIZE + delay + 4096),
            delay,
        }
    }

    pub fn feed(&mut self, samples: &[f32]) {
        self.history.extend(samples);
        let keep = FFT_SIZE + self.delay;
        if self.history.len() > keep {
            self.history.drain(..self.history.len() - keep);
        }
    }

    /// Analyse the current window and move the displayed levels towards it,
    /// `dt` seconds after the previous update.
    pub fn update(&mut self, dt: f32) {
        let end = self.history.len().saturating_sub(self.delay);
        let start = end.saturating_sub(FFT_SIZE);
        let mut re = vec![0.0f32; FFT_SIZE];
        let mut im = vec![0.0f32; FFT_SIZE];
        for (k, s) in self.history.range(start..end).enumerate() {
            re[k] = s * self.window[k];
        }
        fft(&mut re, &mut im);
        // A full-scale sine under a Hann window peaks at N/4 in its bin.
        let scale = 4.0 / FFT_SIZE as f32;
        for (b, &(lo, hi)) in self.bands.iter().enumerate() {
            let amp = (lo..=hi)
                .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() * scale)
                .fold(0.0f32, f32::max);
            let db = 20.0 * amp.max(1e-9).log10();
            let target = ((db + FLOOR_DB) / (FLOOR_DB - CEILING_DB)).clamp(0.0, 1.0);
            let level = &mut self.levels[b];
            *level = target.max(*level - FALL_PER_SECOND * dt);
            if *level >= self.peaks[b] {
                self.peaks[b] = *level;
                self.peak_age[b] = 0.0;
            } else {
                self.peak_age[b] += dt;
                if self.peak_age[b] > PEAK_HOLD {
                    self.peaks[b] = (self.peaks[b] - PEAK_FALL_PER_SECOND * dt).max(*level);
                }
            }
        }
    }

    pub fn levels(&self) -> &[f32] {
        &self.levels
    }

    pub fn peaks(&self) -> &[f32] {
        &self.peaks
    }
}

/// The spectrum as `height` rows of braille, top row first. Each character
/// holds two bands side by side, each `4 * height` dots tall in all.
pub fn spectrum_rows(levels: &[f32], peaks: &[f32], height: usize) -> Vec<String> {
    let steps = 4 * height;
    // Per band: how many dots its bar fills from the bottom, and the dot its
    // peak sits on when that is above the bar.
    let column = |b: usize| -> (usize, Option<usize>) {
        let (Some(&level), Some(&peak)) = (levels.get(b), peaks.get(b)) else {
            return (0, None);
        };
        let filled = ((level * steps as f32).round() as usize).min(steps);
        let peak_dot = ((peak * steps as f32).round() as usize).min(steps);
        (filled, (peak_dot > filled).then(|| peak_dot - 1))
    };
    let mut rows = vec![String::with_capacity(levels.len() * 2); height];
    for pair in (0..levels.len()).step_by(2) {
        let sides = [(column(pair), LEFT), (column(pair + 1), RIGHT)];
        for (r, row) in rows.iter_mut().enumerate() {
            let mut bits = 0u8;
            for ((filled, peak), dots) in sides {
                for (d, &bit) in dots.iter().enumerate() {
                    // The dot's height from the bottom of the display.
                    let at = (height - 1 - r) * 4 + (3 - d);
                    if at < filled || peak == Some(at) {
                        bits |= bit;
                    }
                }
            }
            row.push(char::from_u32(BRAILLE + u32::from(bits)).unwrap_or(' '));
        }
    }
    rows
}

/// `width` braille cells, filled for the part done over a bottom line for the
/// rest, advancing half a cell at a time.
pub fn progress_bar(fraction: f64, width: usize) -> String {
    let halves = ((fraction.clamp(0.0, 1.0) * 2.0 * width as f64).round() as usize).min(2 * width);
    (0..width)
        .map(|i| match halves.saturating_sub(2 * i) {
            0 => BAR_TRACK,
            1 => BAR_HALF,
            _ => BAR_FULL,
        })
        .collect()
}

/// `m:ss`.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).floor() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// How the end of the tune is decided, for the line under the bar.
#[derive(Debug, Clone, Copy)]
pub enum Length {
    /// Known: from the database or the command line.
    Known(f64),
    /// Unknown: playing until the tune goes quiet, at most this long.
    UntilSilence(f64),
}

/// The ANSI colour for row `cell` (from the bottom) of a spectrum `height`
/// rows tall: green, then yellow, then red near the top.
fn colour(cell: usize, height: usize) -> &'static str {
    let f = (cell + 1) as f32 / height as f32;
    if f > 0.85 {
        "\x1b[31m"
    } else if f > 0.6 {
        "\x1b[33m"
    } else {
        "\x1b[32m"
    }
}

/// Redraws the header, spectrum and progress bar in place on a terminal.
pub struct Screen {
    analyzer: Analyzer,
    title: String,
    height: usize,
    drawn_lines: usize,
    /// When the previous redraw happened, and how far into the tune it was.
    /// The bars fall with the tune's time, not the wall clock, so a WAV render
    /// that runs faster than real time still moves like the music.
    last: Option<(Instant, f64)>,
    interval: Duration,
}

impl Screen {
    /// Two bands per character: 64 columns wide.
    pub const BANDS: usize = 128;
    pub const HEIGHT: usize = 8;

    /// `delay` is how many rendered samples sit between the render and the
    /// listener; the spectrum and clock lag the render by that much.
    pub fn new(title: String, sample_rate: f32, delay: usize) -> Self {
        Self {
            analyzer: Analyzer::new(sample_rate, Self::BANDS, 40.0, 16_000.0, delay),
            title,
            height: Self::HEIGHT,
            drawn_lines: 0,
            last: None,
            interval: Duration::from_millis(40),
        }
    }

    pub fn feed(&mut self, samples: &[f32]) {
        self.analyzer.feed(samples);
    }

    /// Redraw, unless the previous redraw was too recent and `force` is false.
    /// `played` is in seconds.
    pub fn draw(&mut self, played: f64, length: Length, force: bool) -> io::Result<()> {
        let now = Instant::now();
        let dt = match self.last {
            Some((t, _)) if !force && now.duration_since(t) < self.interval => return Ok(()),
            Some((_, before)) => (played - before).max(0.0) as f32,
            None => 0.0,
        };
        self.last = Some((now, played));
        self.analyzer.update(dt.min(0.25));

        let mut out = String::new();
        if self.drawn_lines > 0 {
            write!(out, "\x1b[{}A", self.drawn_lines).ok();
        }
        let mut lines = 0;
        let mut line = |out: &mut String, text: &str| {
            out.push_str("\r\x1b[2K");
            out.push_str(text);
            out.push('\n');
            lines += 1;
        };

        line(&mut out, &format!("\x1b[1m{}\x1b[0m", self.title));
        let rows = spectrum_rows(self.analyzer.levels(), self.analyzer.peaks(), self.height);
        for (r, row) in rows.iter().enumerate() {
            let cell = self.height - 1 - r;
            line(
                &mut out,
                &format!(" {}{}\x1b[0m", colour(cell, self.height), row),
            );
        }
        let axis_width = Self::BANDS.div_ceil(2);
        let (left, right) = ("40 Hz", "16 kHz");
        line(
            &mut out,
            &format!(
                " \x1b[2m{left}{}{right}\x1b[0m",
                " ".repeat(axis_width.saturating_sub(left.len() + right.len()))
            ),
        );
        let (fraction, total, note) = match length {
            Length::Known(t) => (played / t, clock(t), ""),
            Length::UntilSilence(cap) => (played / cap, "?:??".to_owned(), "  (until silence)"),
        };
        line(
            &mut out,
            &format!(
                " {} {} / {}{note}",
                progress_bar(fraction, axis_width),
                clock(played),
                total
            ),
        );

        self.drawn_lines = lines;
        let mut err = io::stderr().lock();
        err.write_all(out.as_bytes())?;
        err.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_progress_bar_fills_half_a_cell_at_a_time() {
        assert_eq!(progress_bar(0.0, 4), "⣀⣀⣀⣀");
        assert_eq!(progress_bar(0.375, 4), "⣿⣇⣀⣀");
        assert_eq!(progress_bar(0.5, 4), "⣿⣿⣀⣀");
        assert_eq!(progress_bar(7.0, 4), "⣿⣿⣿⣿");
    }

    #[test]
    fn the_clock_reads_minutes_and_seconds() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(65.9), "1:05");
        assert_eq!(clock(600.0), "10:00");
    }

    #[test]
    fn two_bands_share_a_character_one_per_dot_column() {
        // Two rows, eight dots tall. The left band fills 5 dots, the right 2.
        let rows = spectrum_rows(&[5.0 / 8.0, 2.0 / 8.0], &[0.0, 0.0], 2);
        // Top row: only the left band's lowest dot (dot 7).
        assert_eq!(rows[0], "⡀");
        // Bottom row: the left column full (1,2,3,7), the right's bottom two (6,8).
        assert_eq!(rows[1], "⣧");
    }

    #[test]
    fn a_peak_above_the_bar_is_one_dot() {
        // One band filling 2 of 8 dots, its peak at the 7th.
        let rows = spectrum_rows(&[2.0 / 8.0], &[7.0 / 8.0], 2);
        assert_eq!(rows[0], "⠂");
        assert_eq!(rows[1], "⡄");
    }

    #[test]
    fn an_odd_band_count_leaves_the_last_right_column_empty() {
        let rows = spectrum_rows(&[1.0, 1.0, 1.0], &[1.0, 1.0, 1.0], 1);
        assert_eq!(rows[0], "⣿⡇");
    }

    #[test]
    fn a_sine_lights_the_band_it_falls_in_and_not_the_far_ones() {
        let rate = 44_100.0;
        let mut a = Analyzer::new(rate, 128, 40.0, 16_000.0, 0);
        let tone: Vec<f32> = (0..FFT_SIZE)
            .map(|i| 0.5 * (std::f32::consts::TAU * 1000.0 * i as f32 / rate).sin())
            .collect();
        a.feed(&tone);
        a.update(0.0);
        let levels = a.levels();
        let loudest = levels
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.total_cmp(y.1))
            .unwrap()
            .0;
        // 1 kHz sits about 54% of the way up a 40 Hz to 16 kHz log axis.
        let expected = (128.0 * (1000.0f32 / 40.0).ln() / (16_000.0f32 / 40.0).ln()) as usize;
        assert!(
            loudest.abs_diff(expected) <= 2,
            "loudest band {loudest}, expected {expected}"
        );
        assert!(
            levels[loudest] > 0.85,
            "a -6 dB sine shows at {}",
            levels[loudest]
        );
        assert!(
            levels[0] < 0.2 && levels[127] < 0.2,
            "far bands lit: {levels:?}"
        );
    }

    #[test]
    fn bands_cover_increasing_bins_without_gaps_past_the_bottom() {
        let a = Analyzer::new(44_100.0, 128, 40.0, 16_000.0, 0);
        for w in a.bands.windows(2) {
            assert!(w[0].0 <= w[0].1 && w[1].0 >= w[0].0, "{:?}", a.bands);
        }
    }

    #[test]
    fn the_delay_holds_back_what_is_analysed() {
        let rate = 44_100.0;
        let mut a = Analyzer::new(rate, 128, 40.0, 16_000.0, 4096);
        let tone: Vec<f32> = (0..4096)
            .map(|i| 0.5 * (std::f32::consts::TAU * 1000.0 * i as f32 / rate).sin())
            .collect();
        a.feed(&tone);
        a.update(0.0);
        assert!(
            a.levels().iter().all(|&l| l == 0.0),
            "the tone showed before it was heard"
        );
    }
}
