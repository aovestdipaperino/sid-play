//! Measured waveform tables. Every table here is a *combination* of waveforms:
//! pure sawtooth, triangle, pulse and noise are generated directly and have no
//! table, because the hardware only needs a measurement where the output bits
//! interact with each other through the resistor network.

mod fc;

pub mod default;

/// Four combination waveforms, indexed by the 12-bit phase, each an 8-bit
/// amplitude that the oscillator scales up to its 12-bit output range.
pub struct WaveTables {
    /// Sawtooth + triangle.
    pub st: [u8; 4096],
    /// Pulse + triangle.
    pub p_t: [u8; 4096],
    /// Pulse + sawtooth.
    pub ps_: [u8; 4096],
    /// Pulse + sawtooth + triangle.
    pub pst: [u8; 4096],
}

#[cfg(feature = "gpl-data")]
const fn from_data(d: sid_gpl_data::WaveData) -> WaveTables {
    WaveTables {
        st: d.st,
        p_t: d.p_t,
        ps_: d.ps_,
        pst: d.pst,
    }
}

#[cfg(feature = "gpl-data")]
/// Measured 6581 combined waveforms (reSID data, GPL-2.0, via `sid-gpl-data`).
pub static WAVE6581: WaveTables = from_data(sid_gpl_data::WAVE6581);
#[cfg(feature = "gpl-data")]
/// Measured 8580 combined waveforms (reSID data, GPL-2.0, via `sid-gpl-data`).
pub static WAVE8580: WaveTables = from_data(sid_gpl_data::WAVE8580);

/// The 6581 is the modelled chip, so both clock variants share its tables.
pub fn for_model(model: crate::Model) -> &'static WaveTables {
    match model {
        #[cfg(feature = "gpl-data")]
        crate::Model::Ntsc | crate::Model::Pal => &WAVE6581,
        #[cfg(not(feature = "gpl-data"))]
        crate::Model::Ntsc | crate::Model::Pal => &default::WAVE_AND,
    }
}

pub use fc::{CutoffTable, cutoff_table, cutoff_table_8580};

/// Centre frequency for an 11-bit cutoff value. Values above the register's
/// range clamp to the top rather than wrapping: a tune that writes past 2047
/// wants the filter wide open, not shut.
pub fn cutoff_hz(model: crate::Model, fc: u16) -> f64 {
    cutoff_table(model).f0[usize::from(fc.min(2047))]
}

#[cfg(test)]
mod tests {
    #[test]
    fn both_clock_variants_share_the_6581_tables() {
        let a = super::for_model(crate::Model::Pal);
        let b = super::for_model(crate::Model::Ntsc);
        assert!(std::ptr::eq(a, b), "the two clock rates resolved apart");
        #[cfg(feature = "gpl-data")]
        assert!(!std::ptr::eq(&super::WAVE6581, &super::WAVE8580));
    }
}
