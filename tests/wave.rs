use rust_sid::Model;
use rust_sid::tables;
use rust_sid::wave::{self, Wave};

fn wave_obj() -> Wave {
    Wave::new(tables::for_model(Model::Pal))
}

/// Walk the oscillator once around its full 12-bit output. FREQ 4096 advances the
/// 24-bit accumulator by exactly one phase index per cycle, so 4096 steps visit
/// every phase exactly once.
fn sweep(w: &mut Wave, ctrl: u8, pw: u16) -> Vec<u16> {
    (0..4096).map(|_| w.tick(4096, pw, ctrl, false)).collect()
}

/// The triangle shape the oscillator is expected to produce for an accumulator.
fn triangle_of(acc: u32) -> u16 {
    let shaped = if acc & 0x0080_0000 != 0 { !acc } else { acc };
    ((shaped >> 11) & 0x0fff) as u16
}

#[test]
fn no_waveform_selected_is_silence() {
    let mut w = wave_obj();
    assert!(sweep(&mut w, 0, 0).iter().all(|&v| v == 0));
}

#[test]
fn sawtooth_walks_the_full_12_bit_range() {
    let mut w = wave_obj();
    let v = sweep(&mut w, wave::SAWTOOTH, 0);
    assert!(v.contains(&0), "no zero in the ramp");
    assert!(v.contains(&4095), "no full scale in the ramp");
    // A ramp that wraps exactly once: every step is +1 except the single reset.
    let resets = v.windows(2).filter(|p| p[1] != p[0] + 1).count();
    assert_eq!(resets, 1, "saw is not a single wrapping ramp");
    assert_eq!(v.iter().filter(|&&s| s == 0).count(), 1);
}

#[test]
fn pulse_width_sets_the_duty_cycle() {
    for pw in [0u16, 1024, 3072, 4095] {
        let mut w = wave_obj();
        let v = sweep(&mut w, wave::PULSE, pw);
        let high = v.iter().filter(|&&s| s == 0x0fff).count();
        assert_eq!(high, 4096 - usize::from(pw), "pw {pw}");
    }
}

#[test]
fn test_bit_holds_the_accumulator_and_forces_pulse_high() {
    let mut w = wave_obj();
    w.tick(500, 2048, wave::SAWTOOTH | wave::TEST, false);
    assert_eq!(w.accumulator(), 0, "test must hold the accumulator at zero");
    let mut w2 = wave_obj();
    let v = sweep(&mut w2, wave::PULSE | wave::TEST, 2048);
    assert!(
        v.iter().all(|&s| s == 0x0fff),
        "pulse under test must be high"
    );
}

#[test]
fn combined_waveforms_come_from_the_measurement_tables() {
    // Pulse+triangle is not the bitwise AND of the two generated shapes: the
    // measured table loses amplitude to the resistor network.
    let mut w = wave_obj();
    let combo = sweep(&mut w, wave::PULSE | wave::TRIANGLE, 0);
    let max_combo = *combo.iter().max().unwrap();
    let mut w2 = wave_obj();
    let tri = sweep(&mut w2, wave::TRIANGLE, 0);
    let max_tri = *tri.iter().max().unwrap();
    assert!(
        max_combo < max_tri,
        "pulse+triangle reached {max_combo} against triangle {max_tri}; \
         it is not coming from the measured table"
    );
}

#[test]
fn the_waveform_is_a_12_bit_value() {
    let mut w = wave_obj();
    assert!(
        sweep(&mut w, wave::SAWTOOTH | wave::PULSE, 1500)
            .iter()
            .all(|&v| v <= 0x0fff)
    );
}

/// The guard against the or-pattern failure: every selection must reach its own
/// table, not fall through to the last arm. Each accumulator is set directly and
/// stepped with FREQ 0, so the phase is known exactly and the expected value can
/// be read out of the table the selection is supposed to use.
#[test]
fn each_waveform_selection_reads_its_own_table() {
    let t = tables::for_model(Model::Pal);
    // Accumulators below and above the sign bit, so both halves of the triangle
    // and both sides of the pulse comparator are covered.
    let accs: [u32; 6] = [
        0x0000_0000,
        0x0012_3456,
        0x007f_ffff,
        0x0080_0000,
        0x00ab_cdef,
        0x00ff_fff0,
    ];

    for acc in accs {
        let phase = (acc >> 12) as usize;
        let tri = triangle_of(acc);
        let pw = 2048;
        let pulse_high = phase as u16 >= pw;
        let mask = if pulse_high { 0x0fff } else { 0 };

        let check = |ctrl: u8, want: u16, name: &str| {
            let mut w = wave_obj();
            w.set_accumulator(acc);
            let got = w.tick(0, pw, ctrl, false);
            assert_eq!(got, want, "acc {acc:#x} selection {name}");
        };

        check(wave::SAWTOOTH, phase as u16, "saw");
        check(wave::TRIANGLE, tri, "triangle");
        check(wave::PULSE, if pulse_high { 0x0fff } else { 0 }, "pulse");
        check(
            wave::SAWTOOTH | wave::TRIANGLE,
            u16::from(t.st[phase]) << 4,
            "saw+triangle -> st",
        );
        check(
            wave::PULSE | wave::TRIANGLE,
            (u16::from(t.p_t[usize::from(tri >> 1)]) << 4) & mask,
            "pulse+triangle -> p_t",
        );
        check(
            wave::PULSE | wave::SAWTOOTH,
            (u16::from(t.ps_[phase]) << 4) & mask,
            "pulse+saw -> ps_",
        );
        check(
            wave::PULSE | wave::SAWTOOTH | wave::TRIANGLE,
            (u16::from(t.pst[phase]) << 4) & mask,
            "pulse+saw+triangle -> pst",
        );
    }
}

/// If the combination arms were written as or-patterns they would all fall through
/// to the same table, so the four combinations would read identically. They must not.
#[test]
fn the_four_combinations_are_mutually_distinct() {
    let combos = [
        wave::SAWTOOTH | wave::TRIANGLE,
        wave::PULSE | wave::TRIANGLE,
        wave::PULSE | wave::SAWTOOTH,
        wave::PULSE | wave::SAWTOOTH | wave::TRIANGLE,
    ];
    let mut seen: Vec<Vec<u16>> = Vec::new();
    for ctrl in combos {
        let mut w = wave_obj();
        let v = sweep(&mut w, ctrl, 0);
        assert!(
            seen.iter().all(|other| *other != v),
            "two waveform selections produced identical output for {ctrl:#04x}"
        );
        seen.push(v);
    }
}

#[test]
fn noise_overrides_the_other_selections() {
    let mut w = wave_obj();
    let mut n = wave_obj();
    for i in 0..600u32 {
        w.set_accumulator(i << 12);
        n.set_accumulator(i << 12);
        let combo = w.tick(
            0,
            2048,
            wave::NOISE | wave::SAWTOOTH | wave::TRIANGLE,
            false,
        );
        let alone = n.tick(0, 2048, wave::NOISE, false);
        assert_eq!(combo, alone, "noise+others did not resolve to noise");
    }
}
