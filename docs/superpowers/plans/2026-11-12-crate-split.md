# Crate Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the single GPL-2.0 `rust-sid` crate into a workspace of `mos6581` and `sid-play` (MIT OR Apache-2.0) plus `sid-gpl-data` (GPL-2.0-only, data only), with the GPL crate used only through an off-by-default `gpl-data` feature.

**Architecture:** First audit provenance and record baseline renders. Then move files into a workspace without changing behaviour, then extract the borrowed data into `sid-gpl-data` (still always on, renders bit-identical), then put it behind the feature with GPL-free default data, then relicense and document. Each step keeps every test green and is checked against the baseline renders.

**Tech Stack:** Rust 2024 cargo workspace, Python 3 (stdlib only) for the generator tools and the fidelity metrics, POSIX sh.

Spec: `docs/superpowers/specs/2026-11-12-crate-split-design.md`.

## Global Constraints

- Repository: `/Users/enzo/Code/chipsynth64/rust-sid`, branch `feat/faithful-chip`. Workspace members live under `crates/`.
- Crate names exactly: `mos6581`, `sid-play`, `sid-gpl-data`. Feature name exactly: `gpl-data`, **off by default**, in both `mos6581` and `sid-play`; `sid-play`'s `gpl-data = ["mos6581/gpl-data", "dep:sid-gpl-data"]`.
- `sid-gpl-data` holds borrowed data only: `pub const` items and plain data structs, no functions, no dependencies. Every item keeps its attribution comment.
- No relicensing (Task 5) until the Task 1 provenance audit is complete and every finding resolved.
- With `--features gpl-data`, renders must stay **bit-identical** to the Task 1 baselines.
- `mos6510 = "0.1"` from crates.io.
- Every `Cargo.toml`: `authors = ["Enzo Lombardi <enzinol@gmail.com>"]`, `edition = "2024"`, `rust-version = "1.85"`.
- Clean-room rule: no GPL source may be consulted to *write* replacement data or code. The Task 1 auditor may read GPL sources only to *identify* borrowed material.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` clean at the end of every task, in both feature configurations from Task 4 on. A global pre-commit hook runs clippy and rustfmt; never `--no-verify`.
- Commit trailer: the Co-Authored-By line of the model that authored the commit.

## File Structure (after Task 5)

```
Cargo.toml                         workspace root (no package)
crates/mos6581/                    MIT OR Apache-2.0
  Cargo.toml, README.md, LICENSE-MIT, LICENSE-APACHE
  src/{lib,envelope,extfilt,filter,resample,sid,voice,wave}.rs
  src/tables/{mod,fc,default}.rs   types, interpolation, GPL-free default data
  tests/{envelope,filter,render,wave,gap}.rs
crates/sid-play/                   MIT OR Apache-2.0
  Cargo.toml, README.md, LICENSE-MIT, LICENSE-APACHE
  src/lib.rs (pub mod sink), src/sink.rs, src/main.rs (the render binary), src/poweron.rs
  tests/fixture_render.rs, tests/fixtures/{scale,irq}.sid
  scripts/fidelity-gap.sh, scripts/tonality.py
crates/sid-gpl-data/               GPL-2.0-only
  Cargo.toml, README.md, LICENSE (GPL-2.0 text)
  src/{lib,wave6581,wave8580,cutoff,poweron}.rs
