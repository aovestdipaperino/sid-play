# Dropping the sid-gpl-data dependency: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the 6581 cutoff curve, the 6581 combined waveforms and the power-on RAM image with clean-room models scored against the measured data by a PASS/FAIL-only evaluator. Then drop the `gpl-data` feature and the `sid-gpl-data` dependency from `mos6581` and `sid-play` (0.2.0).

**Architecture:**
- Task 1 adds seams in `mos6581`: `tables::default::clean_cutoff_table()` and `tables::default::clean_waves()`. Task 2 adds the same in `sid-play`: `sid_play::poweron::CLEAN_POWERON` and `render --poweron FILE`.
- The seams are compiled in both builds and carry today's clean defaults, so the default renders don't change. A private evaluator (never committed) can read the clean candidate and the measured data side by side in one `gpl-data` build.
- Each clean-room item has three stages:
  1. An isolated author writes one self-contained Rust file in a scratch crate.
  2. An integrator copies it verbatim behind the seam.
  3. The controller runs the evaluator (PASS/FAIL only to the author, at most 3 rounds), and a public-sources reviewer checks the result.
- The finale removes the feature.

**Tech Stack:** Rust 2024 (rust-version 1.85), cargo, Python 3 (`-I`) for the WAV metrics, POSIX sh.

**Repos:**
- `M` = `/Users/enzo/Code/chipsynth64/mos6581`
- `P` = `/Users/enzo/Code/chipsynth64/sid-play`
- `G` = `/Users/enzo/Code/chipsynth64/sid-gpl-data`
- `S` = a private scratch directory outside the repositories, for the private evaluator and the clean-room scratch crates.

## Global Constraints

- **Oracle rule.** The evaluator returns only PASS or FAIL per metric to the author. No numbers, curves, diffs or hints. At most 3 author rounds per item.
- **No-pass fallback.** If no round passes, the best round that passes the evaluator's "better than today" verdict ships. Otherwise today's default stays. Either outcome is documented.
- **What the author may not see:**
  - all emulator sources (reSID, reSIDfp, libsidplayfp, VICE, others)
  - `sid-gpl-data`
  - the provenance Evidence column
  - `tests/gap.rs` and the fidelity scripts
  - git history
  - this plan and the spec (they contain measured-derived numbers)
- Every authored value carries a `Source:` line or is marked "unsourced own choice".
- **Reviewer.** Public sources only. Checks every `Source:` claim and that nothing looks lifted. It must not suggest values.
- **Allowed sources** (the author fetches only these URLs, or pages directly under them, and lists every URL fetched in its report):
  - MOS 6581 datasheet: https://www.cpcwiki.eu/imgs/9/9d/Mos_6581_sid.pdf
  - US patent 4,677,890: https://patents.google.com/patent/US4677890A/en
  - C64 schematics (250407, 250425, 250466 and siblings): http://www.zimmers.net/anonftp/pub/cbm/schematics/computers/c64/
  - oxyron SID notes: https://www.oxyron.de/html/registers_sid.html
  - Yannes interview: http://www.landley.net/history/mirror/commodore/yannes.html
  - Power-on item only: the Commodore 64 Programmer's Reference Guide and Compute!'s "Mapping the Commodore 64" on https://archive.org/, and the memory map at https://www.pagetable.com/c64ref/c64mem/
  - Cutoff item only: https://en.wikipedia.org/wiki/MOSFET for the textbook triode model.
- **Pass thresholds** (spec, verbatim):
  - **Cutoff:**
    - closed corner (fc = 0) within a factor of 1.5 of the measured value
    - median absolute log2 error over fc = 0..2047 below 0.5
    - sid-play `scale` fixture RMS within a factor of 2 of the `gpl-data` build
  - **Waves:**
    - each of the four tables has an RMS error against measured at least 40% lower than the AND tables'
    - no table is worse than AND
    - `pst` differs from `st`
  - **Power-on:**
    - at least 90% of the 1024 bytes match the measured pattern
    - all 17 fixture tunes render with tonality median within 0.05 of the same default build run with the measured image loaded through `render --poweron FILE`
- **Bit-identity.** Until the finale, `gpl-data` builds must stay bit-identical to `P/target/split-baseline/SHA256SUMS` (scale.wav, irq.wav, last_ninja.wav; 5 s each).
- **Cargo.toml metadata:** authors `Enzo Lombardi <enzinol@gmail.com>`, GitHub account `aovestdipaperino`.
- **Commits** end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. The global pre-commit hook runs rustfmt and `clippy -D warnings`.
- **Nothing is published** (crates.io, git push) without the user's explicit go-ahead in the finale tasks.
- `sid-gpl-data` (crate and repo) is kept: no yank, no archive. Its README gets a note only.

