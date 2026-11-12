use mos6581::Model;
use mos6581::tables::cutoff_hz;

// Asserts facts of the measured curve, which needs the borrowed data.
#[cfg(feature = "gpl-data")]
#[test]
fn cutoff_spans_the_measured_6581_range() {
    let points = sid_gpl_data::CUTOFF_POINTS_6581;
    let (first, last) = (points[0].1, points[points.len() - 1].1);
    let lo = cutoff_hz(Model::Pal, 0);
    let hi = cutoff_hz(Model::Pal, 2047);
    assert!(
        (lo - first).abs() < 1e-6,
        "fc=0 gave {lo} Hz, data says {first}"
    );
    assert!(
        (hi - last).abs() < 1e-6,
        "fc=2047 gave {hi} Hz, data says {last}"
    );
}

#[cfg(not(feature = "gpl-data"))]
#[test]
fn the_default_cutoff_is_the_datasheet_linear_range() {
    let lo = cutoff_hz(Model::Pal, 0);
    let hi = cutoff_hz(Model::Pal, 2047);
    assert!((lo - 30.0).abs() < 0.5, "fc=0 gave {lo} Hz");
    assert!((hi - 12_000.0).abs() < 1.0, "fc=2047 gave {hi} Hz");
    let mid = cutoff_hz(Model::Pal, 1024);
    let linear = 30.0 + (12_000.0 - 30.0) * 1024.0 / 2047.0;
    assert!(
        (mid - linear).abs() < 1.0,
        "fc=1024 gave {mid} Hz, linear is {linear}"
    );
}

// The measured curve has a step the built-in linear one lacks.
#[cfg(feature = "gpl-data")]
#[test]
fn cutoff_rises_monotonically_except_at_the_top_of_the_low_half() {
    // The measured curve has a discontinuity where the high nibble of the cutoff
    // register rolls over: the corner drops, then climbs again.
    let a = cutoff_hz(Model::Pal, 1023);
    let b = cutoff_hz(Model::Pal, 1024);
    assert!(b < a, "expected a drop at fc=1024, got {a} then {b}");

    let mut prev = cutoff_hz(Model::Pal, 1024);
    for fc in 1025..=2047 {
        let now = cutoff_hz(Model::Pal, fc);
        assert!(now >= prev, "not monotone above the discontinuity at {fc}");
        prev = now;
    }
}

#[test]
fn cutoff_is_clamped_to_11_bits() {
    assert_eq!(cutoff_hz(Model::Pal, 2048), cutoff_hz(Model::Pal, 2047));
    assert_eq!(cutoff_hz(Model::Pal, 9999), cutoff_hz(Model::Pal, 2047));
}

use mos6581::filter::{
    Filter, ROUTE_VOICE_1, ROUTE_VOICE_3, TAP_HIGH_PASS, TAP_LOW_PASS, TAP_NONE,
};

const CLOCK: f64 = 985_248.0;

/// The mixer always carries DC, so "how much tone came through" is the
/// mean-subtracted RMS. Measuring plain RMS would put a floor under every
/// attenuation assertion and let a dead filter pass as an attenuating one.
fn ac_rms(v: &[f64]) -> f64 {
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    (v.iter().map(|s| (s - mean) * (s - mean)).sum::<f64>() / v.len() as f64).sqrt()
}

/// One Φ2 cycle of a unit-amplitude tone, fed to the voice slot named.
fn drive(f: &mut Filter, hz: f64, phase: &mut f64, slot: u8) {
    let s = (2.0 * std::f64::consts::PI * hz * *phase / CLOCK).sin();
    *phase += 1.0;
    match slot {
        2 => f.clock(0.0, s, 0.0),
        3 => f.clock(0.0, 0.0, s),
        _ => f.clock(s, 0.0, 0.0),
    }
}

fn run(fc: u16, taps: u8, hz: f64) -> f64 {
    let mut f = Filter::new(Model::Pal);
    f.set_fc(fc, CLOCK);
    f.set_resonance(0);
    f.set_mode(ROUTE_VOICE_1, taps);
    f.set_volume(15);
    let mut phase = 0.0;
    let mut out = Vec::with_capacity(40_000);
    for _ in 0..40_000 {
        drive(&mut f, hz, &mut phase, 1);
        out.push(f.output());
    }
    ac_rms(&out[out.len() / 2..])
}

