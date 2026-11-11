//! End-to-end regression for the PSID harness: a real 6510 program writes the
//! chip through the bus, the chip renders, and a WAV comes out that is audible
//! and not clipped. The fixture is a hand-assembled tune that steps voice 1
//! through a C major scale from its play routine.

use std::path::{Path, PathBuf};
use std::process::Command;

fn harness() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for profile in ["debug", "release"] {
        let candidate = manifest
            .join("target")
            .join(profile)
            .join("examples")
            .join("render");
        if candidate.exists() {
            return candidate;
        }
    }
    panic!(
        "the render example has not been built; run `cargo build --example render` \
         (or `cargo test`, which builds examples) before this test"
    );
}

/// Read a mono PCM16 WAV back into samples.
fn read_wav(path: &Path) -> Vec<i16> {
    let bytes = std::fs::read(path).expect("the harness wrote no output file");
    assert_eq!(&bytes[0..4], b"RIFF", "output is not a WAV file");
    let data_len = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
    let body = &bytes[44..44 + data_len];
    assert_eq!(body.len() % 2, 0, "odd PCM16 length");
    let (pairs, _) = body.as_chunks::<2>();
    pairs
        .iter()
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}

#[test]
fn the_harness_renders_an_audible_unclipped_tune() {
    let tune = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("scale.sid");
    let mut out = std::env::temp_dir();
    out.push(format!(
        "rust-sid-fixture-{}-{}.wav",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    let status = Command::new(harness())
        .arg(&tune)
        .args(["--seconds", "2", "--out"])
        .arg(&out)
        .status()
        .expect("the harness binary would not run");
    assert!(status.success(), "the harness exited with {status}");

    let samples = read_wav(&out);
    std::fs::remove_file(&out).ok();

    // Two seconds at 44.1 kHz, allow the resampler's warm-up to be held back.
    assert!(
        samples.len() > 80_000,
        "only {} samples came out of 2 seconds",
        samples.len()
    );

    let mid = &samples[samples.len() / 2..];
    let rms = (mid
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum::<f64>()
        / mid.len() as f64)
        .sqrt();
    let peak = mid.iter().map(|s| i32::from(*s).abs()).max().unwrap();
    let silent = mid.iter().filter(|s| **s == 0).count() as f64 / mid.len() as f64;

    assert!(
        rms > 500.0,
        "the fixture rendered near-silence: rms {rms:.0}, zero fraction {silent:.2}"
    );
    assert!(silent < 0.05, "mostly zeros: fraction {silent:.2}");
    assert!(
        peak < 32_000,
        "the output is clipping: peak {peak} of 32767"
    );

    // The tune steps through a scale, so the second half must contain more than
    // one pitch. Count zero crossings in two windows and require they differ.
    let crossings =
        |w: &[i16]| -> usize { w.windows(2).filter(|p| (p[0] < 0) != (p[1] < 0)).count() };
    let quarter = mid.len() / 4;
    let first = crossings(&mid[..quarter]);
    let last = crossings(&mid[mid.len() - quarter..]);
    assert!(
        first != last,
        "the pitch never changed: {first} crossings in both windows"
    );
}

/// The second fixture is driven the way most real tunes are: its init installs
/// a handler at the interrupt vector and starts CIA 1 timer A, and the handler —
/// not the harness — calls the play routine. A single interrupt is not enough;
/// the whole point is that the processor keeps taking them, so the render must
/// still be carrying audio at the end rather than only at the start.
#[test]
fn an_irq_driven_tune_keeps_taking_interrupts() {
    let tune = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("irq.sid");
    let mut out = std::env::temp_dir();
    out.push(format!(
        "rust-sid-irq-{}-{}.wav",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    let run = Command::new(harness())
        .arg(&tune)
        .args(["--seconds", "2", "--out"])
        .arg(&out)
        .output()
        .expect("the harness binary would not run");
    assert!(run.status.success(), "the harness exited with failure");

    let report = String::from_utf8_lossy(&run.stdout);
    let words: Vec<&str> = report.split_whitespace().collect();
    let irqs: u64 = words
        .iter()
        .position(|w| *w == "IRQs")
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| words[i].parse().ok())
        .unwrap_or_else(|| panic!("could not find an interrupt count in: {report}"));

    let samples = read_wav(&out);
    std::fs::remove_file(&out).ok();

    // The fixture's timer is latched to raise the line about fifty times a
    // second. Anything near one means the handler is not returning with
    // interrupts restored, which is the defect this test exists to catch.
    assert!(
        irqs >= 40,
        "only {irqs} interrupts in two seconds; the handler is not re-arming"
    );

    let tail = &samples[samples.len() - samples.len() / 4..];
    let rms = (tail
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum::<f64>()
        / tail.len() as f64)
        .sqrt();
    let silent = tail.iter().filter(|s| **s == 0).count() as f64 / tail.len() as f64;
    assert!(
        rms > 300.0,
        "audio died before the end of the render: tail rms {rms:.0}, zero fraction {silent:.2}"
    );
    assert!(silent < 0.05, "the tail is mostly silence: {silent:.2}");
}