tools/                             generators, now writing into crates/sid-gpl-data
docs/provenance.md                 the audit table
HANDSOFF.md
```

---

### Task 1: Provenance audit and baseline renders

**Files:**
- Create: `docs/provenance.md`
- Create (git-ignored): `target/split-baseline/*.wav`, `target/split-baseline/SHA256SUMS`

**Interfaces:**
- Produces: `docs/provenance.md` (table: file, item, origin, evidence, resolution); `target/split-baseline/SHA256SUMS` listing `scale.wav`, `irq.wav`, `last_ninja.wav`. Later tasks compare against these hashes.

- [ ] **Step 1: Record baseline renders at the current tree**

```bash
cd /Users/enzo/Code/chipsynth64/rust-sid
cargo build --release --examples
mkdir -p target/split-baseline
./target/release/examples/render tests/fixtures/scale.sid --seconds 5 --out target/split-baseline/scale.wav
./target/release/examples/render tests/fixtures/irq.sid --seconds 5 --out target/split-baseline/irq.wav
./target/release/examples/render /Users/enzo/Code/chipsynth64/last_ninja.sid --seconds 5 --out target/split-baseline/last_ninja.wav
(cd target/split-baseline && shasum -a 256 *.wav > SHA256SUMS && cat SHA256SUMS)
```
Expected: three WAVs and three hashes.

- [ ] **Step 2: Audit every source file**

For each file in `src/`, `examples/`, `tools/` and `tests/`, decide whether it contains material borrowed from a GPL source (reSID, reSID-fp, libsidplayfp, VICE, or their derivatives): tables, constants, formulas transcribed with their constants, or code structure translated line by line. Already known (record them, do not re-investigate): `src/tables/wave6581.rs`, `src/tables/wave8580.rs` (reSID waveform data), `POINTS_6581`/`POINTS_8580` in `src/tables/fc.rs` (reSID `f0_points`), `examples/render/poweron_table.rs` (libsidplayfp `poweron.bin`).

Pay particular attention to: `src/envelope.rs` (`RATE_PERIOD`, `EXPONENTIAL_PERIOD`, counter logic), `src/filter.rs` (state-variable filter structure and constants), `src/extfilt.rs` (pole constants), `src/voice.rs`/`src/sid.rs` (DC offsets, mixer constants), `src/wave.rs` (noise tap order, `SHIFT_RELOAD`), `src/resample.rs`.

You may read GPL sources only to compare. For each item use one resolution:
- **Public fact:** the value is in a non-GPL public source (the MOS datasheet, a C64 reference manual, a published hardware measurement article). Record the source.
- **Own derivation:** derivable from first principles or from the datasheet (show the derivation in one line).
- **Borrowed:** matches a GPL source with no independent public source. Record where it matches.

Write `docs/provenance.md`:

```markdown
# Provenance of rust-sid's data and constants

Audit date: 2026-11-12. Method: every file in src/, examples/, tools/, tests/
reviewed; GPL sources consulted only to identify matches, never to write code.

| File | Item | Origin | Evidence | Resolution |
|---|---|---|---|---|
| src/tables/wave6581.rs | combined waveform tables | reSID | byte-identical to reSID .dat | move to sid-gpl-data (Task 3) |
| ... one row per finding, including "no borrowed material" rows for clean files ... |
```

- [ ] **Step 3: Stop if the audit finds new borrowed material**

If any **Borrowed** row exists beyond the four known items, do not change code in this task. Report the rows as DONE_WITH_CONCERNS: the controller decides whether each moves into `sid-gpl-data` (and amends Task 3) or is rewritten.

- [ ] **Step 4: Commit**

```bash
git add docs/provenance.md
git commit -m "docs: provenance audit of rust-sid's data and constants"
```

---

### Task 2: Workspace layout, no behaviour change

**Files:**
- Modify: `Cargo.toml` (becomes the workspace root)
- Create: `crates/mos6581/Cargo.toml`, `crates/sid-play/Cargo.toml`, `crates/sid-play/src/lib.rs`
- Move: `src/` → `crates/mos6581/src/` (except `src/sink.rs` → `crates/sid-play/src/sink.rs`); `tests/{envelope,filter,render,wave}.rs` → `crates/mos6581/tests/`; `tests/fixture_render.rs` and `tests/fixtures/` → `crates/sid-play/tests/`; `examples/render.rs` → `crates/sid-play/src/main.rs`; `examples/render/poweron_table.rs` → `crates/sid-play/src/poweron_table.rs`
- Modify: `crates/mos6581/src/lib.rs` (drop `pub mod sink;`), `crates/sid-play/src/main.rs` (imports, module path), `crates/sid-play/tests/fixture_render.rs` (binary lookup), `tools/gen_tables.py`, `tools/gen_poweron.py` (output paths)

**Interfaces:**
- Produces: crate `mos6581` with the same public API `rust_sid` had minus `sink` (`mos6581::{Sid, Model}`, modules `envelope extfilt filter resample sid tables voice wave`); crate `sid-play` with `pub mod sink` (same API as before) and binary `render`.

- [ ] **Step 1: Move files with git**

```bash
cd /Users/enzo/Code/chipsynth64/rust-sid
mkdir -p crates/mos6581/tests crates/sid-play/src crates/sid-play/tests
git mv src crates/mos6581/src
git mv crates/mos6581/src/sink.rs crates/sid-play/src/sink.rs
git mv tests/envelope.rs tests/filter.rs tests/render.rs tests/wave.rs crates/mos6581/tests/
git mv tests/fixture_render.rs crates/sid-play/tests/fixture_render.rs
git mv tests/fixtures crates/sid-play/tests/fixtures
git mv examples/render.rs crates/sid-play/src/main.rs
git mv examples/render/poweron_table.rs crates/sid-play/src/poweron_table.rs
```

- [ ] **Step 2: Manifests**

Root `Cargo.toml` (replace entirely):

```toml
[workspace]
members = ["crates/mos6581", "crates/sid-play"]
resolver = "3"
```

`crates/mos6581/Cargo.toml`:

```toml
[package]
name = "mos6581"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
authors = ["Enzo Lombardi <enzinol@gmail.com>"]
license = "GPL-2.0-only"
description = "MOS 6581 SID chip emulator, stepped one Φ2 cycle at a time"
```

`crates/sid-play/Cargo.toml`:

```toml
[package]
name = "sid-play"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
authors = ["Enzo Lombardi <enzinol@gmail.com>"]
license = "GPL-2.0-only"
description = "Play C64 PSID tunes through the mos6581 SID emulator"

[dependencies]
mos6510 = "0.1"
mos6581 = { path = "../mos6581", version = "0.1" }

[[bin]]
name = "render"
path = "src/main.rs"
```

(Licences stay GPL here; Task 5 changes them after the audit.)

- [ ] **Step 3: Code edits**

`crates/mos6581/src/lib.rs`: delete the line `pub mod sink;`.

`crates/sid-play/src/lib.rs`:

```rust
//! Play C64 PSID tunes through the `mos6581` SID emulator.

pub mod sink;
```

`crates/sid-play/src/main.rs`: replace

```rust
use rust_sid::sink::{SampleFormat, Sink, WavSink, WavSpec};
use rust_sid::{Model, Sid};
```
with
```rust
use mos6581::{Model, Sid};
use sid_play::sink::{SampleFormat, Sink, WavSink, WavSpec};
```
and replace
```rust
#[path = "render/poweron_table.rs"]
mod poweron_table;
```
with
```rust
mod poweron_table;
```
Also update the usage line in the file's top doc comment from `cargo run --release --example render -- tune.sid` to `cargo run --release -p sid-play --bin render -- tune.sid`.

`crates/mos6581/tests/*.rs`: replace every `rust_sid::` with `mos6581::` (`sed -i '' 's/rust_sid::/mos6581::/g; s/use rust_sid;/use mos6581;/' crates/mos6581/tests/*.rs`), then build to find any remaining reference.

`crates/sid-play/tests/fixture_render.rs`: replace the whole body of `fn harness() -> PathBuf` with

```rust
fn harness() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_render"))
}
```
(cargo builds the binary before integration tests and sets this variable). Remove imports that become unused.

`tools/gen_tables.py`: change `OUT = pathlib.Path("src/tables")` to `OUT = pathlib.Path("crates/mos6581/src/tables")`, and the docstring/print paths to match. `tools/gen_poweron.py`: change its output path and docstring from `examples/render/poweron_table.rs` (or `examples/poweron_table.rs`) to `crates/sid-play/src/poweron_table.rs`.

- [ ] **Step 4: Build, test, compare renders**

```bash
cargo build --workspace --release
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
mkdir -p target/split-check
./target/release/render crates/sid-play/tests/fixtures/scale.sid --seconds 5 --out target/split-check/scale.wav
./target/release/render crates/sid-play/tests/fixtures/irq.sid --seconds 5 --out target/split-check/irq.wav
./target/release/render /Users/enzo/Code/chipsynth64/last_ninja.sid --seconds 5 --out target/split-check/last_ninja.wav
(cd target/split-check && shasum -a 256 -c ../split-baseline/SHA256SUMS)
```
Expected: tests pass (the same 91, now spread over two crates); `scale.wav: OK`, `irq.wav: OK`, `last_ninja.wav: OK`. If `shasum -c` fails, the move changed behaviour: find out why before committing.

- [ ] **Step 5: Update HANDSOFF.md build commands**

Replace `cargo run --release --example render -- …` lines in HANDSOFF.md with `cargo run --release -p sid-play --bin render -- …`, `cargo test` with `cargo test --workspace`, and the layout block with the `crates/` layout.

- [ ] **Step 6: Commit**

```bash
git add -A Cargo.toml Cargo.lock crates tools HANDSOFF.md src tests examples
git commit -m "refactor: split rust-sid into the mos6581 and sid-play workspace crates"
```

---

### Task 3: Extract the borrowed data into `sid-gpl-data` (still always on)

**Files:**
- Create: `crates/sid-gpl-data/{Cargo.toml,README.md,LICENSE}`, `crates/sid-gpl-data/src/{lib,wave6581,wave8580,cutoff,poweron}.rs`
- Move: `crates/mos6581/src/tables/wave6581.rs` → `crates/sid-gpl-data/src/wave6581.rs` (same for 8580); `crates/sid-play/src/poweron_table.rs` → `crates/sid-gpl-data/src/poweron.rs`
- Modify: `Cargo.toml` (add member), `crates/mos6581/Cargo.toml`, `crates/mos6581/src/tables/mod.rs`, `crates/mos6581/src/tables/fc.rs`, `crates/sid-play/Cargo.toml`, `crates/sid-play/src/main.rs`, `tools/gen_tables.py`, `tools/gen_poweron.py`

**Interfaces:**
- Produces (`sid-gpl-data`, all `pub`): `struct WaveData { st: [u8; 4096], p_t: [u8; 4096], ps_: [u8; 4096], pst: [u8; 4096] }` (pub fields); `const WAVE6581: WaveData`; `const WAVE8580: WaveData`; `const CUTOFF_POINTS_6581: &[(u32, f64)]`; `const CUTOFF_POINTS_8580: &[(u32, f64)]`; `const POWERON: [u8; 0x0400]`.
- `mos6581::tables` keeps its public API: `WaveTables`, `for_model`, `CutoffTable`, `cutoff_table`, `cutoff_table_8580`, `cutoff_hz`, `WAVE6581`, `WAVE8580`.

- [ ] **Step 1: Create the crate**

```bash
mkdir -p crates/sid-gpl-data/src
git mv crates/mos6581/src/tables/wave6581.rs crates/sid-gpl-data/src/wave6581.rs
git mv crates/mos6581/src/tables/wave8580.rs crates/sid-gpl-data/src/wave8580.rs
git mv crates/sid-play/src/poweron_table.rs crates/sid-gpl-data/src/poweron.rs
curl -sSfL https://www.gnu.org/licenses/old-licenses/gpl-2.0.txt -o crates/sid-gpl-data/LICENSE
```

`crates/sid-gpl-data/Cargo.toml`:

```toml
[package]
name = "sid-gpl-data"
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
authors = ["Enzo Lombardi <enzinol@gmail.com>"]
license = "GPL-2.0-only"
description = "GPL-2.0 measurement data borrowed by the mos6581 and sid-play crates, kept apart so it can be replaced"
```

`crates/sid-gpl-data/src/lib.rs`:

```rust
//! Data borrowed from GPL-2.0 projects, quarantined in one crate.
//!
//! Nothing here is code: only measured tables, each with its attribution.
//! `mos6581` and `sid-play` use this crate only through their `gpl-data`
//! feature. Each item is meant to be replaced by clean-room data and removed;
//! when this crate is empty it will be deleted.

#![no_std]
#![forbid(unsafe_code)]

mod cutoff;
mod poweron;
mod wave6581;
mod wave8580;

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
```

In `wave6581.rs` and `wave8580.rs`, change the item line (keep the two header comment lines):

```bash
sed -i '' 's/^pub static WAVE6581: crate::tables::WaveTables = crate::tables::WaveTables {/pub const WAVE6581: crate::WaveData = crate::WaveData {/' crates/sid-gpl-data/src/wave6581.rs
sed -i '' 's/^pub static WAVE8580: crate::tables::WaveTables = crate::tables::WaveTables {/pub const WAVE8580: crate::WaveData = crate::WaveData {/' crates/sid-gpl-data/src/wave8580.rs
```

In `poweron.rs`, keep its header and the array; it already reads `pub const POWERON: [u8; 0x0400] = [`.

`crates/sid-gpl-data/src/cutoff.rs`: move the two arrays out of `crates/mos6581/src/tables/fc.rs`, with this header:

```rust
//! Filter cutoff measurement points (cutoff register value, Hz).
//!
//! Filter cutoff measurement points: reSID (Dag Lem), GPL-2.0. The point
//! tables below match reSID's `f0_points_6581` and `f0_points_8580`.

/// 6581 cutoff curve points, including the step at register 1024.
pub const CUTOFF_POINTS_6581: &[(u32, f64)] = &[
    // the existing POINTS_6581 entries, unchanged
];

/// 8580 cutoff curve points (near-linear).
pub const CUTOFF_POINTS_8580: &[(u32, f64)] = &[
    // the existing POINTS_8580 entries, unchanged
];
```
(copy the entries verbatim from `fc.rs`; do not retype them.)

`crates/sid-gpl-data/README.md`:

```markdown
# sid-gpl-data

GPL-2.0-only. Measurement data borrowed from other projects, kept in one crate
so the rest of the SID stack can be MIT OR Apache-2.0. Each item will be
replaced by clean-room data; when the crate is empty it will be deleted.

| Item | Origin | Retired by |
|---|---|---|
| `WAVE6581`, `WAVE8580` | reSID (Dag Lem) waveform measurements | clean-room combined-waveform model |
| `CUTOFF_POINTS_6581`, `CUTOFF_POINTS_8580` | reSID `f0_points` | own cutoff measurements or a public model |
| `POWERON` | libsidplayfp `poweron.bin` | public C64 power-on documentation, or removal |
```

Move the data tests from `crates/mos6581/src/tables/mod.rs` (`tables_are_4096_entries_of_8_bits`, `the_6581_combination_tables_lose_amplitude`, `the_triangle_combination_rises_then_falls`, `the_combination_tables_are_distinct`) into a `#[cfg(test)] mod tests` in `crates/sid-gpl-data/src/lib.rs`, using `crate::{WAVE6581, WAVE8580}`. Keep `both_clock_variants_share_the_6581_tables` in mos6581.

- [ ] **Step 2: Wire it in (non-optional for now)**

Root `Cargo.toml` members: add `"crates/sid-gpl-data"`.

`crates/mos6581/Cargo.toml`:

```toml
[dependencies]
sid-gpl-data = { path = "../sid-gpl-data", version = "0.1" }
```

`crates/mos6581/src/tables/mod.rs`: remove `mod wave6581; mod wave8580;` and `pub use wave6581::WAVE6581; pub use wave8580::WAVE8580;`, and add:

```rust
const fn from_data(d: sid_gpl_data::WaveData) -> WaveTables {
    WaveTables {
        st: d.st,
        p_t: d.p_t,
        ps_: d.ps_,
        pst: d.pst,
    }
}

/// Measured 6581 combined waveforms (reSID data, GPL-2.0, via `sid-gpl-data`).
pub static WAVE6581: WaveTables = from_data(sid_gpl_data::WAVE6581);
/// Measured 8580 combined waveforms (reSID data, GPL-2.0, via `sid-gpl-data`).
pub static WAVE8580: WaveTables = from_data(sid_gpl_data::WAVE8580);
```

`crates/mos6581/src/tables/fc.rs`: delete `POINTS_6581` and `POINTS_8580` and the reSID attribution lines in the module doc; replace the uses with `sid_gpl_data::CUTOFF_POINTS_6581` / `sid_gpl_data::CUTOFF_POINTS_8580`.

`crates/sid-play/Cargo.toml` `[dependencies]`: add `sid-gpl-data = { path = "../sid-gpl-data", version = "0.1" }`.

`crates/sid-play/src/main.rs`: delete `mod poweron_table;` and replace `poweron_table::POWERON` with `sid_gpl_data::POWERON`.

`tools/gen_tables.py`: write to `crates/sid-gpl-data/src`, emitting `pub const WAVE{model}: crate::WaveData = crate::WaveData {` instead of the old static line. `tools/gen_poweron.py`: write to `crates/sid-gpl-data/src/poweron.rs`.

- [ ] **Step 3: Build, test, compare renders**

Same commands as Task 2 Step 4. Expected: all tests pass (the moved data tests now run in `sid-gpl-data`); all three hashes OK.

- [ ] **Step 4: Commit**

```bash
git add -A Cargo.toml Cargo.lock crates tools
git commit -m "refactor: quarantine the borrowed GPL data in the sid-gpl-data crate"
```

---

### Task 4: `gpl-data` feature and GPL-free defaults

**Files:**
- Create: `crates/mos6581/src/tables/default.rs`, `crates/mos6581/tests/gap.rs`
- Modify: `crates/mos6581/Cargo.toml`, `crates/mos6581/src/tables/mod.rs`, `crates/mos6581/src/tables/fc.rs`, `crates/mos6581/tests/filter.rs`, `crates/sid-play/Cargo.toml`, `crates/sid-play/src/main.rs`, `crates/sid-play/src/poweron.rs` (new)

**Interfaces:**
- Consumes: `sid_gpl_data::{WaveData, WAVE6581, WAVE8580, CUTOFF_POINTS_6581, CUTOFF_POINTS_8580, POWERON}` (Task 3).
- Produces: `mos6581::tables::default::{WAVE_AND: WaveTables, CUTOFF_POINTS: &[(u32, f64)]}`; with the feature, `WAVE6581`/`WAVE8580` as in Task 3. `for_model` and `cutoff_table` choose by feature. `sid_play` binary module `poweron` exposing `POWERON: [u8; 0x0400]` chosen by feature.

- [ ] **Step 1: Manifests**

`crates/mos6581/Cargo.toml`:

```toml
[dependencies]
sid-gpl-data = { path = "../sid-gpl-data", version = "0.1", optional = true }

[features]
default = []
# Measured reSID tables instead of the built-in approximations. Makes the build GPL-2.0.
gpl-data = ["dep:sid-gpl-data"]
```

`crates/sid-play/Cargo.toml`:

```toml
[dependencies]
mos6510 = "0.1"
mos6581 = { path = "../mos6581", version = "0.1" }
sid-gpl-data = { path = "../sid-gpl-data", version = "0.1", optional = true }

[features]
default = []
# Measured tables and the libsidplayfp power-on pattern. Makes the build GPL-2.0.
gpl-data = ["mos6581/gpl-data", "dep:sid-gpl-data"]
```

- [ ] **Step 2: Failing tests for the defaults**

Append to `crates/mos6581/tests/filter.rs`:

```rust
#[cfg(not(feature = "gpl-data"))]
#[test]
fn the_default_cutoff_is_the_datasheet_linear_range() {
    let lo = cutoff_hz(Model::Pal, 0);
    let hi = cutoff_hz(Model::Pal, 2047);
    assert!((lo - 30.0).abs() < 0.5, "fc=0 gave {lo} Hz");
    assert!((hi - 12_000.0).abs() < 1.0, "fc=2047 gave {hi} Hz");
    let mid = cutoff_hz(Model::Pal, 1024);
    let linear = 30.0 + (12_000.0 - 30.0) * 1024.0 / 2047.0;
    assert!((mid - linear).abs() < 1.0, "fc=1024 gave {mid} Hz, linear is {linear}");
}
```

and put `#[cfg(feature = "gpl-data")]` above `cutoff_spans_the_measured_6581_range` and `cutoff_rises_monotonically_except_at_the_top_of_the_low_half` (they assert facts of the measured curve). In `crates/mos6581/src/tables/fc.rs`, put `#[cfg(feature = "gpl-data")]` on its test module (it asserts the measured 8580 curve).

Create `crates/mos6581/src/tables/default.rs` with only its tests first:

```rust
//! Built-in data used when the `gpl-data` feature is off. Our own derivations,
//! MIT OR Apache-2.0.

use super::WaveTables;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saw_triangle_is_the_and_of_the_two() {
        // Phase 0x400: saw 0x400, triangle 0x800, AND 0.
        assert_eq!(WAVE_AND.st[0x400], 0);
        // Phase 0x7ff: saw 0x7ff, triangle 0xffe, AND 0x7fe, >> 4 = 0x7f.
        assert_eq!(WAVE_AND.st[0x7ff], 0x7f);
    }

    #[test]
    fn pulse_alone_passes_the_other_waveform() {
        assert_eq!(WAVE_AND.ps_[0xabc], 0xab);
        assert_eq!(WAVE_AND.p_t[0x400], 0x80);
    }

    #[test]
    fn three_way_matches_saw_triangle() {
        assert_eq!(WAVE_AND.pst, WAVE_AND.st);
    }
}
```

and add `mod default;` (make it `pub mod default;`) to `tables/mod.rs`.

Run: `cargo test -p mos6581` → expected: compile error, `WAVE_AND` not found.

- [ ] **Step 3: Implement the defaults**

In `crates/mos6581/src/tables/default.rs`, above the tests:

```rust
/// Triangle output for a phase, as the oscillator produces it with no ring
/// modulation and the accumulator's low 12 bits zero.
const fn triangle(phase: u16) -> u16 {
    if phase & 0x800 != 0 {
        ((!phase << 1) | 1) & 0x0fff
    } else {
        (phase << 1) & 0x0fff
    }
}

const fn and_tables() -> WaveTables {
    let mut t = WaveTables {
        st: [0; 4096],
        p_t: [0; 4096],
        ps_: [0; 4096],
        pst: [0; 4096],
    };
    let mut i = 0;
    while i < 4096 {
        let phase = i as u16;
        let saw_tri = phase & triangle(phase);
        t.st[i] = (saw_tri >> 4) as u8;
        // p_t is indexed by triangle >> 1; with pulse high it passes the triangle.
        t.p_t[i] = (((phase << 1) & 0x0fff) >> 4) as u8;
        t.ps_[i] = (phase >> 4) as u8;
        t.pst[i] = (saw_tri >> 4) as u8;
        i += 1;
    }
    t
}

/// Combined waveforms approximated as the bitwise AND of the pure waveforms:
/// the textbook model. Real silicon is quieter and less regular on combined
/// waveforms; the `gpl-data` feature swaps in measured tables.
pub static WAVE_AND: WaveTables = and_tables();

/// Cutoff points for the built-in curve: linear over the MOS 6581 datasheet's
/// nominal filter range, "approximately 30 Hz - 12 kHz" with 2200 pF filter
/// capacitors. The C64 fits other values and real chips vary; the `gpl-data`
/// feature swaps in measured curves.
pub const CUTOFF_POINTS: &[(u32, f64)] = &[(0, 30.0), (2047, 12_000.0)];
```

In `tables/mod.rs`, make `for_model` and the `WAVE6581`/`WAVE8580` statics feature-dependent:

```rust
#[cfg(feature = "gpl-data")]
const fn from_data(d: sid_gpl_data::WaveData) -> WaveTables { /* as in Task 3 */ }
#[cfg(feature = "gpl-data")]
pub static WAVE6581: WaveTables = from_data(sid_gpl_data::WAVE6581);
#[cfg(feature = "gpl-data")]
pub static WAVE8580: WaveTables = from_data(sid_gpl_data::WAVE8580);

