use mos6581::Model;
use mos6581::tables;
use mos6581::wave::{self, Wave};

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
fn the_combinations_are_mutually_distinct() {
    // The built-in AND tables make the three-way table equal saw+triangle by
    // construction, so only the measured tables keep all four apart.
    let combos = [
        wave::SAWTOOTH | wave::TRIANGLE,
        wave::PULSE | wave::TRIANGLE,
        wave::PULSE | wave::SAWTOOTH,
        #[cfg(feature = "gpl-data")]
        (wave::PULSE | wave::SAWTOOTH | wave::TRIANGLE),
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

#[test]
fn noise_rate_follows_the_frequency_register() {
    // The shift register is clocked by an accumulator bit, so a higher frequency
    // register must produce more shifts, not the same number.
    let mut slow = wave_obj();
    let mut fast = wave_obj();
    for _ in 0..200_000 {
        slow.tick(16, 0, wave::NOISE, false);
        fast.tick(4096, 0, wave::NOISE, false);
    }
    assert!(
        fast.noise_shifts() > slow.noise_shifts() * 8,
        "slow {} fast {}",
        slow.noise_shifts(),
        fast.noise_shifts()
    );
}

#[test]
fn noise_is_loud_symmetric_about_mid_scale_and_reproducible() {
    // The waveform output is unsigned 12-bit like every other shape; centring to
    // zero happens later, in the voice's amplitude stage. FREQ is high enough that
    // the clock bit rises the register thousands of times over the sample, so this
    // measures the shift register rather than its seed value.
    let mut a = wave_obj();
    let mut b = wave_obj();
    let mut sum = 0i64;
    let mut squares = 0i64;
    let n = 2_000_000;
    let mut same = true;
    for _ in 0..n {
        let x = a.tick(32_768, 0, wave::NOISE, false);
        let y = b.tick(32_768, 0, wave::NOISE, false);
        if x != y {
            same = false;
        }
        let centred = i64::from(x) - 2048;
        sum += centred;
        squares += centred * centred;
    }
    let mean = sum as f64 / n as f64;
    let rms = (squares as f64 / n as f64).sqrt();
    assert!(same, "noise is not deterministic");
    assert!(a.noise_shifts() > 10_000, "the register barely moved");
    assert!(rms > 800.0, "noise is quiet: rms {rms}");
    assert!(
        mean.abs() < rms * 0.1,
        "noise is not centred: mean {mean} rms {rms}"
    );
}

#[test]
fn the_random_register_moves_and_reads_back_as_a_byte() {
    let mut w = wave_obj();
    let mut seen = [0u8; 512];
    let mut distinct = 0usize;
    for _ in 0..4096 {
        w.tick(4096, 0, wave::NOISE, false);
        let b = w.random_byte();
        let slot = &mut seen[usize::from(b)];
        if *slot == 0 {
            *slot = 1;
            distinct += 1;
        }
    }
    assert!(distinct > 1, "the noise register never moved");
}

#[test]
fn sync_zeroes_the_slave_on_the_masters_wrap() {
    let mut master = wave_obj();
    let mut slave = wave_obj();
    let mut unsynced = wave_obj();
    let mut locked = 0;
    let mut slave_max = 0u32;
    let mut unsynced_max = 0u32;
    for _ in 0..4096 {
        master.tick(32_768, 0, wave::SAWTOOTH, false);
        slave.tick(1_000, 0, wave::SAWTOOTH, false);
        unsynced.tick(1_000, 0, wave::SAWTOOTH, false);
        if master.msb_rising() {
            slave.force_phase_zero();
            assert_eq!(
                slave.accumulator(),
                0,
                "force_phase_zero left a phase behind"
            );
            locked += 1;
        }
        slave_max = slave_max.max(slave.accumulator());
        unsynced_max = unsynced_max.max(unsynced.accumulator());
    }
    assert!(locked >= 4, "master wrapped only {locked} times");
    assert!(
        slave_max < unsynced_max / 4,
        "synced slave still ran away: max {slave_max} against unsynced {unsynced_max}"
    );
}

#[test]
fn ring_mod_reads_the_sources_phase_not_its_output() {
    // A pulse source with pw = 4095 outputs zero for all but one phase in 4096, so
    // an implementation that XORed the source's output bit 11 would essentially
    // never ring-modulate. Reading the source's accumulator must still flip the
    // destination's triangle sign twice per source cycle.
    let mut source = wave_obj();
    let mut dest = wave_obj();
    let mut plain = wave_obj();
    let mut with_ring = Vec::new();
    let mut without = Vec::new();
    for _ in 0..4096 {
        source.tick(2048, 4095, wave::PULSE, false);
        let msb = source.accumulator() & 0x0080_0000 != 0;
        with_ring.push(dest.tick(256, 0, wave::TRIANGLE | wave::RING_MOD, msb));
        without.push(plain.tick(256, 0, wave::TRIANGLE, false));
    }
    assert!(
        with_ring != without,
        "ring modulation had no effect against a nearly silent pulse source"
    );
    assert!(
        without.windows(2).any(|p| p[0] != p[1]),
        "the control oscillator produced nothing to compare against"
    );
}

#[test]
fn toggling_test_resets_the_noise_shift_register() {
    let mut w = wave_obj();

    // Rising edge first: applying test clears the register, and with the accumulator
    // pinned there is nothing to clock it, so it stays put.
    for _ in 0..64 {
        w.tick(32_768, 0, wave::NOISE | wave::TEST, false);
        assert!(!w.msb_rising(), "test held must not produce sync edges");
    }
    assert_eq!(
        w.random_byte(),
        0,
        "setting test must clear the shift register"
    );
    let shifts_while_tested = w.noise_shifts();
    assert_eq!(
        shifts_while_tested, 0,
        "the register shifted while test was held"
    );

    // Falling edge: the register reloads to its start value.
    w.tick(0, 0, wave::NOISE, false);
    assert_eq!(
        w.random_byte(),
        0xff,
        "clearing test must reload the shift register"
    );

    // Prove the register is live again, then re-apply test so the clear above cannot
    // have passed by the register already sitting at zero.
    let mut guard = 0;
    while w.random_byte() == 0xff && guard < 100_000 {
        w.tick(32_768, 0, wave::NOISE, false);
        guard += 1;
    }
    assert!(
        w.noise_shifts() > shifts_while_tested,
        "the register never resumed after test was released"
    );
    assert_ne!(
        w.random_byte(),
        0xff,
        "register never moved off its reload value"
    );
    let moved = w.random_byte();

    w.tick(32_768, 0, wave::NOISE | wave::TEST, false);
    assert_eq!(
        w.random_byte(),
        0,
        "re-applying test did not clear a register that had moved (was {moved:#04x})"
    );
}

use mos6581::envelope::Adsr;
use mos6581::voice::{FULL_SCALE, Voice};

fn voice() -> Voice {
    Voice::new(tables::for_model(Model::Pal))
}

const FAST: Adsr = Adsr {
    attack: 0x0,
    decay: 0x0,
    sustain: 0xf,
    release: 0x0,
};

#[test]
fn a_gated_sawtooth_reaches_full_amplitude() {
    let mut v = voice();
    let mut peak = 0i32;
    for _ in 0..200_000 {
        peak = peak.max(
            v.tick(1000, 0, wave::SAWTOOTH | wave::GATE, FAST, false)
                .abs(),
        );
    }
    assert!(peak > 400_000, "voice peak is only {peak}");
}

#[test]
fn an_ungated_voice_carries_the_dc_and_no_signal() {
    // Silence for an idle voice is achieved downstream: the output stage's
    // high-pass throws the DC away. What the voice itself must guarantee is that
    // it produces no *signal* - a flat line at the DC term.
    let mut v = voice();
    let first = v.tick(1000, 0, wave::SAWTOOTH, FAST, false);
    for _ in 0..10_000 {
        assert_eq!(
            v.tick(1000, 0, wave::SAWTOOTH, FAST, false),
            first,
            "an ungated voice is generating a signal"
        );
    }
    // The DC term is a borrowed value, so its size is only checked with the data.
    #[cfg(feature = "gpl-data")]
    assert_eq!(
        first,
        sid_gpl_data::VOICE_DC,
        "idle voice is not sitting at the DC term"
    );
}

// The measured zero level sits off-centre; the built-in model is centred.
#[cfg(feature = "gpl-data")]
#[test]
fn the_output_is_asymmetric_as_the_hardware_is() {
    // The waveform's zero level sits well below the middle of its 12-bit range, so a
    // full-swing voice swings far further positive than negative about its own DC
    // point. A model that centred the waveform would swing symmetrically about zero
    // and cross it; the hardware's voice never goes negative at full envelope.
    //
    // Two things the obvious version of this test gets wrong:
    //   - measuring from the first cycle includes the attack ramp, where the voice
    //     sits at the DC term, so the ratio check below can pass on a model whose
    //     zero level is wrong;
    //   - a low FREQ never walks the oscillator out of the bottom of its range.
    // So: charge the envelope to full first, then walk the whole range at one phase
    // per cycle.
    let mut v = voice();
    let mut guard = 0;
    while v.envelope_level() != 255 && guard < 100_000 {
        v.tick(4096, 0, wave::SAWTOOTH | wave::GATE, FAST, false);
        guard += 1;
    }
    assert_eq!(v.envelope_level(), 255, "envelope never reached full level");

    let mut lo = i32::MAX;
    let mut hi = i32::MIN;
    for _ in 0..8192 {
        let s = v.tick(4096, 0, wave::SAWTOOTH | wave::GATE, FAST, false);
        lo = lo.min(s);
        hi = hi.max(s);
    }
    assert!(hi > lo, "flat output: {lo}..{hi}");
    assert!(
        lo > 0,
        "output crosses zero, so the waveform is centred rather than hardware-offset: {lo}..{hi}"
    );
    // The floor of the swing is the giveaway. A model that put the zero level in
    // the middle of the range has almost the whole of its negative excursion
    // cancelled by the DC term instead, and its floor collapses towards zero while
    // its peak stays high.
    assert!(
        lo * 8 > hi,
        "floor is {lo} against a peak of {hi}: the waveform's zero level looks centred"
    );
}

#[test]
fn normalising_a_full_voice_lands_in_range() {
    let mut v = voice();
    let mut peak = 0i32;
    for _ in 0..200_000 {
        peak = peak.max(
            v.tick(1000, 0, wave::SAWTOOTH | wave::GATE, FAST, false)
                .abs(),
        );
    }
    let n = f64::from(peak) / FULL_SCALE;
    assert!((0.5..=2.0).contains(&n), "normalised peak {n} is off scale");
}

#[test]
fn release_takes_the_signal_swing_down_to_nothing() {
    // The DC term never goes away - it is what keeps the output continuous - so
    // "got quieter" has to be measured as the peak-to-peak swing, not as the
    // magnitude of the output.
    let mut v = voice();
    let charge = Adsr {
        attack: 0x0,
        decay: 0x0,
        sustain: 0xf,
        release: 0x0,
    };
    let swing_while_gated = {
        for _ in 0..50_000 {
            v.tick(1000, 0, wave::SAWTOOTH | wave::GATE, charge, false);
        }
        let mut lo = i32::MAX;
        let mut hi = i32::MIN;
        for _ in 0..200_000 {
            let s = v.tick(1000, 0, wave::SAWTOOTH | wave::GATE, charge, false);
            lo = lo.min(s);
            hi = hi.max(s);
        }
        hi - lo
    };
    assert!(
        swing_while_gated > 400_000,
        "never got loud: {swing_while_gated}"
    );

    let off = Adsr {
        attack: 0x0,
        decay: 0x0,
        sustain: 0xf,
        release: 0x2,
    };
    // Let the release finish before measuring. A falling sweep at rate nibble 2
    // takes well under 100_000 cycles, so that is comfortably past the end; measuring across the ramp would just re-measure how loud it was.
    for _ in 0..100_000 {
        v.tick(1000, 0, wave::SAWTOOTH, off, false);
    }
    assert_eq!(v.envelope_level(), 0, "release never reached silence");
    let mut lo = i32::MAX;
    let mut hi = i32::MIN;
    for _ in 0..200_000 {
        let s = v.tick(1000, 0, wave::SAWTOOTH, off, false);
        lo = lo.min(s);
        hi = hi.max(s);
    }
    let swing_after_release = hi - lo;
    assert!(
        swing_after_release < swing_while_gated / 20,
        "release left a swing of {swing_after_release} against {swing_while_gated} while gated"
    );
    assert_eq!(
        hi, lo,
        "a released voice should sit flat at the DC term, not drift: {lo}..{hi}"
    );
}
