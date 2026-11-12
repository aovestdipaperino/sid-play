//! The chip: register file, per-cycle order, mixer and output.

use crate::envelope::Adsr;
use crate::extfilt::ExtFilter;
use crate::filter::Filter;
use crate::resample::{Method, ResampleError, Resampler};
use crate::tables;
use crate::voice::{Voice, FULL_SCALE};
use crate::wave::{OSC_SYNC, RING_MOD};
use crate::Model;

/// Registers at `$d400..=$d41f`.
pub const REG_COUNT: usize = 32;

/// The mixer sums up to three voices, each of which swings about one unit of
/// [`FULL_SCALE`] of signal on top of its own DC term, so the raw mix peaks near
/// three. Dividing by the number of voices keeps a full three-voice mix inside
/// the plus or minus one that the output path assumes.
const MIX_SCALE: f64 = 1.0 / 3.0;

/// Host rate used when the caller asks for something that cannot be turned into
/// a usable resampler at all. Both clock rates accept it.
const FALLBACK_RATE: f64 = 44_100.0;

/// Pull a requested host sample rate into the window the resampler can work with:
/// finite, positive, and never above the chip clock, which the resampler refuses
/// because there would be nothing to decimate.
fn clamp_rate(requested: f64, clock_hz: f64) -> f64 {
    if !requested.is_finite() || requested <= 0.0 {
        return FALLBACK_RATE.min(clock_hz);
    }
    requested.min(clock_hz)
}

pub struct Sid {
    regs: [u8; REG_COUNT],
    voices: [Voice; 3],
    filter: Filter,
    extfilt: ExtFilter,
    resampler: Resampler,
    /// Kept so a reset can rebuild the resampler: its filter history holds
    /// roughly 124 host samples of whatever the chip was playing when it was
    /// reset, and leaving it in place would keep that audio coming out.
    rate: f64,
    clock_hz: f64,
}

impl Sid {
    /// A chip at the requested host sample rate. Rates that cannot produce a
    /// usable resampler are clamped; see [`Sid::try_new`] for the fallible form.
    pub fn new(model: Model, sample_rate: f64) -> Self {
        let clock_hz = model.clock_hz();
        let rate = clamp_rate(sample_rate, clock_hz);
        // The fallback rate is a constant against a constant clock, so this arm
        // does not depend on anything the caller supplied.
        Self::try_new(model, rate)
            .or_else(|_| Self::try_new(model, FALLBACK_RATE.min(clock_hz)))
            .expect("the fallback rate is accepted at both clock rates")
    }

    /// A chip that reports a host sample rate it cannot resample to, rather than
    /// quietly substituting one.
    pub fn try_new(model: Model, sample_rate: f64) -> Result<Self, ResampleError> {
        let clock_hz = model.clock_hz();
        let tables = tables::for_model(model);
        let mut sid = Self {
            regs: [0; REG_COUNT],
            voices: [Voice::new(tables), Voice::new(tables), Voice::new(tables)],
            filter: Filter::new(model),
            extfilt: ExtFilter::new(),
            resampler: Resampler::new(clock_hz, sample_rate, Method::ResampleInterpolate)?,
            rate: sample_rate,
            clock_hz,
        };
        sid.sync_filter();
        Ok(sid)
    }

    #[inline]
    pub fn clock_hz(&self) -> f64 {
        self.clock_hz
    }

    pub fn write(&mut self, addr: usize, value: u8) {
        if addr >= REG_COUNT {
            return;
        }
        self.regs[addr] = value;
        match addr {
            21 | 22 => self.sync_cutoff(),
            // `$d417` carries the resonance nibble and the routing mask together, so
            // a write here moves both of them.
            23 => {
                self.sync_cutoff();
                self.sync_mode();
            }
            24 => self.sync_mode(),
            _ => {}
        }
    }

    pub fn read(&self, addr: usize) -> u8 {
        match addr {
            25 | 26 => 0xff,
            27 => self.voices[2].random_byte(),
            28 => self.voices[2].envelope_level(),
            29..=31 => 0xff,
            _ if addr < REG_COUNT => self.regs[addr],
            _ => 0xff,
        }
    }

    pub fn set_ext_in(&mut self, sample: f32) {
        self.filter.set_ext_in(f64::from(sample));
    }