/// The 6581 is the modelled chip, so both clock variants share its tables.
pub fn for_model(model: crate::Model) -> &'static WaveTables {
    match model {
        #[cfg(feature = "gpl-data")]
        crate::Model::Ntsc | crate::Model::Pal => &WAVE6581,
        #[cfg(not(feature = "gpl-data"))]
        crate::Model::Ntsc | crate::Model::Pal => &default::WAVE_AND,
    }
}
```

The `both_clock_variants_share_the_6581_tables` test: keep its first assertion (Pal and Ntsc resolve to the same table) unconditional and put the `WAVE6581`/`WAVE8580` comparison under `#[cfg(feature = "gpl-data")]`.

In `tables/fc.rs`, select the points:

```rust
#[cfg(feature = "gpl-data")]
const POINTS_6581: &[(u32, f64)] = sid_gpl_data::CUTOFF_POINTS_6581;
#[cfg(feature = "gpl-data")]
const POINTS_8580: &[(u32, f64)] = sid_gpl_data::CUTOFF_POINTS_8580;
#[cfg(not(feature = "gpl-data"))]
const POINTS_6581: &[(u32, f64)] = super::default::CUTOFF_POINTS;
#[cfg(not(feature = "gpl-data"))]
const POINTS_8580: &[(u32, f64)] = super::default::CUTOFF_POINTS;
```
and use `POINTS_6581` / `POINTS_8580` in `cutoff_table` / `cutoff_table_8580`. (Two points give a straight line through the existing monotone Hermite interpolation.)