---

### Task 1: mos6581 seams for the clean cutoff and clean waves

**Files:**
- Modify: `M/src/tables/default.rs`
- Modify: `M/src/tables/fc.rs:86-101`
- Modify: `M/src/tables/mod.rs:40-48`

**Interfaces:**
- Produces:
  - `pub fn mos6581::tables::default::clean_cutoff_table() -> &'static CutoffTable`
  - `pub fn mos6581::tables::default::clean_waves() -> &'static WaveTables`
  - `pub(crate) fn mos6581::tables::fc::build_from(f: impl Fn(u16) -> f64) -> CutoffTable`
  - All of these are compiled in both builds. The default build routes through them.

- [ ] **Step 1: Write the failing tests** (append to the `tests` module in `M/src/tables/default.rs`)

```rust
    #[test]
    fn the_clean_seams_carry_todays_defaults() {
        // Until a clean-room model lands, the seams return today's built-ins.
        let c = clean_cutoff_table();
        assert!((c.f0[0] - 30.0).abs() < 1e-9);
        assert!((c.f0[2047] - 12_000.0).abs() < 1e-9);
        assert!(std::ptr::eq(clean_waves(), &WAVE_AND));
    }

    #[cfg(not(feature = "gpl-data"))]
    #[test]
    fn the_default_build_routes_through_the_seams() {
        let w = crate::tables::for_model(crate::Model::Pal);
        assert!(std::ptr::eq(w, clean_waves()));
        let c = crate::tables::cutoff_table(crate::Model::Pal);
        assert!(std::ptr::eq(c, clean_cutoff_table()));
    }
```

- [ ] **Step 2: Run them and confirm they fail**

Run: `cd M && cargo test --lib tables::default`
Expected: FAIL to compile, with "cannot find function `clean_cutoff_table`".

- [ ] **Step 3: Implement.** In `M/src/tables/fc.rs`:
  - Add `build_from`.
  - Make `build` use it.
  - Have the default-build 6581 table come from the seam.

```rust
/// A table from a function of the register value, for curves given as a model
/// rather than as points.
pub(crate) fn build_from(f: impl Fn(u16) -> f64) -> CutoffTable {
    let mut f0 = [0.0; 2048];
    for (i, slot) in f0.iter_mut().enumerate() {
        *slot = f(i as u16);
    }
    CutoffTable { f0 }
}
```

Replace the four `POINTS_*` consts and `cutoff_table` with:

```rust
#[cfg(feature = "gpl-data")]
const POINTS_6581: &[(u32, f64)] = sid_gpl_data::CUTOFF_POINTS_6581;
#[cfg(feature = "gpl-data")]
const POINTS_8580: &[(u32, f64)] = sid_gpl_data::CUTOFF_POINTS_8580;
#[cfg(not(feature = "gpl-data"))]
const POINTS_8580: &[(u32, f64)] = super::default::CUTOFF_POINTS;

#[cfg(feature = "gpl-data")]
static CUTOFF_6581: std::sync::OnceLock<CutoffTable> = std::sync::OnceLock::new();
static CUTOFF_8580: std::sync::OnceLock<CutoffTable> = std::sync::OnceLock::new();

/// The 6581 is the modelled chip, so both clock variants share its curve.
pub fn cutoff_table(model: crate::Model) -> &'static CutoffTable {
    match model {
        #[cfg(feature = "gpl-data")]
        crate::Model::Pal | crate::Model::Ntsc => CUTOFF_6581.get_or_init(|| build(POINTS_6581)),
        #[cfg(not(feature = "gpl-data"))]
        crate::Model::Pal | crate::Model::Ntsc => super::default::clean_cutoff_table(),
    }
}
```

Make `build` `pub(crate)`. In `M/src/tables/default.rs`, after `CUTOFF_POINTS`, add:

```rust
/// The built-in 6581 cutoff curve, compiled in every build so it can be compared
/// with the measured one. Today: the linear datasheet curve.
pub fn clean_cutoff_table() -> &'static super::CutoffTable {
    static T: std::sync::OnceLock<super::CutoffTable> = std::sync::OnceLock::new();
    T.get_or_init(|| super::fc::build(CUTOFF_POINTS))
}

/// The built-in 6581 combined waveforms, compiled in every build so they can be
/// compared with the measured ones. Today: the AND tables.
pub fn clean_waves() -> &'static WaveTables {
    &WAVE_AND
}
```

