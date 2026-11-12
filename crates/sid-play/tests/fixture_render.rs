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

// Timing tests. Each builds a tiny PSID in memory, renders it, and compares the
// "N frames driven" figure against the number of play calls the documented
// clock implies over the "N cycles" the render reports. The cycle count rather
// than seconds*clock is used because the render stops on a 4096-sample block
// boundary, which overshoots the requested length by up to 4095 samples.

/// The two processor clocks, as `mos6581::Model::clock_hz` gives them.
const PAL_CLOCK: f64 = 985_248.0;
const NTSC_CLOCK: f64 = 1_022_727.0;
/// One VIC-II raster frame in cycles: 6569 (PAL) 312 lines x 63 cycles, and
/// 6567R8 (NTSC) 263 lines x 65 cycles (Christian Bauer, "The MOS 6567/6569
/// video controller (VIC-II) and its application in the Commodore 64",
/// section 3.4, table of video display dimensions).
const PAL_FRAME: f64 = 312.0 * 63.0;
const NTSC_FRAME: f64 = 263.0 * 65.0;
/// The KERNAL's 1/60 s timer A latch for PAL, $4025 (ROM routine at $FDDD;
/// also the PSID format's "SID file environment"). The timer underflows every
/// latch+1 cycles.
const KERNAL_LATCH_PAL: f64 = 16_421.0;

/// A version 2 PSID with one song, loaded at $1000 from the header, whose init
/// is at $1000 and whose play is at `play`. The flags word is zero, so the
/// header expresses no clock preference and the harness defaults to PAL.
fn psid(code: &[u8], play: u16, speed: u32) -> Vec<u8> {
    let mut b = vec![0u8; 0x7c];
    b[0..4].copy_from_slice(b"PSID");
    b[0x04..0x06].copy_from_slice(&2u16.to_be_bytes());
    b[0x06..0x08].copy_from_slice(&0x7cu16.to_be_bytes());
    b[0x08..0x0a].copy_from_slice(&0x1000u16.to_be_bytes());
    b[0x0a..0x0c].copy_from_slice(&0x1000u16.to_be_bytes());
    b[0x0c..0x0e].copy_from_slice(&play.to_be_bytes());
    b[0x0e..0x10].copy_from_slice(&1u16.to_be_bytes());
    b[0x10..0x12].copy_from_slice(&1u16.to_be_bytes());
    b[0x12..0x16].copy_from_slice(&speed.to_be_bytes());
    b.extend_from_slice(code);
    // The loader refuses anything shorter than 128 bytes, so a very short
    // block is padded with zeros that nothing executes.
    if b.len() < 0x80 {
        b.resize(0x80, 0);
    }
    b
}

/// What the summary line reports about a render.
struct Summary {
    cycles: f64,
    frames: f64,
    chip_writes: u64,
}

/// The number printed immediately before `label` in the summary line.
fn figure_before(report: &str, label: &str) -> u64 {
    let words: Vec<&str> = report.split_whitespace().collect();
    words
        .iter()
        .position(|w| *w == label)
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| words[i].parse().ok())
        .unwrap_or_else(|| panic!("no figure before {label:?} in: {report}"))
}