- [ ] **Step 4: Power-on area in sid-play**

Create `crates/sid-play/src/poweron.rs`:

```rust
//! The RAM pattern at `$0000-$03FF` before a tune's init runs.

/// With `gpl-data`: libsidplayfp's captured pattern (GPL-2.0, via
/// `sid-gpl-data`). Without it: zeros; the harness still writes the Kernal
/// pointers it sets for every tune.
#[cfg(feature = "gpl-data")]
pub const POWERON: [u8; 0x0400] = sid_gpl_data::POWERON;
#[cfg(not(feature = "gpl-data"))]
pub const POWERON: [u8; 0x0400] = [0; 0x0400];
```

In `crates/sid-play/src/main.rs` add `mod poweron;` and replace `sid_gpl_data::POWERON` with `poweron::POWERON`.

- [ ] **Step 5: The table gap test**

`crates/mos6581/tests/gap.rs`:

```rust
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
```

- [ ] **Step 6: Run both configurations**

```bash
cargo test --workspace
cargo test --workspace --features sid-play/gpl-data
cargo test -p mos6581 --features gpl-data --test gap -- --nocapture
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features sid-play/gpl-data -- -D warnings
cargo fmt --check
cargo tree -p sid-play -e normal | grep -c sid-gpl-data        # expect 0
cargo tree -p sid-play -e normal --features gpl-data | grep -c sid-gpl-data   # expect >= 1
```

