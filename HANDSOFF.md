# rust-sid — handoff notes

Branch `feat/faithful-chip`. 7 test targets green (91 tests),
`cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check`
both clean.

> **Update 2026-11-12: the silent-tune problem was the CPU, not the tunes.**
> `lib6502` 0.2.2 fails Klaus Dormann's 6502 functional test early: `(zp),Y`
> never adds Y, binary `ADC`/`SBC` never set C/N/V, `SBC` subtracts the carry
> instead of the borrow, and `PHP`/`BRK` push a corrupted status byte.
> `examples/render/cpu_fixups.rs` executes those instructions correctly; with it,
> **16/16 PSIDs play** (previously 3). The CPU still fails the test further on
> (next: `LSR A` flags at `$230C`), so more defects remain. See "CPU defects"
> below. Sections further down that say the tunes "decided not to play" are
> superseded.

## What this is

A MOS 6581 SID chip emulator in Rust, cycle-accurate at the Φ2 rate, plus a
PSID harness that runs a tune's 6510 code and renders it through the chip to a
WAV file.

The library is the chip only — no CPU, no memory map, no player. `mos6502` and
`lib6502` are dev-dependencies used by the example and tests.

```
src/            1929 lines   the chip: wave, envelope, voice, filter, extfilt,
                             resample, sink, sid, tables (generated)
tests/          1532 lines   behavioural tests per component + fixture renders
examples/render.rs ~1200     PSID loader + 6510 harness + trace diagnostics
examples/render/cpu_fixups.rs  correct execution of instructions lib6502 breaks
examples/cpu_conformance.rs    runs Klaus Dormann's functional test on lib6502
docs/superpowers/specs/      the design spec
docs/superpowers/plans/      plan 1 (faithful chip)
.superpowers/sdd/            task briefs, reports, review packages, ledger
```

## Build and run

```sh
cargo test
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release --example render -- tune.sid --seconds 20 --out out.wav
cargo run --release --example render -- tune.sid --seconds 5 --out /tmp/x.wav --trace
```

`--trace` is inert unless passed. It prints one `TRACE heard=...` summary line
plus per-site detail: ROM-area executions, ROM reads, vector rewrites, a
per-register SID write histogram, an out-of-range write count, and every address
read after init returned (marked in-block or not).

## What is verified working

The chip itself. Two fixtures in `tests/fixtures/` exercise it end to end:

- `scale.sid` — clock-driven; steps a frequency table through a scale.
- `irq.sid` — installs its own `$FFFE` handler, programs CIA 1 timer A at latch
  `$4C00`, and refuses to touch the chip until the handler has run. Sustains
  **103 interrupts in 2 s** against the expected ~50 Hz, with audio to the last
  sample.

Together these cover oscillator, combination-waveform tables, noise clocking,
hard sync, ring modulation, envelope, the voice DC model, filter routing and
resonance, mixer DC, the output-stage high-pass, the sinc resampler, the sink,
the hand-rolled interrupt entry and the CIA timer.

Three real tunes render sustained tonal audio: `Action_Biker`, `5_Title_Tunes`,
`05-09-87` (autocorrelation peaks 0.74–0.99, so tonal rather than noise).

## CPU defects in lib6502 0.2.2

Found with `examples/cpu_conformance.rs` against Klaus Dormann's
`6502_functional_test.bin` (fetch it from
github.com/Klaus2m5/6502_65C02_functional_tests, `bin_files/`; success trap is
`$3469` for that build):

```sh
cargo run --release --example cpu_conformance -- 6502_functional_test.bin 3469
PATCH=1 cargo run --release --example cpu_conformance -- 6502_functional_test.bin 3469
```

Without `PATCH` it traps at `$0674`; with it, at `$230C`. Worked around so far:

| Defect | Effect on tunes |
|---|---|
| `(zp),Y` ignores Y (`addressing.rs`, `IndirectIndexed`) | every pattern/instrument fetch reads the wrong byte |
| `(zp,X)` does not wrap in the zero page | wrong pointer near `$FF` |
| `ADC`/`SBC` binary mode never set C, N, V on NMOS/CMOS (only the NES branch does); `ADC` never clears Z | every 16-bit add and every compare-by-subtract breaks |
| `SBC` subtracts C instead of the borrow `1 - C` | off-by-one in every subtraction |
| `PHP` ORs `get_flag(..) as u8` (0/1) in, setting Carry instead of B/bit 5, then clears B/bit 5 in P | `PHP`/`PLP` corrupts carry |
| `BRK` pushes P without bit 5 | minor |
| Every undocumented opcode (`LAX`, `SAX`, `DCP`, ...) is an empty stub | none in this corpus (trace shows 0) |

