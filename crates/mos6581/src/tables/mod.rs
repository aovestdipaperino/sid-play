//! Measured waveform tables. Every table here is a *combination* of waveforms:
//! pure sawtooth, triangle, pulse and noise are generated directly and have no
//! table, because the hardware only needs a measurement where the output bits
//! interact with each other through the resistor network.

mod fc;
mod wave6581;
mod wave8580;

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

pub use wave6581::WAVE6581;
pub use wave8580::WAVE8580;

/// The 6581 is the modelled chip, so both clock variants share its tables.
pub fn for_model(model: crate::Model) -> &'static WaveTables {
    match model {
        crate::Model::Ntsc | crate::Model::Pal => &WAVE6581,
    }
}

pub use fc::{cutoff_table, cutoff_table_8580, CutoffTable};

/// Centre frequency for an 11-bit cutoff value. Values above the register's
/// range clamp to the top rather than wrapping: a tune that writes past 2047
/// wants the filter wide open, not shut.
pub fn cutoff_hz(model: crate::Model, fc: u16) -> f64 {
    cutoff_table(model).f0[usize::from(fc.min(2047))]
}

#[cfg(test)]
mod tests {
    use super::{WAVE6581, WAVE8580};

    #[test]
    fn tables_are_4096_entries_of_8_bits() {
        for t in [&WAVE6581, &WAVE8580] {
            assert_eq!(t.st.len(), 4096);
            assert_eq!(t.p_t.len(), 4096);
            assert_eq!(t.ps_.len(), 4096);
            assert_eq!(t.pst.len(), 4096);
        }
    }

    #[test]
    fn the_6581_combination_tables_lose_amplitude() {
        // The 6581's combined waveforms swing about half the range a single
        // waveform reaches; the 8580's do not. A 6581 table that reaches 255
        // means the generator read the wrong model's files.
        for (name, table) in [
            ("st", &WAVE6581.st[..]),
            ("ps_", &WAVE6581.ps_[..]),
            ("pst", &WAVE6581.pst[..]),
        ] {
            assert_eq!(
                table.iter().max(),
                Some(&127),
                "6581 {name} does not peak at half scale"
            );
        }
        assert_eq!(
            WAVE8580.pst.iter().max(),
            Some(&255),
            "8580 pst is not full scale"
        );
    }

    #[test]
    fn the_triangle_combination_rises_then_falls() {
        let pt = &WAVE6581.p_t;
        assert!(pt[0] < pt[2047], "no rising half");
        assert!(pt[2047] > pt[4095], "no falling half");
    }

    #[test]
    fn the_combination_tables_are_distinct() {
        // Guards against the generator reading one .dat file four times.
        let groups = [&WAVE6581.st, &WAVE6581.p_t, &WAVE6581.ps_, &WAVE6581.pst];
        for (i, a) in groups.iter().enumerate() {
            for (j, b) in groups.iter().enumerate() {
                if i != j {
                    assert_ne!(
                        a.as_slice(),
                        b.as_slice(),
                        "tables {i} and {j} are identical"
                    );
                }
            }
        }
        assert_ne!(WAVE6581.pst.as_slice(), WAVE8580.pst.as_slice());
    }

    #[test]
    fn both_clock_variants_share_the_6581_tables() {
        let a = super::for_model(crate::Model::Pal);
        let b = super::for_model(crate::Model::Ntsc);
        assert!(std::ptr::eq(a, b), "the two clock rates resolved apart");
        assert!(!std::ptr::eq(&WAVE6581, &WAVE8580));
    }
}
