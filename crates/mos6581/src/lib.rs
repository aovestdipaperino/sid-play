//! MOS 6581 SID, emulated one step per Φ2 cycle.
//!
//! The default build is GPL-free (MIT OR Apache-2.0). The off-by-default
//! `gpl-data` feature swaps in measured reSID data and makes the build GPL-2.0.
//! Libraries should never enable `gpl-data`; leave the choice to the final
//! binary. Cargo unifies features, so one dependency enabling it makes the whole
//! program GPL-2.0.

#[doc(hidden)]
pub mod chip_data;
#[cfg(not(feature = "gpl-data"))]
mod clean_chip_data;
pub mod envelope;
pub mod extfilt;
pub mod filter;
pub mod resample;
pub mod sid;
pub mod tables;
pub mod voice;
pub mod wave;

pub use sid::Sid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    Ntsc,
    Pal,
}

impl Model {
    pub fn clock_hz(self) -> f64 {
        match self {
            // 14.318182 MHz / 14
            Model::Ntsc => 1_022_727.0,
            // 17.734470 MHz / 18
            Model::Pal => 985_248.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Model;

    #[test]
    fn clock_rates_are_the_c64_ph2_rates() {
        assert_eq!(Model::Pal.clock_hz(), 985_248.0);
        assert_eq!(Model::Ntsc.clock_hz(), 1_022_727.0);
    }
}
