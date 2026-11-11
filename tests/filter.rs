use rust_sid::Model;
use rust_sid::tables::cutoff_hz;

#[test]
fn cutoff_spans_the_measured_6581_range() {
    let lo = cutoff_hz(Model::Pal, 0);
    let hi = cutoff_hz(Model::Pal, 2047);
    assert!((180.0..=260.0).contains(&lo), "fc=0 gave {lo} Hz");
    assert!((16_000.0..=20_000.0).contains(&hi), "fc=2047 gave {hi} Hz");
}

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