    /// Fill `out` with host samples, advancing the chip as needed.
    pub fn render(&mut self, out: &mut [f32]) {
        for slot in out.iter_mut() {
            loop {
                self.tick();
                if let Some(s) = self.resampler.output() {
                    *slot = s as f32;
                    break;
                }
            }
        }
    }

    /// Advance exactly `cycles` Phi 2 cycles, collecting the audio they produce.
    pub fn render_cycles(&mut self, cycles: u64, out: &mut Vec<f32>) {
        for _ in 0..cycles {
            self.tick();
            if let Some(s) = self.resampler.output() {
                out.push(s as f32);
            }
        }
    }

    fn tick(&mut self) {
        // Ring modulation reads the source oscillator's phase before anything moves.
        let source_msb = [
            self.voices[2].wave_bit23(),
            self.voices[0].wave_bit23(),
            self.voices[1].wave_bit23(),
        ];

        for (i, &msb) in source_msb.iter().enumerate() {
            let base = i * 7;
            let freq = u16::from_le_bytes([self.regs[base], self.regs[base + 1]]);
            let pw = ((u16::from(self.regs[base + 3] & 0x0f) << 8)
                | u16::from(self.regs[base + 2]))
            .min(4095);
            let ctrl = self.regs[base + 4];
            let adsr = Adsr::from_regs(self.regs[base + 5], self.regs[base + 6]);
            let ring = (ctrl & RING_MOD) != 0;
            self.voices[i].tick(freq, pw, ctrl, adsr, ring && msb);
        }

        // Hard sync, in hardware order, skipping a source that is itself being
        // zeroed in the same cycle.
        let rising = [
            self.voices[2].msb_rising(),
            self.voices[0].msb_rising(),
            self.voices[1].msb_rising(),
        ];
        let syncing = [
            (self.regs[4] & OSC_SYNC) != 0,
            (self.regs[11] & OSC_SYNC) != 0,
            (self.regs[18] & OSC_SYNC) != 0,
        ];
        // `rising[k]` is the MSB-rise flag of voice k's *source*, so the slave at
        // index i is reset when `rising[i]` fires, and the source that is itself
        // being zeroed this cycle is the one `rising[src]` describes.
        for i in 0..3 {
            let src = (i + 2) % 3;
            if syncing[i] && rising[i] && !(syncing[src] && rising[src]) {
                self.voices[i].force_phase_zero();
            }
        }

        let v1 = f64::from(self.voices[0].output()) / FULL_SCALE;
        let v2 = f64::from(self.voices[1].output()) / FULL_SCALE;
        let v3 = f64::from(self.voices[2].output()) / FULL_SCALE;
        self.filter.clock(v1, v2, v3);
        self.extfilt.clock(self.filter.output());
        // The voice DC terms are left in the signal on purpose: the output stage's
        // high pass is what makes an idle voice inaudible, and taking the offset
        // out here would put a step back on every envelope edge.
        self.resampler.input(self.extfilt.output() * MIX_SCALE);
    }

    fn sync_cutoff(&mut self) {
        let fc = ((u16::from(self.regs[22]) << 3) | u16::from(self.regs[21] & 0x07)).min(2047);
        self.filter.set_fc(fc, self.clock_hz);
        self.filter.set_resonance(self.regs[23] >> 4);
    }

    fn sync_mode(&mut self) {
        let routing = self.regs[23] & 0x0f;
        let taps = self.regs[24] >> 4;
        self.filter.set_mode(routing, taps);
        self.filter.set_voice3_off((self.regs[24] & 0x80) != 0);
        self.filter.set_volume(self.regs[24] & 0x0f);
    }

    fn sync_filter(&mut self) {
        self.sync_cutoff();
        self.sync_mode();
    }

    pub fn reset(&mut self) {
        self.regs = [0; REG_COUNT];
        for v in &mut self.voices {
            v.reset();
        }
        self.filter.reset();
        self.extfilt.reset();
        // The rate was accepted once already, so this cannot fail; if it somehow
        // did, the existing resampler is left in place rather than panicking.
        if let Ok(fresh) = Resampler::new(self.clock_hz, self.rate, Method::ResampleInterpolate) {
            self.resampler = fresh;
        }
        // The filter keeps its volume, routing and taps across a reset, so the
        // chip has to re-apply them from the now-zeroed register file itself.
        self.sync_filter();
    }
}