Not yet worked around: whatever lies past `$230C` (shift/rotate flags first).
`clock()` also calls `disassemble_instruction_at` on every instruction, which
allocates a `String` and reads the operand bytes through the bus.

The renderer applies the fixups unless `NO_FIXUPS` is set, which keeps the old
behaviour available for A/B comparison.

## What does not work — the honest coverage number

*Superseded by the update above: with the CPU fixups, 16/16 PSIDs render tonal
audio (autocorrelation medians 0.41–0.88). Kept for the record.*

From 24 tunes sampled from HVSC (six composers):

- **8 are RSID**, not PSID — magic `RSID`, meaning "recorded on a real C64 with
  ROMs enabled". The loader refuses them, correctly and by design.
- **16 are PSID; 3 play.** Roughly 19% of the format we claim to support.

The 13 failures are not one findable bug. See the investigation log below.

## Licensing — hard constraints, do not change casually

- Licence is **`GPL-2.0-only`**, deliberately. `src/tables/wave6581.rs` and
  `src/tables/wave8580.rs` contain waveform measurement data **byte-identical to
  reSID's `.dat` files** (verified). That data is GPL-2.0 and cannot be
  redistributed under a permissive licence. Re-licensing to MIT would require
  first removing those tables and regenerating them clean-room.
- Every generated table file carries `Waveform measurement data: reSID (Dag
  Lem), GPL-2.0.` — licence-required attribution. Do not strip it.
- `examples/render/poweron_table.rs` is derived from libsidplayfp's
  `poweron.bin` capture (also GPL-2.0) and is **byte-identical to it across all
  1024 addresses**.
- The library keeps **zero runtime dependencies**. `lib6502` and `mos6502` are
  dev-dependencies only.

## Upstream bug filed

<https://github.com/drewwalton19216801/lib6502/issues/4>

`lib6502`'s `do_interrupt` sets the I flag *before* pushing status, so the
stacked P carries `I = 1`, every `RTI` restores interrupts masked, and only one
interrupt can ever be taken. It also clears I for the handler (hardware leaves
it set) and pushes B set for a hardware IRQ (B=1 distinguishes BRK).

**Consequence: never call `cpu.irq()`.** `examples/render.rs` performs the
interrupt entry by hand (`irq_entry`) using lib6502's public `pc`, `sp`,
`status` and `cycles` fields. The repo has 0 stars and last saw a push in
October 2024 — do not plan around a fix landing.

## Investigation log: why 13 PSIDs are silent

Every systemic hypothesis was tested against evidence, not reasoning. All of
these are eliminated:

| Hypothesis | Result |
|---|---|
| Loader misplaces the data block | Fixed in `08c298e`, validated against libsidplayfp's own rules |
| Power-on zero-page state wrong | Our table is byte-identical to libsidplayfp's capture |
| Writes past register 31 silently discarded | **0** on all 16 tunes |
| Tunes call Kernal ROM routines | **1** tune in 16 (`Athena`), and it never reaches the chip |
| Tunes wait for an interrupt | **0** interrupts taken across all 24 tunes |
| Tunes read the raster counter `$D012` | **0** reads across all 16 |
| `init` does not finish | Returns successfully for **16/16** |
| Note data reads as untouched RAM | Failing tune reads its **own in-block tables** (`$5396-$539E`) |
| Tunes execute undocumented opcodes, which `lib6502` 0.2.2 stubs as no-ops | **0** executions across all 16 (`TRACE stubbed`; positive control with `LDA #` flagged caught 1584) |
| **The CPU executes documented instructions wrong** | **Confirmed.** Conformance test fails; fixing `(zp),Y`, `ADC`/`SBC`, `PHP`, `BRK` makes 16/16 play |

Two traps worth recording, because they nearly produced false conclusions:

- The dominant memory reads in every trace — `$0410/$0411/$0412` at 1.93 M each,
  two thirds of all CPU activity — are **our own halt stub self-looping**
  (`JMP $0410`) while the harness waits between frames. Benign, and present
  identically in working and failing tunes.
- A VIC raster model landed as one blob (`d576a71`) including `$D011/$D012`
  reads, write interception on `$D000-$D01F` and a `$FFFE` driver, and
  **regressed** acceptance from 2 tunes to 1. Reverted in `8eaf07d`. The
  subsequent single-variable test showed zero tunes read `$D012` at all — the
  reads were innocent; the blob was the mistake.