If a test outside the ones named above fails only in the default build, decide which kind it is and record it in the report: if it asserts a fact of the *measured* data, gate it with `#[cfg(feature = "gpl-data")]` and say why; if it asserts something any SID must do (sound comes out, the filter attenuates, no clipping), the default data is wrong and must be fixed, not the test.

- [ ] **Step 7: Bit-identical with the feature, and a default-build smoke render**

```bash
cargo build --release -p sid-play --features gpl-data
./target/release/render crates/sid-play/tests/fixtures/scale.sid --seconds 5 --out target/split-check/scale.wav
./target/release/render crates/sid-play/tests/fixtures/irq.sid --seconds 5 --out target/split-check/irq.wav
./target/release/render /Users/enzo/Code/chipsynth64/last_ninja.sid --seconds 5 --out target/split-check/last_ninja.wav
(cd target/split-check && shasum -a 256 -c ../split-baseline/SHA256SUMS)
cargo build --release -p sid-play
./target/release/render /Users/enzo/Code/chipsynth64/last_ninja.sid --seconds 5 --out target/split-check/last_ninja-default.wav
```
Expected: three `OK`; the default render completes with a non-zero chip-write count.

- [ ] **Step 8: Commit**

```bash
git add -A crates Cargo.lock
git commit -m "feat: gpl-data feature, off by default, with GPL-free built-in tables"
```