#[test]
fn a_closed_low_pass_attenuates_a_tone_an_open_one_passes() {
    let open = run(2047, TAP_LOW_PASS, 2000.0);
    let closed = run(0, TAP_LOW_PASS, 2000.0);
    assert!(open > 0.2, "nothing came through at full cutoff: {open}");
    assert!(
        closed < open / 10.0,
        "cutoff did nothing: closed {closed} open {open}"
    );
}

#[test]
fn the_high_pass_tap_passes_what_the_low_pass_stops() {
    let lp = run(0, TAP_LOW_PASS, 4000.0);
    let hp = run(0, TAP_HIGH_PASS, 4000.0);
    assert!(
        hp > lp * 10.0,
        "lp {lp} hp {hp} at 4 kHz with the cutoff closed"
    );
}

#[test]
fn resonance_boosts_output_at_the_corner() {
    let measure = |res: u8| {
        let mut f = Filter::new(Model::Pal);
        f.set_fc(768, CLOCK);
        f.set_resonance(res);
        f.set_mode(ROUTE_VOICE_1, TAP_LOW_PASS);
        f.set_volume(15);
        let mut p = 0.0;
        let mut o = Vec::with_capacity(40_000);
        for _ in 0..40_000 {
            drive(&mut f, 1600.0, &mut p, 1);
            o.push(f.output());
        }
        ac_rms(&o[20_000..])
    };
    let (dry, wet) = (measure(0), measure(15));
    assert!(
        wet > dry * 1.1,
        "resonance did not boost: dry {dry} wet {wet}"
    );
}

#[test]
fn all_taps_off_leaves_no_signal_on_a_routed_voice() {
    let mut f = Filter::new(Model::Pal);
    f.set_fc(2047, CLOCK);
    f.set_mode(ROUTE_VOICE_1, TAP_NONE);
    f.set_volume(15);
    let mut p = 0.0;
    let mut o = Vec::with_capacity(40_000);
    for _ in 0..40_000 {
        drive(&mut f, 1000.0, &mut p, 1);
        o.push(f.output());
    }
    let tail = &o[20_000..];
    assert!(
        ac_rms(tail) < 1e-9,
        "no taps but signal got through: {}",
        ac_rms(tail)
    );
    let mean = tail.iter().sum::<f64>() / tail.len() as f64;
    // What the bypass leaves behind is the mixer's own DC term, a borrowed value.
    #[cfg(feature = "gpl-data")]
    assert!(
        (mean - sid_gpl_data::MIXER_DC).abs() < 1e-9,
        "the mixer DC is not what the bypass leaves behind: {mean}"
    );
    #[cfg(not(feature = "gpl-data"))]
    assert!(mean.is_finite());
}

#[test]
fn voice3_off_does_not_silence_a_voice3_that_is_routed() {
    let mut f = Filter::new(Model::Pal);
    f.set_fc(2047, CLOCK);
    f.set_mode(ROUTE_VOICE_3, TAP_LOW_PASS);
    f.set_voice3_off(true);
    f.set_volume(15);
    let mut p = 0.0;
    let mut o = Vec::with_capacity(40_000);
    for _ in 0..40_000 {
        drive(&mut f, 1000.0, &mut p, 3);
        o.push(f.output());
    }
    assert!(
        ac_rms(&o[20_000..]) > 0.01,
        "routed voice 3 was silenced by voice3off: {}",
        ac_rms(&o[20_000..])
    );
}

#[test]
fn voice3_off_silences_a_voice3_that_is_not_routed() {
    let mut f = Filter::new(Model::Pal);
    f.set_fc(2047, CLOCK);
    f.set_mode(0, TAP_LOW_PASS);
    f.set_voice3_off(true);
    f.set_volume(15);
    let mut p = 0.0;
    let mut o = Vec::with_capacity(40_000);
    for _ in 0..40_000 {
        drive(&mut f, 1000.0, &mut p, 3);
        o.push(f.output());
    }
    assert!(
        ac_rms(&o[20_000..]) < 1e-9,
        "unrouted voice 3 survived voice3off: {}",
        ac_rms(&o[20_000..])
    );
}