What remains: the failing tunes run their player loop, write all 21 voice
registers plus volume once per frame, read their own tables, and never set a
waveform nibble. The tune is executing correctly and has decided not to play.
That is per-tune reverse engineering, not a systemic defect.

## Rulings made during the project

Decisions taken on your behalf, with what they cost if wrong:

1. **Branch instead of worktree.** The repo is nested inside `chipsynth64` and
   the sandbox only writes under the workspace and temp dirs. Cost: none,
   trivially re-rooted.
2. **Removed the `level == 0 → output = 0` rule** the plan mandated, in favour
   of hardware behaviour (voice keeps `VOICE_DC` at zero envelope). The plan was
   wrong against hardware: the hard zero is a 50%-of-range step at every release
   — a click. Silence is achieved downstream by the output-stage high-pass.
3. **Removed the brief's hard-sync condition**, which tested the grandparent
   oscillator's MSB flag instead of the source's, so sync never locked. Inherited
   verbatim from the plan text.
4. **Accepted a documented limitation rather than building ROM support.**
   RSID and Kernal-dependent tunes are out of scope. Cost: the demo plays a
   minority of tunes.
5. **Kept `GPL-2.0-only`** when MIT was requested, because the vendored reSID
   data cannot be relicensed. The MIT path requires clean-room tables first.
6. **Declined to build a Kernal stub** after the trace showed one tune in
   sixteen touches ROM. Cost: if the trace is wrong, a stub is one task of work.

## Known deferred minors

From the review ledger (`.superpowers/sdd/progress.md`), none blocking:

- `Filter::reset()` does not clear `volume`, `routed` or `taps`; the chip
  re-applies them, but the asymmetry is a trap.
- `Filter::set_fc` produces NaN if `clock_hz` is 0 (public API, unguarded).
- `extfilt` hardcodes `DT = 1e-6` rather than the real Φ2 rate, so poles sit
  ~1.5–2.2% off; the "16 kHz" comment is really ~17.4 kHz.
- Envelope-counter flip quirks (attack 255→0, release 0→255) unmodelled.
- The "source is itself being synced the same cycle" skip clause is untested.
- The no-clip test runs at resonance 0; at resonance 15 near the corner the
  worst case approaches ~0.85 and is untested.
- PSID v4/v5 trailing-data exclusion not implemented (all sampled tunes are v2).
- `ResampleFast` accuracy untested; it discards sub-chip-sample output phase.
- `--subsong` is now **0-based**, matching the register init reads. This is a
  user-visible change from earlier behaviour.

## Next steps, ranked

*Revised 2026-11-12, replacing the list kept below for the record:*

1. **Replace `lib6502`.** Patching it instruction by instruction is whack-a-mole
   and the conformance test still fails. Either write an NMOS 6510 core in the
   harness (small, and it gives exact cycles, correct IRQ entry and the
   undocumented opcodes) or adopt a crate that passes Klaus Dormann's functional
   and decimal tests. The acceptance gate is that test passing, plus the corpus.
2. Re-check `Axel_F` (0.41) and `Ark_Pandora` (0.47), the least tonal renders,
   once the CPU passes the test.
3. Report the defects upstream on lib6502 issue #4 or a new issue.

*Original list:*

1. **Stop, and document the limit.** The chip is verified; the demo plays some
   tunes. Write the coverage honestly into the README and ship it as an emulator
   library rather than a player.
2. **If real-tune coverage matters**, the credible path is adopting
   libsidplayfp's architecture wholesale — full C64 memory map with the PLA and
   `$01` banking, plus its `psiddrv` in-machine driver — not continuing to diff
   it piece by piece. This is substantially bigger than the remaining gaps look.
3. **Per-tune investigation** of the 13 silent PSIDs, one tune at a time, using
   `--trace`. Cheap to start, but there is no evidence a shared cause remains.

What I did **not** check, so it isn't hidden: I never compared the *values*
written against what a reference player produces for the same tune — that needs
libsidplayfp built here, which it isn't. The 24-tune sample is six composers,
not a representative HVSC slice. And I never confirmed that the 13 silent tunes
actually play in a real emulator, so "the tune decided not to play" is an
inference from its register writes. It remains possible that one shared subtle
bug produces all 13 and the seven eliminated candidates were the wrong seven.
