# Crate split: permissive chip and player, quarantined GPL data

Date: 2026-11-12. Status: implemented; see the plan and HANDSOFF.md.

## Why

The long-term goal is a SID stack with no GPL-2.0 code at all, reached by
clean-room work on whatever cannot be obtained otherwise. Today rust-sid is one
`GPL-2.0-only` crate because a few data tables are borrowed from GPL projects.
That makes every line of our own code GPL too.

This split confines the borrowed data to one small crate that the permissive
crates use only through an off-by-default feature. Default builds are GPL-free.
The GPL crate then shrinks one table at a time as clean-room replacements land,
and is deleted when empty.

## Target layout

| Crate | Licence | Contents | Where |
|---|---|---|---|
| `mos6510` | MIT OR Apache-2.0 | CPU core | own repo, done |
| `mos6581` | MIT OR Apache-2.0 | oscillators, envelope (formerly the `adsr6581` idea), filter, external filter, mixer, resampler, `Sid` | `rust-sid/crates/mos6581` |
| `sid-play` | MIT OR Apache-2.0 | PSID loader, CIA, harness, trace, WAV sink, `render` binary | `rust-sid/crates/sid-play` |
| `sid-gpl-data` | GPL-2.0-only | borrowed data only, never code | `rust-sid/crates/sid-gpl-data` |

The rust-sid repository becomes a cargo workspace with those three members.
`mos6510` stays a separate repository and is consumed from crates.io once
published (path dependency until then).

## What is GPL-derived today

| Item | Current file | Source |
|---|---|---|
| Combined waveform tables, 6581 and 8580 | `src/tables/wave6581.rs`, `wave8580.rs` | reSID (Dag Lem) |
| Filter cutoff measurement points, 6581 and 8580 | `src/tables/fc.rs` (`POINTS_6581`, `POINTS_8580`) | reSID `f0_points` |
| Power-on RAM pattern `$0000-$03FF` | `examples/render/poweron_table.rs` | libsidplayfp `poweron.bin` |

**Provenance audit (first task of the plan).** Before any file is relicensed,
every remaining source file is checked for further borrowed material: constants,
tables, or code transcribed from reSID, libsidplayfp or other GPL sources
(for example envelope rate periods, filter or external-filter constants, the
resampler). Each finding is either moved into `sid-gpl-data`, shown to come
from a non-GPL source (datasheet, public measurement, own derivation) with the
source recorded in a comment, or rewritten. The audit's result is a table in
the plan's report; nothing is relicensed until it is complete.

## `sid-gpl-data`

- Data only: `pub static` arrays and slices, no functions beyond trivial
  accessors, no dependency on any other crate (so it cannot create a cycle and
  carries no logic of ours under GPL).
- Exposes: `WAVE6581_{ST,PT,PS,PST}: [u8; 4096]` and the same for 8580;
  `CUTOFF_POINTS_6581`, `CUTOFF_POINTS_8580: &[(u32, f64)]` (the types
  `fc.rs` uses today); `POWERON: [u8; 0x400]`.
- Every item keeps its attribution line. The crate README lists each item, its
  origin, and the clean-room task that will retire it.

## `mos6581`

- Same public API as `rust_sid` today (`Sid`, `Model`, modules), renamed crate.
- Defines its own table types (`WaveTables`, cutoff table) and chooses the data
  source at compile time:
  - feature `gpl-data` (off by default): uses `sid-gpl-data`.
  - default: built-in permissive data:
    - **Combined waveforms:** the bitwise AND of the pure waveforms' 12-bit
      outputs, reduced to the 8-bit table format. The textbook approximation;
      audibly simpler than measured silicon on combined waveforms only.
    - **Cutoff curve:** a monotone curve from the published datasheet range
      (6581: about 30 Hz to 12 kHz; 8580: about 30 Hz to 12 kHz, near-linear),
      with the exact figures and the datasheet reference recorded in the code.
      The implementation task verifies these figures against the datasheets
      before using them.
- The crate README states plainly: default build is MIT/Apache with reduced
  fidelity on combined waveforms and filter cutoff; `--features gpl-data`
  restores measured tables and makes the build GPL-2.0.

## `sid-play`

- The current `examples/render.rs` becomes a library module plus a `render`
  binary; `poweron_table.rs` moves to `sid-gpl-data`.
- Feature `gpl-data = ["mos6581/gpl-data", "dep:sid-gpl-data"]`, off by
  default. Without it the power-on area is zeros plus the Kernal pointers the
  harness already writes.
- The fixture tests (`tests/fixture_render.rs`, `tests/fixtures/`) move here.

## Measuring the gap

The fidelity cost of the default build is measured, not guessed:

- A test in `mos6581`, run with `--features gpl-data`, reports the RMS and
  maximum difference between the default and the measured tables (per table).
- A script `scripts/fidelity-gap.sh` renders the fixtures and a list of tunes in
  both builds and prints, per tune, the tonality median of each and the RMS
  difference of the outputs. Its output for the 16-tune corpus is recorded in
  `HANDSOFF.md`.

## Clean-room roadmap (out of scope here, one spec each later)

1. Cutoff curve: own measurements or a published non-GPL model.
2. Combined waveforms: a model of the waveform DAC and its bit interactions
   derived from public documentation, checked against measurements.
3. Power-on pattern: derive from public documentation of the C64 RAM power-on
   state, or drop it if no tune in a larger corpus depends on it.

Each one removes an item from `sid-gpl-data`; the last one deletes the crate
and the feature.

## Non-goals

- Changing chip behaviour. With `--features gpl-data` the output must be
  identical to today's (bit-exact WAVs for the fixtures and one corpus tune).
- Publishing `mos6581` / `sid-play` to crates.io (a separate decision).
- Any clean-room work itself.

## Acceptance

- Workspace builds; `cargo test --workspace` and
  `cargo test --workspace --features sid-play/gpl-data` both pass;
  clippy and fmt clean.
- `cargo tree -p sid-play -e normal` (default features) contains no
  `sid-gpl-data`; with the feature it does.
- `--features gpl-data` renders are bit-identical to the pre-split renders for
  `tests/fixtures/*.sid` and `last_ninja.sid` (5 s each).
- Default-build renders of the corpus are recorded with the gap script; any tune
  that goes silent in the default build is reported, not hidden.
- Licence files: MIT + Apache in `mos6581` and `sid-play`, GPL-2.0 in
  `sid-gpl-data`; each `Cargo.toml` `license` field matches.
- The provenance audit table exists and every finding is resolved.