---

### Task 5: Relicense, fidelity-gap script, documentation

Precondition: `docs/provenance.md` has no unresolved **Borrowed** row outside `sid-gpl-data`.

**Files:**
- Create: `crates/{mos6581,sid-play}/{LICENSE-MIT,LICENSE-APACHE,README.md}`, `crates/sid-play/scripts/fidelity-gap.sh`, `crates/sid-play/scripts/tonality.py`
- Modify: `crates/mos6581/Cargo.toml`, `crates/sid-play/Cargo.toml` (`license`), `HANDSOFF.md`, the spec's Status line

- [ ] **Step 1: Licences**

In both `crates/mos6581/Cargo.toml` and `crates/sid-play/Cargo.toml` set `license = "MIT OR Apache-2.0"`. Copy `LICENSE-MIT` and `LICENSE-APACHE` from `/Users/enzo/Code/chipsynth64/mos6510/` into both crate directories. Delete the root-level GPL `LICENSE` file if one exists (the GPL text now lives in `crates/sid-gpl-data/LICENSE`).

- [ ] **Step 2: Tonality metric and gap script**

`crates/sid-play/scripts/tonality.py`:

```python
"""Print the median autocorrelation peak (tonality) of a mono 16-bit WAV."""
import array, sys, wave

def peak(seg, sr):
    n = len(seg); m = sum(seg) / n; seg = [v - m for v in seg]
    e = sum(v * v for v in seg)
    if e < 1e-9:
        return 0.0
    return max(sum(seg[i] * seg[i + lag] for i in range(0, n - lag, 2)) * 2 / e
               for lag in range(sr // 2000, sr // 60, 2))

w = wave.open(sys.argv[1]); sr = w.getframerate(); ch = w.getnchannels()
a = array.array('h'); a.frombytes(w.readframes(w.getnframes()))
x = list(a[::ch])[::4]; r = sr // 4
ps = sorted(peak(x[s:s + r // 10], r) for s in range(r, len(x) - r // 10, 2 * r))
print(f"{ps[len(ps) // 2]:.2f}")
```

