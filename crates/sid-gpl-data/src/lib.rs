//! Data borrowed from GPL-2.0 projects, quarantined in one crate.
//!
//! Nothing here is code: only measured tables and constants, each with its
//! attribution. `mos6581` and `sid-play` use this crate only through their
//! `gpl-data` feature. Each item is meant to be replaced by clean-room data
//! and removed; when this crate is empty it will be deleted.

#![no_std]
#![forbid(unsafe_code)]

mod chip;
mod cutoff;
mod poweron;
mod wave6581;
mod wave8580;

pub use chip::*;
pub use cutoff::{CUTOFF_POINTS_6581, CUTOFF_POINTS_8580};
pub use poweron::POWERON;
pub use wave6581::WAVE6581;
pub use wave8580::WAVE8580;

/// Four combined-waveform tables indexed by the 12-bit phase, 8-bit amplitude.
pub struct WaveData {
    /// Sawtooth + triangle.
    pub st: [u8; 4096],
    /// Pulse + triangle.
    pub p_t: [u8; 4096],
    /// Pulse + sawtooth.
    pub ps_: [u8; 4096],
    /// Pulse + sawtooth + triangle.
    pub pst: [u8; 4096],
}

#[cfg(test)]
mod tests {
    use crate::{WAVE6581, WAVE8580};

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
}