/// Render a PSID built in memory and parse the summary line.
fn render_built(name: &str, bytes: &[u8], seconds: &str, extra: &[&str]) -> Summary {
    let dir = std::env::temp_dir().join(format!("sidplay-timing-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tune = dir.join("tune.sid");
    let out = dir.join("out.wav");
    std::fs::write(&tune, bytes).unwrap();
    let run = Command::new(harness())
        .arg(&tune)
        .args(["--seconds", seconds])
        .args(extra)
        .arg("--out")
        .arg(&out)
        .output()
        .expect("the harness binary would not run");
    std::fs::remove_dir_all(&dir).ok();
    let report = String::from_utf8_lossy(&run.stdout).into_owned();
    assert!(
        run.status.success(),
        "the harness failed: {report}{}",
        String::from_utf8_lossy(&run.stderr)
    );
    Summary {
        cycles: figure_before(&report, "cycles,") as f64,
        frames: figure_before(&report, "frames") as f64,
        chip_writes: figure_before(&report, "chip"),
    }
}

fn assert_frames(s: &Summary, period: f64, what: &str) {
    let expected = s.cycles / period;
    assert!(
        (s.frames - expected).abs() <= 2.0,
        "{what}: {} frames driven over {} cycles, expected {expected:.1} (one per {period} cycles)",
        s.frames,
        s.cycles
    );
}

/// A CIA-speed song whose init only rewrites the timer A latch, never the
/// control register, exactly as a tempo table does. Play must follow the
/// latch: one call per latch+1 cycles.
#[test]
fn a_cia_speed_song_that_rewrites_only_the_latch_plays_at_the_latch_rate() {
    // init: LDA #$00 / STA $DC04 / LDA #$80 / STA $DC05 / RTS; play: RTS.
    let code = [
        0xa9, 0x00, 0x8d, 0x04, 0xdc, 0xa9, 0x80, 0x8d, 0x05, 0xdc, 0x60, 0x60,
    ];
    let s = render_built("latch", &psid(&code, 0x100b, 1), "2", &[]);
    assert_frames(&s, f64::from(0x8000 + 1), "latch $8000");
}

/// A CIA-speed song that never touches the CIA runs on the timer exactly as the
/// KERNAL left it.
#[test]
fn a_cia_speed_song_that_leaves_the_cia_alone_runs_on_the_kernal_timer() {
    // init: RTS; play: RTS.
    let s = render_built("untouched", &psid(&[0x60, 0x60], 0x1001, 1), "2", &[]);
    assert_frames(&s, KERNAL_LATCH_PAL + 1.0, "KERNAL latch $4025");
}

/// A CIA-speed song whose init enables interrupts and keeps running across a
/// timer underflow must still finish its init. The timer is running from
/// before init, so an interrupt arrives while init is still busy; it has to be
/// acknowledged and returned from rather than strand the processor.
#[test]
fn a_cia_speed_init_that_enables_interrupts_still_finishes() {
    // $1000 CLI
    // $1001 LDY #$20
    // $1003 LDX #$00
    // $1005 DEX / BNE $1005
    // $1008 DEY / BNE $1003      (about 41,000 cycles: two KERNAL periods)
    // $100B SEI
    // $100C LDA #$0F / STA $D418
    // $1011 RTS
    // $1012 RTS (play)
    let code = [
        0x58, 0xa0, 0x20, 0xa2, 0x00, 0xca, 0xd0, 0xfd, 0x88, 0xd0, 0xf8, 0x78, 0xa9, 0x0f, 0x8d,
        0x18, 0xd4, 0x60, 0x60,
    ];
    let s = render_built("cli", &psid(&code, 0x1012, 1), "1", &[]);
    assert_eq!(
        s.chip_writes, 1,
        "init never reached its volume write after an interrupt arrived mid-init"
    );
    assert_frames(&s, KERNAL_LATCH_PAL + 1.0, "KERNAL latch after a busy init");
}

/// A VBL song on PAL is called once per 6569 raster frame. Thirty seconds makes
/// the old clock/50 period (19704 cycles) miss by more than the tolerance.
#[test]
fn a_vbl_song_on_pal_is_called_once_per_raster_frame() {
    let s = render_built("vbl-pal", &psid(&[0x60, 0x60], 0x1001, 0), "30", &[]);
    assert_frames(&s, PAL_FRAME, "PAL raster frame");
    assert!(
        (s.cycles / PAL_CLOCK - 30.0).abs() < 0.2,
        "the render ran {} cycles, not about 30 PAL seconds",
        s.cycles
    );
}

/// A VBL song on NTSC is called once per 6567R8 raster frame.
#[test]
fn a_vbl_song_on_ntsc_is_called_once_per_raster_frame() {
    let s = render_built(
        "vbl-ntsc",
        &psid(&[0x60, 0x60], 0x1001, 0),
        "20",
        &["--ntsc"],
    );
    assert_frames(&s, NTSC_FRAME, "NTSC raster frame");
    assert!(
        (s.cycles / NTSC_CLOCK - 20.0).abs() < 0.2,
        "the render ran {} cycles, not about 20 NTSC seconds",
        s.cycles
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