`crates/sid-play/scripts/fidelity-gap.sh`:

```sh
#!/bin/sh
# Render each tune with the default (GPL-free) build and the gpl-data build and
# print the tonality median of each. Usage: fidelity-gap.sh tune.sid...
set -eu
cd "$(dirname "$0")/../../.."
out=$(mktemp -d)
cargo build --release -q -p sid-play && cp target/release/render "$out/render-default"
cargo build --release -q -p sid-play --features gpl-data && cp target/release/render "$out/render-gpl"
printf '%-36s %8s %8s %10s\n' tune default gpl-data rms-diff
for f in "$@"; do
    n=$(basename "$f" .sid)
    "$out/render-default" "$f" --seconds 10 --out "$out/$n-d.wav" > /dev/null
    "$out/render-gpl" "$f" --seconds 10 --out "$out/$n-g.wav" > /dev/null
    printf '%-36s %8s %8s %10s\n' "$n" \
        "$(python3 -I crates/sid-play/scripts/tonality.py "$out/$n-d.wav")" \
        "$(python3 -I crates/sid-play/scripts/tonality.py "$out/$n-g.wav")" \
        "$(python3 -I crates/sid-play/scripts/tonality.py --rms-diff "$out/$n-d.wav" "$out/$n-g.wav")"
done
```

