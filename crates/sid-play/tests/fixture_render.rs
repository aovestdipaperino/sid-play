//! End-to-end regression for the PSID harness: a real 6510 program writes the
//! chip through the bus, the chip renders, and a WAV comes out that is audible
//! and not clipped. The fixture is a hand-assembled tune that steps voice 1
//! through a C major scale from its play routine.

use std::path::{Path, PathBuf};
use std::process::Command;

// The fixtures play through a low-pass with the cutoff register at zero, so how
// loud they are depends on how low the closed corner sits. The floors are about
// half the observed values (noted beside them).
const MIN_RMS: f64 = 830.0; // observed 1668
const MIN_TAIL_RMS: f64 = 640.0; // observed 1278

fn harness() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_render"))
}

/// Read a mono PCM16 WAV back into samples.
fn read_wav(path: &Path) -> Vec<i16> {
    let bytes = std::fs::read(path).expect("the harness wrote no output file");
    assert_eq!(&bytes[0..4], b"RIFF", "output is not a WAV file");
    let data_len = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
    let body = &bytes[44..44 + data_len];
    assert_eq!(body.len() % 2, 0, "odd PCM16 length");
    body.chunks_exact(2)
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
        rms > MIN_RMS,
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
        rms > MIN_TAIL_RMS,
        "audio died before the end of the render: tail rms {rms:.0}, zero fraction {silent:.2}"
    );
    assert!(silent < 0.05, "the tail is mostly silence: {silent:.2}");
}

/// The container header was read against the wrong field layout for a long time:
/// the data offset was treated as relative to the end of the magic, the load
/// address was read as a block length, and the song count was read as the load
/// address. Each of those mistakes cancelled another, so the fixtures still
/// played. This pins the layout itself, so a future change cannot quietly
/// reintroduce a pair of cancelling errors.
#[test]
fn the_loader_resolves_the_container_header() {
    for (fixture, expect) in [
        (
            "scale.sid",
            "load $2000 init $2000 play $202A (called), 82 bytes at file 128",
        ),
        (
            "irq.sid",
            "load $2000 init $2000 play $2100 (called), 512 bytes at file 128",
        ),
    ] {
        let tune = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(fixture);
        let out = Command::new(harness())
            .args(["--probe"])
            .arg(&tune)
            .output()
            .expect("the harness ran");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.contains(expect),
            "{fixture} resolved to {text:?}, expected it to contain {expect:?}"
        );
    }
}

/// A load address that would place the tune over the zero page and stack is a
/// corrupt file, not something to guess around.
#[test]
fn the_loader_refuses_a_block_that_cannot_be_placed() {
    let dir = std::env::temp_dir().join("rust-sid-badload.sid");
    let mut bytes = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("scale.sid"),
    )
    .unwrap();
    bytes[8..10].copy_from_slice(&0x0052u16.to_be_bytes());
    std::fs::write(&dir, &bytes).unwrap();

    let out = Command::new(harness())
        .args(["--probe"])
        .arg(&dir)
        .output()
        .expect("the harness ran");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("zero page"),
        "a load at $0052 was not refused: {err:?}"
    );
    std::fs::remove_file(&dir).ok();
}

#[test]
fn poweron_file_overrides_the_built_in_image() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scale.sid");
    let dir = std::env::temp_dir().join(format!("sidplay-poweron-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let image = dir.join("image.bin");
    std::fs::write(&image, sid_play::poweron::POWERON).unwrap();
    let render = |extra: &[&std::ffi::OsStr], out: &Path| {
        let status = Command::new(harness())
            .arg(&fixture)
            .args(["--seconds", "1", "--out"])
            .arg(out)
            .args(extra)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::read(out).unwrap()
    };
    let plain = render(&[], &dir.join("a.wav"));
    let loaded = render(
        &["--poweron".as_ref(), image.as_os_str()],
        &dir.join("b.wav"),
    );
    assert_eq!(
        plain, loaded,
        "loading the built-in image from a file changed the render"
    );
    std::fs::write(&image, [0u8; 10]).unwrap();
    let status = Command::new(harness())
        .arg(&fixture)
        .args(["--seconds", "1", "--poweron"])
        .arg(&image)
        .arg("--out")
        .arg(dir.join("c.wav"))
        .status()
        .unwrap();
    assert!(!status.success(), "a 10-byte power-on file was accepted");
}

/// Without the `playback` feature there is no audio device code at all, and
/// asking for it must say how to get it rather than quietly writing a WAV.
#[cfg(not(feature = "playback"))]
#[test]
fn audio_sink_without_the_feature_says_how_to_get_it() {
    let tune = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("scale.sid");
    let out = Command::new(harness())
        .arg(&tune)
        .args(["--seconds", "1", "--sink", "audio"])
        .output()
        .expect("the harness binary would not run");
    assert!(
        !out.status.success(),
        "--sink audio succeeded without the feature"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("this build has no audio output; rebuild with --features playback"),
        "unexpected message: {stderr}"
    );
}

#[test]
fn an_unknown_sink_is_refused() {
    let tune = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("scale.sid");
    let out = Command::new(harness())
        .arg(&tune)
        .args(["--seconds", "1", "--sink", "speaker"])
        .output()
        .expect("the harness binary would not run");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--sink"), "unexpected message: {stderr}");
}
