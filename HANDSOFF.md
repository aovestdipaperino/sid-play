# rust-sid - handoff notes

Branch `feat/faithful-chip`. 7 test targets green (91 tests),
`cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check`
both clean.

> **Update 2026-11-12: the silent-tune problem was the CPU, not the tunes.**
> `lib6502` was the cause. It has been replaced by the crate
> [`mos6510`](https://crates.io/crates/mos6510) 0.1 (cycle-exact NMOS 6510, MIT OR Apache-2.0, passes Tom Harte's
> SingleStepTests 256/256 and Klaus Dormann's functional test). The CIA now
> drives the CPU's IRQ pin, and the 6510 port starts as the Kernal leaves it.
> Coverage: **16/16 corpus PSIDs plus Last Ninja play tonal audio** (10 s
> renders, autocorrelation medians 0.40 to 0.89). "Old" is the CPU with a
> partial instruction-fix layer; before any fix, 13 of the 16 were silent.
> No tune has a median below 0.40, but `Axel_F` sits right at it, and
> per-window minimums go lower (`Axel_F` 0.12, `Ark_Pandora` 0.24).
> The medians of six tunes dropped: 4_Soccer_Sims, 5_Title_Tunes, ACE_II,
> All_Terrain_Vehicle (the most, 0.77 to 0.69), Alternative_World_Games and
> `Axel_F`. All_Terrain_Vehicle was investigated: the register write streams
> of the two builds are identical except voice 3 frequency, because the tune
> reads through a null pointer (`LDA ($F0),Y` with `$F0/$F1 = $0000`). On a
> C64 that read hits the 6510 port register at `$00` (`$2F`); the old build
> had no port and read `0`. Giving the old build the port values made its
> writes byte-identical to the new build. So that drop is the new core being
> right. The other five drops were not investigated individually.
> Sections further down that say the tunes "decided not to play" are
> superseded.

Tonality medians over 10 s renders. "Old" is lib6502 plus the partial
instruction-fix layer; "New" is mos6510. Before any CPU fix, 13 of the 16
PSIDs were silent.

| Tune | Old median (lib6502 + partial fixes) | New median (mos6510) | New min |
|---|---|---|---|
| 05-09-87 | 0.61 | 0.62 | 0.48 |
| 4_Soccer_Sims_Soccer_Skills | 0.68 | 0.66 | 0.52 |
| 5_Title_Tunes | 0.72 | 0.71 | 0.66 |
| 64_Dwarfs_on_XTC | 0.88 | 0.89 | 0.70 |
| 720_Degrees | 0.83 | 0.84 | 0.62 |
| ACE_II | 0.67 | 0.66 | 0.45 |
| Acceleration-mix | 0.53 | 0.60 | 0.44 |
| Action_Biker | 0.68 | 0.78 | 0.48 |
| All_Points_Bulletin | 0.74 | 0.74 | 0.55 |
| All_Terrain_Vehicle_Simulator | 0.77 | 0.69 | 0.64 |
| Alternative_World_Games | 0.71 | 0.70 | 0.70 |
| Antics | 0.87 | 0.87 | 0.73 |
| Ark_Pandora | 0.47 | 0.51 | 0.24 |
| Artura | 0.80 | 0.80 | 0.75 |
| Athena | 0.62 | 0.62 | 0.53 |
| Axel_F | 0.41 | 0.40 | 0.12 |
| last_ninja | n/a | 0.58 | 0.58 |

## What this is

A MOS 6581 SID chip emulator in Rust, cycle-accurate at the Φ2 rate, plus a
PSID harness that runs a tune's 6510 code and renders it through the chip to a
WAV file.

The library is the chip only - no CPU, no memory map, no player. The CPU is the
`mos6510` crate from crates.io, a dependency of `sid-play`.

```
crates/mos6581/src/          the chip: wave, envelope, voice, filter, extfilt,
                             resample, sid, tables (generated)
crates/mos6581/tests/        behavioural tests per component
crates/sid-play/src/         sink (WAV) + main.rs: PSID loader, 6510 harness,
                             trace diagnostics (binary `render`)
crates/sid-play/tests/       fixture renders (tests/fixtures/*.sid)
mos6510 (crates.io)          the CPU core, github.com/aovestdipaperino/mos6510
docs/superpowers/specs/      the design spec
docs/superpowers/plans/      plan 1 (faithful chip)
.superpowers/sdd/            task briefs, reports, review packages, ledger
```

## Build and run

`mos6510` comes from crates.io; no sibling checkout is needed.

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release -p sid-play --bin render -- tune.sid --seconds 20 --out out.wav
cargo run --release -p sid-play --bin render -- tune.sid --seconds 5 --out /tmp/x.wav --trace
```

`--trace` is inert unless passed. It prints one `TRACE heard=...` summary line
plus per-site detail: ROM-area executions, ROM reads, vector rewrites, a
per-register SID write histogram, an out-of-range write count, and every address
read after init returned (marked in-block or not). Site stats are recorded at
instruction boundaries.

## What is verified working

The chip itself. Two fixtures in `crates/sid-play/tests/fixtures/` exercise it end to end:

- `scale.sid` - clock-driven; steps a frequency table through a scale.
- `irq.sid` - installs its own `$FFFE` handler, programs CIA 1 timer A at latch
  `$4C00`, and refuses to touch the chip until the handler has run. Sustains
  **103 interrupts in 2 s** against the expected ~50 Hz, with audio to the last
  sample.

Together these cover oscillator, combination-waveform tables, noise clocking,
hard sync, ring modulation, envelope, the voice DC model, filter routing and
resonance, mixer DC, the output-stage high-pass, the sinc resampler, the sink,
the CIA timer, which drives the CPU's IRQ pin (the old hand-rolled interrupt
entry no longer exists).

*Historical (before the CPU fixes; all 16 corpus PSIDs now play, see the update
above):* three real tunes render sustained tonal audio: `Action_Biker`, `5_Title_Tunes`,
`05-09-87` (autocorrelation peaks 0.74–0.99, so tonal rather than noise).

## CPU defects in lib6502 0.2.2

Historical: rust-sid no longer uses lib6502 (see the `mos6510` crate).

Found with a harness that ran Klaus Dormann's `6502_functional_test.bin` on
lib6502 (the harness has since been removed):

The unpatched CPU trapped at `$0674`; with the fixups, at `$230C`. The fixups
(`cpu_fixups.rs`), the `PATCH` switch, `NO_FIXUPS` and `cpu_conformance` no
longer exist; mos6510 replaced all of it. Defects that were worked around:

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

## What does not work - the honest coverage number

*Historical and superseded by the update above: on mos6510, 16/16 corpus PSIDs
render tonal audio. The numbers below (3 play, 19%, no findable bug) describe
the original lib6502 state and are kept for the record.*

From 24 tunes sampled from HVSC (six composers):

- **8 are RSID**, not PSID - magic `RSID`, meaning "recorded on a real C64 with
  ROMs enabled". The loader refuses them, correctly and by design.
- **16 are PSID; 3 play.** Roughly 19% of the format we claim to support.

The 13 failures are not one findable bug. See the investigation log below.

## Licensing

The repository is split so that the permissive crates carry no GPL material.

- `mos6581` (the chip) and `sid-play` (the PSID player and `render` binary) are
  `MIT OR Apache-2.0`, each with `LICENSE-MIT`, `LICENSE-APACHE` and a README.
- `sid-gpl-data` is `GPL-2.0-only`. It holds only the measured data taken from
  reSID (Dag Lem) and libsidplayfp: the combined waveform tables, the filter
  cutoff curves, the reSID envelope, filter, voice and noise constants and the
  power-on RAM pattern. Every item keeps its attribution comment. Its licence
  text is `crates/sid-gpl-data/LICENSE`; there is no root-level licence file.
- `gpl-data` is an off-by-default feature of `mos6581` and `sid-play`. Without it
  the dependency tree does not contain `sid-gpl-data`
  (`cargo tree -p sid-play -e normal | grep -c sid-gpl-data` prints 0). With
  `--features gpl-data` the build is GPL-2.0, and its renders are bit-identical to
  the pre-split baselines.
- The default build uses clean-room data instead: AND-approximated combined
  waveforms, a linear 30 Hz to 12 kHz datasheet cutoff, an all-zero power-on RAM
  image and the constants in `crates/mos6581/src/clean_chip_data.rs`.
- `docs/provenance.md` records the audit (every item, its origin and how it was
  resolved), the clean-room defaults and the known default-build fidelity gaps.
  Its audit left no unresolved Borrowed row outside `sid-gpl-data`.
- `mos6581` has no runtime dependencies in the default build; `sid-play` depends
  on `mos6510` (crates.io) and `mos6581`.

Clean-room roadmap (from `docs/superpowers/specs/2026-11-12-crate-split-design.md`):
replace the `gpl-data` pieces one at a time with data derived from public
sources or from own measurements, starting with the cutoff closed corner (derive
it from the datasheet formula and the C64's actual filter capacitors, sourced
from a public schematic). No GPL source may be consulted to write replacement
data. When nothing is left to borrow, `sid-gpl-data` and the feature can go.

### Default-build fidelity gap

`crates/sid-play/scripts/fidelity-gap.sh` renders each tune for 10 s with both
builds. Tonality is the median autocorrelation peak (0.00 is silence); rms-diff is
the RMS of the difference between the two renders over full scale. Corpus: the 16
PSIDs in the HVSC sample, Last Ninja and the two test fixtures.

```
tune                                  default gpl-data   rms-diff
05-09-87                                 0.60     0.62     0.0748
4_Soccer_Sims_Soccer_Skills              0.66     0.66     0.0271
5_Title_Tunes                            0.73     0.71     0.0300
64_Dwarfs_on_XTC                         0.84     0.89     0.0783
720_Degrees                              0.87     0.84     0.0521
Acceleration-mix                         0.44     0.60     0.2531
ACE_II                                   0.65     0.66     0.0838
Action_Biker                             0.79     0.78     0.0389
All_Points_Bulletin                      0.77     0.74     0.0397
All_Terrain_Vehicle_Simulator            0.83     0.69     0.0623
Alternative_World_Games                  0.70     0.70     0.0101
Antics                                   0.85     0.87     0.0189
Ark_Pandora                              0.51     0.51     0.0352
Artura                                   0.81     0.80     0.0184
Athena                                   0.62     0.62     0.0129
Axel_F                                   0.34     0.40     0.0402
last_ninja                               0.58     0.58     0.0046
irq                                      0.86     0.85     0.0308
scale                                    0.82     0.71     0.0417
```

No tune is silent in the default build and none is below 0.20. The lowest are
`Axel_F` (0.34, against 0.40 with `gpl-data`) and `Acceleration-mix` (0.44
against 0.60, and the largest rms-diff, 0.2531). Other numbers: the scale fixture
renders at rms 53 by default and 1362 with `gpl-data`, and the combined waveform
tables differ from the measured ones by rms 84 to 141 (of 255). The full list is
in the "Known default-build fidelity gaps" section of `docs/provenance.md`.

## Upstream bug filed

<https://github.com/drewwalton19216801/lib6502/issues/4>

`lib6502`'s `do_interrupt` sets the I flag *before* pushing status, so the
stacked P carries `I = 1`, every `RTI` restores interrupts masked, and only one
interrupt can ever be taken. It also clears I for the handler (hardware leaves
it set) and pushes B set for a hardware IRQ (B=1 distinguishes BRK).

**Consequence (historical): never call `cpu.irq()`.** `crates/sid-play/src/main.rs`
performed the interrupt entry by hand (`irq_entry`) using lib6502's public
`pc`, `sp`, `status` and `cycles` fields. The repo has 0 stars and last saw a
push in October 2024, so no fix was expected.

rust-sid no longer depends on lib6502, so issue #4 is history.

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

- The dominant memory reads in every trace - `$0410/$0411/$0412` at 1.93 M each,
  two thirds of all CPU activity - are **our own halt stub self-looping**
  (`JMP $0410`) while the harness waits between frames. Benign, and present
  identically in working and failing tunes.
- A VIC raster model landed as one blob (`d576a71`) including `$D011/$D012`
  reads, write interception on `$D000-$D01F` and a `$FFFE` driver, and
  **regressed** acceptance from 2 tunes to 1. Reverted in `8eaf07d`. The
  subsequent single-variable test showed zero tunes read `$D012` at all - the
  reads were innocent; the blob was the mistake.

*Historical and wrong: the "decided not to play" conclusion below was refuted;
the cause was the CPU (see the update at the top).*

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
   - a click. Silence is achieved downstream by the output-stage high-pass.
3. **Removed the brief's hard-sync condition**, which tested the grandparent
   oscillator's MSB flag instead of the source's, so sync never locked. Inherited
   verbatim from the plan text.
4. **Accepted a documented limitation rather than building ROM support.**
   RSID and Kernal-dependent tunes are out of scope. Cost: the demo plays a
   minority of tunes. *(Historical: all 16 corpus PSIDs now play.)*
5. **Kept `GPL-2.0-only`** at first, because the vendored reSID data could not
   be relicensed. Superseded: the data now lives in `sid-gpl-data` behind the
   `gpl-data` feature and the other crates are `MIT OR Apache-2.0`.
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

*Revised 2026-11-12 after the mos6510 switch:*

1. **Listen to the renders.** Tonality only shows the tunes play; it does not
   show they are right. Per-tune listening checks, starting with `Axel_F` (0.40)
   and `Ark_Pandora` (0.51, min 0.24).
2. **Compare register writes against a reference player** (libsidplayfp built
   locally) for a few tunes.
3. RSID support needs the ROMs and a fuller machine; still out of scope.
4. Widen the corpus beyond six composers.

*Historical (written before the CPU cause was found; the first list above
supersedes it):*

1. **Stop, and document the limit.** The chip is verified; the demo plays some
   tunes. Write the coverage honestly into the README and ship it as an emulator
   library rather than a player.
2. **If real-tune coverage matters**, the credible path is adopting
   libsidplayfp's architecture wholesale - full C64 memory map with the PLA and
   `$01` banking, plus its `psiddrv` in-machine driver - not continuing to diff
   it piece by piece. This is substantially bigger than the remaining gaps look.
3. **Per-tune investigation** of the 13 silent PSIDs, one tune at a time, using
   `--trace`. Cheap to start, but there is no evidence a shared cause remains.

*Historical, from the same earlier state. The "decided not to play" inference
here was refuted, and the 13 silent tunes now play.* What I did **not** check,
so it isn't hidden: I never compared the *values*
written against what a reference player produces for the same tune - that needs
libsidplayfp built here, which it isn't. The 24-tune sample is six composers,
not a representative HVSC slice. And I never confirmed that the 13 silent tunes
actually play in a real emulator, so "the tune decided not to play" is an
inference from its register writes. It remains possible that one shared subtle
bug produces all 13 and the seven eliminated candidates were the wrong seven.
