# Removing sid-gpl-data: design

Date: 2026-11-12. Status: approved in brainstorming.

## Goal

Delete the GPL-2.0 crate `sid-gpl-data` and the `gpl-data` feature from `mos6581` and
`sid-play`, after replacing the items that matter with clean-room models that pass
fidelity thresholds fixed in this document.

## Decisions

- Sequence: close the gaps first, delete the crate last, in one breaking release.
- Scope: the 6581 cutoff curve, the 6581 combined waveforms, and the power-on RAM
  image. The 8580 data (`WAVE8580`, `CUTOFF_POINTS_8580`) is used by nothing in the
  chip path and is dropped, not replaced. The chip constants keep their current
  clean values; their gaps stay documented in `mos6581/docs/provenance.md`.
- Oracle rule: an evaluator compares each clean model against the measured GPL data
  and returns only PASS or FAIL per metric. No numbers, curves, diffs or hints reach
  the author. At most 3 author rounds per item. If no round passes, the evaluator
  also reports PASS/FAIL for "better than today's default on every metric"; the
  best round that passes that check ships, otherwise today's default stays. The outcome is documented in
  either case.

## Roles (same clean-room method as the chip-constant round)

- **Author**: an isolated agent. Gets only names, types, semantics, the public
  sources listed below and the PASS/FAIL results of earlier rounds. Forbidden: all
  emulator sources (reSID, reSIDfp, libsidplayfp, VICE, others), `sid-gpl-data`,
  the provenance Evidence column, `tests/gap.rs`, the fidelity scripts, git history.
  Every value carries a `Source:` line or is marked "unsourced own choice".
- **Evaluator**: runs the metrics below against the GPL data, outside the published
  repos, and reports PASS/FAIL only.
- **Reviewer**: a separate agent with public sources only. Checks every `Source:`
  claim and that nothing looks lifted. It must not suggest values.

Public sources allowed: MOS 6581 datasheet; US patent 4,677,890; C64 schematic
250466 (and other publicly posted C64 board schematics); oxyron's SID notes; the
Yannes interview; the Commodore 64 Programmer's Reference Guide and published Kernal
memory maps; general textbook electronics (MOSFET triode model, NMOS logic).

## Item 1: 6581 cutoff curve

Model: a voltage-controlled-resistor integrator. The 11-bit cutoff register drives a
DAC; the DAC voltage drives the gate of a FET acting as the integrator resistor, so
`f0 = 1 / (2π R(Vdac) C)`. `C` is the C64's filter capacitor value from the public
schematic. `R(V)` follows a textbook triode FET model: a leakage floor below
threshold (the closed corner), a near-linear rise above it, and saturation at the
top. Output: a 2048-entry `f64` curve or point list feeding the existing
`CutoffTable` (`src/tables/fc.rs`), replacing `default::CUTOFF_POINTS`.

Pass thresholds (all must pass):
- Closed corner (fc = 0) within a factor of 1.5 of the measured value.
- Median absolute log2 error over fc = 0..2047 below 0.5 (half an octave).
- sid-play `scale` fixture RMS within a factor of 2 of the `gpl-data` build.

## Item 2: 6581 combined waveforms

Model: shared-line pull-down. When several waveforms are selected their outputs
share the DAC lines through NMOS drivers, so each output bit is a threshold of a
weighted window over neighbouring combined bits; zeros pull neighbours down.
Pulse gates first (pulse low forces the line low). Weights, window and threshold
are own choices grounded in the patent and oxyron's notes. Output: `st`, `p_t`,
`ps_`, `pst` tables of the existing `WaveTables` type, replacing `WAVE_AND`.

Pass thresholds (all must pass):
- Each of the four tables has an RMS error against measured at least 40% lower than
  the AND tables' (today: st 83.8, p_t 139.2, ps_ 140.9, pst 84.4 of 255).
- No table worse than AND.
- `pst` differs from `st` (the mutual-distinctness test then runs in every build).

## Item 3: power-on RAM

Model: the documented post-reset state of `$0000-$03FF`: zero-page system
variables, the stack page left as zeros unless documented, `$0300-$0333` BASIC and
Kernal vectors, and other page 2/3 defaults, each byte or range carrying a
`Source:` from the Programmer's Reference Guide or a published Kernal memory map.
Undocumented bytes stay zero. Output: `sid-play/crates/sid-play/src/poweron.rs`
`POWERON`, no longer feature-gated.

Pass thresholds (all must pass):
- At least 90% of the 1024 bytes match the measured pattern.
- All 17 fixture tunes (16 HVSC plus The Last Ninja) render with tonality median
  within 0.05 of the `gpl-data` build.

## Finale: delete the crate (one breaking release)

- `mos6581`: remove the `gpl-data` feature, the `sid-gpl-data` dependency, every
  `cfg(feature = "gpl-data")` branch, `chip_data.rs` switch (use
  `clean_chip_data` directly), `cutoff_table_8580`, gated tests and `tests/gap.rs`.
  Update README and `docs/provenance.md` (record the final gaps). Publish 0.2.0.
- `sid-play`: remove the feature and dependency, `scripts/fidelity-gap.sh` and the
  gpl paths in docs and HANDSOFF.md. Publish 0.2.0.
- The gap tooling moves to an unpublished evaluator area under the session
  scratchpad; it is not committed to any public repo.
- `sid-gpl-data`: a final 0.1.2 whose README says it is deprecated and unused, then
  yank all versions and archive the GitHub repo.
- Check: `cargo tree` in both repos shows no `sid-gpl-data`; no `gpl-data` string
  remains outside changelogs and provenance history; all tests pass; the 17 tunes
  play.

## Order of work

Cutoff curve, then combined waveforms, then power-on, then the finale. Each item is
its own task cycle: author, evaluate (up to 3 rounds), review, commit with
`provenance.md` updated.
