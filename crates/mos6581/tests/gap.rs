//! How far the built-in tables are from the measured ones. Only meaningful
//! with `--features gpl-data`, where both are available; prints the numbers
//! and checks nothing beyond their sanity, so the gap is recorded, not hidden.

#![cfg(feature = "gpl-data")]

use mos6581::tables::{WAVE6581, default::WAVE_AND};

fn gap(name: &str, a: &[u8; 4096], b: &[u8; 4096]) {
    let mut sq = 0.0f64;
    let mut max = 0u8;
    for (x, y) in a.iter().zip(b) {
        let d = x.abs_diff(*y);
        max = max.max(d);
        sq += f64::from(d) * f64::from(d);
    }
    let rms = (sq / 4096.0).sqrt();
    println!("GAP {name}: rms {rms:.1} max {max} (of 255)");
    assert!(rms.is_finite());
}

#[test]
fn print_the_table_gap() {
    gap("st", &WAVE_AND.st, &WAVE6581.st);
    gap("p_t", &WAVE_AND.p_t, &WAVE6581.p_t);
    gap("ps_", &WAVE_AND.ps_, &WAVE6581.ps_);
    gap("pst", &WAVE_AND.pst, &WAVE6581.pst);
}