In `M/src/tables/mod.rs`, change the default arm of `for_model` to `crate::Model::Ntsc | crate::Model::Pal => default::clean_waves(),`. Then make `mod fc;` a `pub(crate) mod fc;` if `default.rs` needs it, and fix the doc comments that say "built-in linear" so they mention the seam.

- [ ] **Step 4: Run all tests in both builds**

Run: `cd M && cargo test && cargo test --features gpl-data && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features gpl-data -- -D warnings`
Expected: all pass. The default build runs 90 tests, two more than before.

- [ ] **Step 5: Commit**

```bash
cd M && git add -A src && git commit -m "refactor: seams for the clean cutoff curve and combined waveforms

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: sid-play seam, `--poweron FILE`, local mos6581 path and corpus (controller)

**Files:**
- Modify: `P/Cargo.toml` (add `[patch.crates-io]`)
- Modify: `P/crates/sid-play/src/lib.rs`, `P/crates/sid-play/src/poweron.rs`, `P/crates/sid-play/src/main.rs:12,630-640,665-700,905`
- Create (git-ignored): `P/data/*.sid` (17 tunes)
- Test: `P/crates/sid-play/tests/fixture_render.rs`

**Interfaces:**
- Consumes: Task 1 seams through the path patch.
- Produces:
  - `pub const sid_play::poweron::CLEAN_POWERON: [u8; 0x400]` (all builds; zeros today)
  - `pub const sid_play::poweron::POWERON` (measured with `gpl-data`, otherwise `CLEAN_POWERON`)
  - `render --poweron FILE`

- [ ] **Step 1:** Append this to `P/Cargo.toml` so that sid-play builds against the local mos6581. It is removed in Task 7.

```toml
# Development only: build against the sibling mos6581 checkout. Removed before publishing.
[patch.crates-io]
mos6581 = { path = "../mos6581" }
```

- [ ] **Step 2: Corpus.** The directory is git-ignored.

```bash
cd P && for n in 05-09-87 4_Soccer_Sims_Soccer_Skills 5_Title_Tunes 64_Dwarfs_on_XTC 720_Degrees Acceleration-mix ACE_II Action_Biker All_Points_Bulletin All_Terrain_Vehicle_Simulator Alternative_World_Games Antics Ark_Pandora Artura Athena Axel_F; do cp the local corpus directory/$n.sid data/; done && cp ../last_ninja.sid data/ && ls data/*.sid | wc -l
```
Expected: `17`.

- [ ] **Step 3: Write the failing test** (append to `P/crates/sid-play/tests/fixture_render.rs`)

```rust
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
    let loaded = render(&["--poweron".as_ref(), image.as_os_str()], &dir.join("b.wav"));
    assert_eq!(plain, loaded, "loading the built-in image from a file changed the render");
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
```

Run: `cd P && cargo test -p sid-play --test fixture_render poweron`
Expected: FAIL to compile (`sid_play::poweron` not found).

- [ ] **Step 4: Implement.** `P/crates/sid-play/src/poweron.rs` becomes:

```rust
//! The RAM pattern at `$0000-$03FF` before a tune's init runs.

/// The clean-room image, compiled in every build so it can be compared with the
/// measured one. Today: zeros; the harness still writes the Kernal pointers it
/// sets for every tune.
pub const CLEAN_POWERON: [u8; 0x0400] = [0; 0x0400];

/// With `gpl-data`: libsidplayfp's captured pattern (GPL-2.0, via
/// `sid-gpl-data`). Without it: [`CLEAN_POWERON`].
#[cfg(feature = "gpl-data")]
pub const POWERON: [u8; 0x0400] = sid_gpl_data::POWERON;
#[cfg(not(feature = "gpl-data"))]
pub const POWERON: [u8; 0x0400] = CLEAN_POWERON;
```

In `lib.rs`, add `#[doc(hidden)] pub mod poweron;`. In `main.rs`:
- Delete `mod poweron;`.
- Add `poweron: Option<PathBuf>` to `Options`.
- Parse it with `"--poweron" => o.poweron = Some(PathBuf::from(args.next().ok_or("--poweron needs a value")?)),`.
- Give `write_machine_state` an `image: &[u8; 0x400]` parameter and copy from it instead of `poweron::POWERON`.
- At the call site, build the image:

```rust
let image: [u8; 0x400] = match &opts.poweron {
    Some(p) => std::fs::read(p)?
        .try_into()
        .map_err(|v: Vec<u8>| format!("--poweron wants 1024 bytes, got {}", v.len()))?,
    None => sid_play::poweron::POWERON,
};
```

Add `[--poweron FILE]` to the usage string and the module doc line 9-10.

- [ ] **Step 5: Verify tests and bit-identity**

```bash
cd P && cargo test && cargo test --features gpl-data && cargo clippy --all-targets -- -D warnings && cargo build --release -p sid-play --features gpl-data && mkdir -p target/bitcheck && for t in scale irq; do ./target/release/render crates/sid-play/tests/fixtures/$t.sid --seconds 5 --out target/bitcheck/$t.wav; done && ./target/release/render ../last_ninja.sid --seconds 5 --out target/bitcheck/last_ninja.wav && (cd target/bitcheck && shasum -a 256 -c ../split-baseline/SHA256SUMS)
```
Expected: all tests pass, and the three files report `OK`.

- [ ] **Step 6: Commit** `P/Cargo.toml`, `crates/sid-play/src/*`, `crates/sid-play/tests/fixture_render.rs` with the message `feat: --poweron FILE and a clean power-on seam; build against local mos6581`.

---

### Task 3: Private evaluator (controller only, never committed)

The controller builds a private evaluator in `S`, outside every repository. Nothing of it is committed, and neither its code nor its scratch paths appear in a public repository.

For each item (cutoff, waves, poweron) it compares the clean candidate with the measured data side by side and scores named metrics. The metrics cover, for the cutoff, the closed corner, the median log2 error over the register range, the rms level of the `scale` fixture, and a "better than today" verdict against the current default. For the waves they are, per table, the rms error against the measured table relative to the AND baseline (`rms_40pct_below_and`, `not_worse_than_and`), plus `pst_differs_from_st`. For the power-on image they are `bytes_90pct_match` and `tonality_all_within_0.05` over the corpus.

Later tasks invoke it as `sh S/gpl-eval/eval.sh cutoff|waves|poweron [--verbose]`. It prints only `metric: PASS|FAIL` lines. A `--verbose` flag adds the numbers for the controller's own use; they are never shown to the clean-room authors and never written to a public file except where a later task records a published summary figure in `docs/provenance.md`.

- [ ] **Baseline sanity run:** with the Task 1 and 2 seams in place, every "better than today" metric must FAIL (the candidate is today's default). Threshold metrics FAIL except where today's default already passes; record those in the ledger.

---

### Task 4: Clean-room 6581 cutoff curve

**Files:**
- Author (isolated): `S/cleanroom/cutoff/{Cargo.toml,src/lib.rs}`
- Integrate: create `M/src/tables/clean_cutoff.rs`, modify `M/src/tables/default.rs` (`clean_cutoff_table`) and `M/src/tables/mod.rs` (`mod clean_cutoff;`)
- Docs: `M/docs/provenance.md` ("Clean-room defaults", "Known default-build fidelity gaps")

**Interfaces:**
- Author produces `pub fn cutoff_hz_6581(fc: u16) -> f64` (fc in 0..=2047; the caller clamps).
- Integration: `clean_cutoff_table()` returns `T.get_or_init(|| super::fc::build_from(super::clean_cutoff::cutoff_hz_6581))`.

- [ ] **Step 1: Scaffold the author crate** (controller):

```bash
mkdir -p S/cleanroom/cutoff/src && cd S/cleanroom/cutoff && printf '[package]\nname = "cutoff"\nversion = "0.0.0"\nedition = "2024"\npublish = false\n\n[workspace]\n' > Cargo.toml && echo '//! Clean-room 6581 cutoff curve.' > src/lib.rs
```

- [ ] **Step 2: Dispatch the author** (fresh general-purpose agent, most capable model, no repo access). The prompt is the brief below, verbatim, plus the Global Constraints' "Allowed sources" list, the forbidden list and the `Source:` rule:

> You are writing a clean-room model of the MOS 6581 SID filter's cutoff frequency as a function of its 11-bit cutoff register, for a SID fitted in a Commodore 64. Work only in `S/cleanroom/cutoff/`. Write `src/lib.rs` exposing `pub fn cutoff_hz_6581(fc: u16) -> f64`. It returns the filter's centre frequency in Hz for `fc` in 0..=2047, the value written to $D415 (low 3 bits) and $D416 (high 8 bits).
>
> Model it physically. The register feeds a DAC whose output controls a transistor used as a voltage-controlled resistor in each of the filter's two integrators, so f = 1/(2πRC). C is the filter capacitor the C64 actually fits; find it on the public C64 schematics. R as a function of the DAC voltage follows the textbook MOSFET triode-region behaviour, including what happens at and below threshold. Choose the DAC output range and transistor parameters from the sources where they say something, and otherwise as clearly marked own choices that you justify physically.
>
> Requirements:
> - The result is finite, positive and non-decreasing in fc.
> - Every numeric constant has a `/// Source:` line (URL plus the page, figure or section) or is marked `/// unsourced own choice:` with a one-line justification.
> - The module doc explains the model.
> - Tests check physical properties the model implies, such as monotonicity, the behaviour below threshold, and the stated capacitor scaling. Do not write tests that restate constants.
> - `cargo test` and `cargo clippy -- -D warnings` must pass.
>
> You may only fetch the listed URLs (and pages directly under them). Never open emulator source code or emulator documentation, or any page about SID emulation internals.
>
> Write a report to `S/cleanroom/cutoff/REPORT.md` listing every URL you fetched and every own choice. Reply with only the status and a one-line summary.

- [ ] **Step 3: Integrate** (fresh implementer, cheap model):
  - Copy `S/cleanroom/cutoff/src/lib.rs` verbatim to `M/src/tables/clean_cutoff.rs`. Change only `#![...]` crate attributes if present, and add the header line `//! Clean-room, see docs/provenance.md.` above the author's doc.
  - Keep the author's tests in the file.
  - Wire `clean_cutoff_table()` as above.
  - Replace the Task 1 test assertions `f0[0]==30`/`f0[2047]==12000` with `assert!((c.f0[1000] - clean_cutoff::cutoff_hz_6581(1000)).abs() < 1e-12)`.
  - Update `MIN_RMS`/`MIN_TAIL_RMS` default-build comments in `P/crates/sid-play/tests/fixture_render.rs` only if they now fail. Set the floor to half the new observed value and note the observed value beside it.
  - Run `cd M && cargo test && cargo test --features gpl-data && cargo clippy --all-targets -- -D warnings`, then `cd P && cargo test`.
  - Commit to M (and P if touched) with the message `feat: clean-room 6581 cutoff curve`.

- [ ] **Step 4: Evaluate** (controller): `sh S/gpl-eval/eval.sh cutoff`.
  - All three threshold metrics PASS: go to Step 5.
  - Any FAIL and fewer than 3 rounds done: SendMessage the same author with only the FAIL metric names, e.g. "Round 2. Results: cutoff.closed_corner_within_1.5x: FAIL; cutoff.median_log2_error_below_0.5: PASS; cutoff.scale_rms_within_2x: PASS. Revise using the allowed sources only." Then re-integrate (Step 3) and re-evaluate.
  - After round 3 with no pass: apply the no-pass fallback. If no round passes "better_than_today", revert the M commits for this item.

- [ ] **Step 5: Review** (fresh agent, standard model, public sources only):
  - Give it `M/src/tables/clean_cutoff.rs`, `S/cleanroom/cutoff/REPORT.md` and the allowed-source list.
  - It verifies each `Source:` against the fetched source and flags overstated or lifted-looking claims. It must not suggest values.
  - Fix doc-only findings directly. A finding that changes a value is a new author round, and counts toward the limit of 3.

- [ ] **Step 6: Provenance and gap record** (controller):
  - Run `sh S/gpl-eval/eval.sh cutoff --verbose`.
  - Add a "Clean-room cutoff curve" entry to `M/docs/provenance.md` (model, sources, own choices, rounds used, outcome).
  - Replace the "Cutoff closed corner" bullet in the gaps section with the new verbose numbers and the scale and irq fixture rms values.
  - Delete the "Candidate for the clean-room cutoff task" paragraph.
  - Commit `docs: provenance for the clean-room cutoff curve`.

---

### Task 5: Clean-room 6581 combined waveforms

**Files:**
- Author (isolated): `S/cleanroom/waves/{Cargo.toml,src/lib.rs}`
- Integrate: create `M/src/tables/clean_waves.rs`, modify `M/src/tables/default.rs` (`clean_waves`) and `M/src/tables/mod.rs`
- Docs: `M/docs/provenance.md`

**Interfaces:**
- Author produces `pub struct Tables { pub st: [u8; 4096], pub p_t: [u8; 4096], pub ps_: [u8; 4096], pub pst: [u8; 4096] }` and `pub fn combined_tables() -> Tables`.
- Integration: `clean_waves()` returns a `OnceLock<WaveTables>` filled from `combined_tables()` field by field.

- [ ] **Step 1: Scaffold** (as in Task 4 Step 1, with the crate name `waves`).

- [ ] **Step 2: Dispatch the author** (fresh agent, most capable model). The brief:

> You are writing a clean-room model of the MOS 6581 SID's *combined* waveforms, the output when more than one of triangle, sawtooth and pulse is selected at once. Work only in `S/cleanroom/waves/`. Write `src/lib.rs` exposing `pub struct Tables { pub st: [u8; 4096], pub p_t: [u8; 4096], pub ps_: [u8; 4096], pub pst: [u8; 4096] }` and `pub fn combined_tables() -> Tables`.
>
> Each entry is the top 8 bits of the 12-bit waveform output.
> - `st[phase]`: sawtooth plus triangle.
> - `ps_[phase]`: pulse plus sawtooth, while the pulse is high.
> - `pst[phase]`: all three, while the pulse is high.
> - `p_t[i]`: pulse plus triangle while the pulse is high, where the triangle output is `(i << 1) & 0xfff`.
>
> `phase` is the oscillator's top 12 accumulator bits. Sawtooth is `phase` itself. Triangle is the function below. The caller already forces the output to zero while the pulse is low.
>
> ```rust
> const fn triangle(phase: u16) -> u16 {
>     if phase & 0x800 != 0 { ((!phase << 1) | 1) & 0x0fff } else { (phase << 1) & 0x0fff }
> }
> ```
>
> The textbook approximation is the bitwise AND of the selected waveforms. Real silicon differs, because the selected waveform generators drive the same 12 DAC input lines together through NMOS transistors. Model that electrically: decide from the circuit how a line settles when drivers disagree, and how a line is influenced by its neighbours through the shared network. Then derive each output bit from that model.
>
> Requirements:
> - `pst` and `st` must not be identical.
> - Every numeric constant has a `/// Source:` line (URL plus the page, figure or section) or is marked `/// unsourced own choice:` with a justification.
> - The module doc explains the model.
> - Tests check properties the model implies, not restated constants.
> - `cargo test` and `cargo clippy -- -D warnings` must pass.
>
> You may only fetch the listed URLs. Never open emulator source code, emulator documentation, waveform sample dumps, or any page about SID emulation internals.
>
> Write `S/cleanroom/waves/REPORT.md` listing every URL fetched and every own choice. Reply with only the status and a one-line summary.

- [ ] **Step 3: Integrate** (fresh implementer, cheap model):
  - Copy the file verbatim to `M/src/tables/clean_waves.rs`, with the same header rule as Task 4.
  - Wire `clean_waves()`:

```rust
pub fn clean_waves() -> &'static WaveTables {
    static T: std::sync::OnceLock<WaveTables> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let c = super::clean_waves::combined_tables();
        WaveTables { st: c.st, p_t: c.p_t, ps_: c.ps_, pst: c.pst }
    })
}
```

  - Replace `assert!(std::ptr::eq(clean_waves(), &WAVE_AND))` in the Task 1 test with `assert_ne!(clean_waves().pst, clean_waves().st);`.
  - The `three_way_matches_saw_triangle` test still describes `WAVE_AND` and stays.
  - Check whether any default-build test in `M/tests/wave.rs` or `M/src/wave.rs` asserted AND-specific values through `for_model`. If one did, make it read `default::WAVE_AND` directly, or turn it into a property of the clean tables. Never change the clean data.
  - Run both builds' tests and clippy, plus `cd P && cargo test`.
  - Commit `feat: clean-room 6581 combined waveforms`.

- [ ] **Step 4: Evaluate** with `sh S/gpl-eval/eval.sh waves`. Use the same loop and fallback rules as Task 4 Step 4. Pass means every `rms_40pct_below_and`, every `not_worse_than_and` and `pst_differs_from_st` is PASS.

- [ ] **Step 5: Review**, as in Task 4 Step 5, with the waves files.

- [ ] **Step 6: Provenance.** Run `eval.sh waves --verbose`. Add a "Clean-room combined waveforms" entry. Replace the "Combined waveforms" and "default `pst` table equals `st`" gap bullets with the new numbers. Commit.

---

### Task 6: Clean-room power-on RAM image

**Files:**
- Author (isolated): `S/cleanroom/poweron/{Cargo.toml,src/lib.rs}`
- Integrate: modify `P/crates/sid-play/src/poweron.rs` (`CLEAN_POWERON`)
- Docs: `M/docs/provenance.md` (power-on bullet) and `P/HANDSOFF.md` licensing paragraph

**Interfaces:**
- Author produces `pub const POWERON: [u8; 0x400]`.
- Integration: `CLEAN_POWERON` takes the author's array. The author's file goes into `poweron.rs` as a private `mod clean { ... }` block, or as `crates/sid-play/src/poweron_clean.rs` with `pub const CLEAN_POWERON: [u8; 0x400] = poweron_clean::POWERON;`.

- [ ] **Step 1: Scaffold** (crate name `poweron`).

- [ ] **Step 2: Dispatch the author** (fresh agent, standard model). The brief:

> You are writing the contents of a Commodore 64's RAM at $0000-$03FF at the moment a PSID player hands control to a tune. That is after the KERNAL's power-on reset and BASIC's cold start have finished and the machine is sitting at the READY prompt with no program loaded. Work only in `S/cleanroom/poweron/`. Write `src/lib.rs` exposing `pub const POWERON: [u8; 0x400]`.
>
> Fill each byte or range from published documentation of the KERNAL and BASIC memory map:
> - zero-page system variables
> - the 6510 port defaults at $00/$01
> - BASIC and KERNAL vectors at $0300-$0333
> - the page 2 and 3 tables and defaults
> - the screen line link table
>
> Each byte or range carries a `/// Source:` with the document, page or entry. Bytes no source documents stay 0 and are listed in the report. Do not use RAM dumps, emulator snapshots, emulator sources, or ROM images or disassembly listings. Use documentation only.
>
> Tests check documented relationships, for example that a 16-bit vector is stored low byte first and points into the KERNAL ROM range. They must not restate the array.
>
> `cargo test` and `cargo clippy -- -D warnings` must pass. Write `S/cleanroom/poweron/REPORT.md` listing every URL fetched and every own choice. Reply with only the status and a one-line summary.

- [ ] **Step 3: Integrate** (fresh implementer, cheap model):
  - Put the author's file in as `P/crates/sid-play/src/poweron_clean.rs`, verbatim plus a header. Declare it with `mod poweron_clean;` inside `poweron.rs`, or in lib.rs as `#[doc(hidden)] pub mod poweron_clean;`.
  - Set `CLEAN_POWERON = poweron_clean::POWERON`.
  - Run `cd P && cargo test && cargo test --features gpl-data && cargo clippy --all-targets -- -D warnings`.
  - Run the Task 2 Step 5 bit-identity check for the gpl-data build.
  - Commit `feat: clean-room power-on RAM image`.

- [ ] **Step 4: Evaluate** with `sh S/gpl-eval/eval.sh poweron`. The pass criteria are `bytes_90pct_match` and `tonality_all_within_0.05`. Use the same loop and fallback rules as Task 4.

- [ ] **Step 5: Review**, as in Task 4 Step 5.

- [ ] **Step 6: Provenance.** Run `--verbose`. Replace the "Power-on RAM in `sid-play`" gap bullet, add a "Clean-room power-on image" entry, and commit in M. Update the HANDSOFF.md licensing paragraph in P and commit.

---

### Task 7: mos6581 0.2.0 without gpl-data

**Files:**
- Modify:
  - `M/Cargo.toml`
  - `M/src/lib.rs`
  - `M/src/chip_data.rs` (delete it and use `clean_chip_data` directly, renamed `chip_data`)
  - `M/src/tables/{mod.rs,fc.rs,default.rs}`
  - `M/src/{voice.rs,wave.rs,envelope.rs,filter.rs}` (gpl-gated tests and comments)
  - `M/README.md`, `M/docs/provenance.md`, `M/CHANGELOG.md` (create it if absent)
- Delete: `M/tests/gap.rs`, and every `#[cfg(feature = "gpl-data")]` and `#[cfg(not(feature = "gpl-data"))]` item. Keep the not-gated side.

- [ ] **Step 1: Strip the feature.**
  - Remove the `[dependencies] sid-gpl-data` line and the `[features] gpl-data` entry.
  - Bump to `version = "0.2.0"`.
  - Rename `src/clean_chip_data.rs` to `src/chip_data.rs`, replacing the switch file, and keep it `#[doc(hidden)] pub mod chip_data;`.
  - Delete the 8580 points, `cutoff_table_8580`, `WAVE6581`, `WAVE8580` and `from_data`.
  - Keep `WAVE_AND` and `CUTOFF_POINTS` only if something still uses them (the Task 4/5 fallback). Otherwise delete them and their tests.
  - The crate-level doc in `lib.rs` loses the `gpl-data` paragraph.

- [ ] **Step 2: Verify.**

```bash
cd M && cargo test && cargo clippy --all-targets -- -D warnings && cargo tree -e normal | grep -c sid-gpl-data; grep -rn "gpl-data\|sid_gpl_data\|sid-gpl-data" src tests Cargo.toml README.md
```
Expected:
- Tests pass and clippy is clean.
- `cargo tree` prints `0` (grep exits 1).
- The final grep prints nothing.

- [ ] **Step 3: Docs.**
  - README: the licence is MIT OR Apache-2.0 with no optional GPL. Add a short "Fidelity" note pointing to provenance.md.
  - provenance.md: add a dated "0.2.0: gpl-data removed" section. Keep the historical audit table as history.
  - CHANGELOG: `0.2.0 - removed the gpl-data feature; clean-room cutoff/waves (outcomes in docs/provenance.md)`.
  - Commit `feat!: drop the gpl-data feature (0.2.0)`.

- [ ] **Step 4: Publish (ask first).** Show the user the diff stat and the `cargo publish --dry-run` output. Only after an explicit go: `git push && git tag mos6581-v0.2.0 && git push --tags && cargo publish`.

---

### Task 8: sid-play 0.2.0 and the sid-gpl-data README note

**Files:**
- Modify:
  - `P/Cargo.toml` (remove `[patch.crates-io]`)
  - `P/crates/sid-play/Cargo.toml`
  - `P/crates/sid-play/src/{lib.rs,poweron.rs,main.rs}`
  - `P/crates/sid-play/tests/fixture_render.rs`
  - `P/README.md`, `P/HANDSOFF.md`
  - `G/README.md`
- Delete: `P/crates/sid-play/scripts/fidelity-gap.sh`. Keep `tonality.py`, which is licence-neutral and still useful.

- [ ] **Step 1: Strip the feature.**
  - Set `mos6581 = "0.2"`, remove `sid-gpl-data` and `[features] gpl-data`, and set `version = "0.2.0"`.
  - `poweron.rs`: `pub const POWERON: [u8; 0x400] = CLEAN_POWERON;` with no cfg.
  - `lib.rs` doc: drop the gpl paragraph.
  - `fixture_render.rs`: a single `MIN_RMS`/`MIN_TAIL_RMS` (the former default-build values).
  - Remove the `[patch.crates-io]` block only after mos6581 0.2.0 is on crates.io. Until then keep it, so the step can be verified locally.

- [ ] **Step 2: Verify.**

```bash
cd P && cargo test && cargo clippy --all-targets -- -D warnings && cargo tree -e normal | grep -c sid-gpl-data; grep -rn "gpl-data\|sid_gpl_data" crates Cargo.toml README.md
```
Expected: pass, `0`, and nothing printed. HANDSOFF.md may keep a dated historical mention, edited to say the feature was removed in 0.2.0.

Also render the 17 corpus tunes (`for f in data/*.sid; do ./target/release/render "$f" --seconds 10 --out /tmp/x.wav >/dev/null && python3 -I crates/sid-play/scripts/tonality.py /tmp/x.wav; done`) and confirm every tonality is above 0.2.

- [ ] **Step 3: Docs.**
  - README: drop the GPL build section.
  - HANDSOFF.md: replace the Licensing section with the 0.2.0 state and remove the fidelity-gap table's "gpl-data" column. Note that the historical numbers live in mos6581's provenance.md.
  - `G/README.md`: add at the top "Since mos6581 0.2.0 and sid-play 0.2.0, neither crate depends on sid-gpl-data. The data here is unchanged and remains available under GPL-2.0-only." Commit in G (no publish).
  - Commit P with `feat!: drop the gpl-data feature (0.2.0)`.

- [ ] **Step 4: Publish (ask first).** Ask after mos6581 0.2.0 is live:
  - Remove `[patch.crates-io]` and run `cargo update -p mos6581`. Re-run Step 2.
  - Commit `chore: build against mos6581 0.2 from crates.io`.
  - Show `cargo publish --dry-run -p sid-play`.
  - After an explicit go: push P and G, `git tag sid-play-v0.2.0 && git push --tags && cargo publish -p sid-play`.
  - Update the memory file `crate-split-plan.md` with the final state.