Add the `--rms-diff A B` mode to `tonality.py` (before the tonality code, exit after printing): read both WAVs as 16-bit mono, compare over the shorter length, print the RMS of the sample difference divided by 32768 with 4 decimals:

```python
if len(sys.argv) == 4 and sys.argv[1] == "--rms-diff":
    def load(p):
        w = wave.open(p); ch = w.getnchannels()
        a = array.array('h'); a.frombytes(w.readframes(w.getnframes()))
        return a[::ch]
    a, b = load(sys.argv[2]), load(sys.argv[3])
    n = min(len(a), len(b))
    rms = (sum((a[i] - b[i]) ** 2 for i in range(n)) / max(n, 1)) ** 0.5 / 32768
    print(f"{rms:.4f}")
    sys.exit(0)
```

Run it on the PSIDs in `/private/tmp/corpus` (filter with `head -c4 "$f" | grep -q PSID`) plus `last_ninja.sid`, and on the two fixtures. Record the table. Any tune whose default-build tonality is `0.00` (silent) is called out explicitly in the README and HANDSOFF, not hidden.

- [ ] **Step 3: READMEs**

`crates/mos6581/README.md`: what the crate is (cycle-exact SID, one Φ2 step at a time), licence MIT OR Apache-2.0, and a "Fidelity" section stating plainly: the default build uses AND-approximated combined waveforms and the datasheet's linear 30 Hz to 12 kHz cutoff; `--features gpl-data` swaps in measured reSID tables and makes the build GPL-2.0; include the gap numbers printed by `tests/gap.rs`. `crates/sid-play/README.md`: usage of the `render` binary (`cargo run --release -p sid-play --bin render -- tune.sid --seconds 20 --out out.wav`, `--subsong N` 0-based, `--trace`), the same licence and feature note, and the fidelity-gap table from Step 2. No em-dashes.

- [ ] **Step 4: HANDSOFF and spec**

HANDSOFF.md: replace the "Licensing — hard constraints" section with the new arrangement (permissive crates, quarantine crate, feature, clean-room roadmap from the spec), link `docs/provenance.md`, and add the fidelity-gap table. Spec status line: `Status: implemented; see the plan and HANDSOFF.md.`

- [ ] **Step 5: Final checks and commit**

```bash
cargo test --workspace && cargo test --workspace --features sid-play/gpl-data
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features sid-play/gpl-data -- -D warnings
cargo fmt --check
git add -A crates HANDSOFF.md docs
git commit -m "chore: relicense mos6581 and sid-play as MIT OR Apache-2.0; document the GPL quarantine"
```
