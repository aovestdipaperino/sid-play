# mos6510 — a cycle-exact, permissively licensed 6510 core

Date: 2026-11-12. Status: approved design; amended after research (see git history); plan in `docs/superpowers/plans/2026-11-12-mos6510-core.md`.

## Why

`lib6502` 0.2.2, the CPU the rust-sid harness uses, fails Klaus Dormann's 6502
functional test. `(zp),Y` ignores Y, binary `ADC`/`SBC` never set C/N/V,
`SBC` subtracts the carry instead of the borrow, `PHP`/`BRK` push a corrupted
status byte, interrupt entry stacks the wrong flags, and every undocumented
opcode is an empty stub. `examples/render/cpu_fixups.rs` papers over the worst
of it and took the sampled corpus from 3/16 playing PSIDs to 16/16, but the core
still fails the test past `$230C`. Patching it further is whack-a-mole. This
spec replaces it.

## Goals

- A MOS 6510 core that is exact **per Φ2 cycle**: one bus access per tick,
  including dummy reads and writes, with interrupts sampled where the hardware
  samples them.
- All 256 opcodes, including every undocumented one.
- The 6510's on-chip I/O port at `$00`/`$01`.
- Licence **MIT OR Apache-2.0**, zero runtime dependencies, no `unsafe`.
- Replaces `lib6502` in rust-sid's harness and deletes the fixup layer.

## Non-goals

- 65C02 or other CMOS variants, and the NES 2A03.
- The RDY line and VIC-II bus stealing (the harness has no VIC). The sequencing
  must not make RDY impossible to add later, but it is not built now.
- The fade of undriven I/O port bits over time.
- Performance work beyond not being wasteful; correctness first.

## Licensing rules for the implementation

- Behaviour comes from documentation and test data: the MOS datasheets,
  "No More Secrets" (NMOS 6510 Unintended Opcodes), Harte's SingleStepTests
  (MIT) and Klaus Dormann's tests (GPL-3.0, used as external test input only).
- Permissive cores may be consulted to cross-check behaviour: floooh/chips
  `m6502.h` (zlib), MAME's m6502 (BSD-3-Clause), `mos6502` (BSD-3-Clause). No
  code is copied from them.
- GPL sources are **not** consulted: VICE, libsidplayfp, reSID, Frodo.
- Klaus Dormann's binaries are downloaded at test time and never committed.

## Placement

A new crate at `chipsynth64/mos6510/`, a sibling of `rust-sid/`, initialised as
its own git repository (as `rust-sid` is; the parent `chipsynth64` repository
ignores it the same way, so `/mos6510/` is added to the parent's `.gitignore`).
`Cargo.toml` uses author Enzo Lombardi `<enzinol@gmail.com>`, repository
`github.com/aovestdipaperino/mos6510`, edition 2024, licence
`MIT OR Apache-2.0`, with both licence files.

## Public interface

```rust
pub trait Bus {
    /// `&mut` because reads have side effects (a CIA's ICR clears on read).
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, value: u8);
}

pub enum Model {
    /// I/O port at $00/$01. The C64's CPU.
    Mos6510,
    /// No port; $00/$01 are plain bus addresses. Used for the flat-RAM tests.
    Mos6502,
}

pub struct Cpu {
    pub a: u8, pub x: u8, pub y: u8, pub s: u8, pub p: u8, pub pc: u16,
    // private: model, sequencing state, interrupt latches, port registers
}

impl Cpu {
    pub fn new(model: Model) -> Self;
    /// Next ticks run the 7-cycle reset sequence, loading PC from $FFFC.
    pub fn reset(&mut self);
    /// Exactly one Φ2 cycle and exactly one bus access.
    pub fn tick(&mut self, bus: &mut impl Bus);
    /// Level-triggered, like the pin. The host holds it while the source asserts.
    pub fn set_irq(&mut self, asserted: bool);
    /// Edge-triggered on assertion.
    pub fn set_nmi(&mut self, asserted: bool);
    /// The next tick fetches an opcode.
    pub fn at_instruction_boundary(&self) -> bool;
    /// A JAM opcode executed; only `reset` recovers.
    pub fn jammed(&self) -> bool;
    /// The port's pins as driven: output bits from $01, input bits from the pins.
    pub fn port_output(&self) -> u8;
    /// What external circuitry pulls the port pins to (default: all high).
    pub fn set_port_input(&mut self, pins: u8);
    /// Start executing at `pc` on the next tick, abandoning any instruction in
    /// progress (and a pending reset). For hosts that call routines directly.
    pub fn jump(&mut self, pc: u16);
    /// At a boundary: the next tick begins an interrupt sequence rather than
    /// an opcode fetch. Lets a host count interrupts actually taken.
    pub fn interrupt_pending(&self) -> bool;
    /// Load the port's direction and data registers directly, for a host that
    /// starts a program without running the Kernal that would set them.
    pub fn set_port_registers(&mut self, ddr: u8, data: u8);
}
```

The register fields are public so a host can set up a call. Status-register
conventions follow Harte's data: bit 5 of `p` stays set; B in `p` is left alone
by everything except `PLP`/`RTI`, which clear it; `PHP`/`BRK` push `p | $30`;
interrupt entry pushes `(p & !$10) | $20`.

## Behaviour to get right

**Interrupts.** IRQ and NMI are sampled during the penultimate cycle of each
instruction, so an interrupt taken depends on the line state one cycle before
the instruction ends. `CLI`, `SEI` and `PLP` change I after that sample point,
so the instruction following them still runs under the old I. `RTI` changes I
before it. A taken branch without page crossing does not sample on its extra
cycle, delaying an interrupt by one instruction. An NMI that arrives during the
vector fetch of `BRK` or IRQ hijacks it to `$FFFA`. Interrupt entry pushes P
with B clear; `BRK` and `PHP` push it with B set; all push bit 5 set. I is set
on entry.

**Decimal mode.** NMOS behaviour: for `ADC`, Z comes from the binary sum and N
and V from the intermediate result after the low-nibble adjust; for `SBC`, all
flags come from the binary result. Invalid BCD inputs follow the same formulas
(Klaus's decimal test checks all 256×256×2 cases).

**Undocumented opcodes.** Stable: `SLO RLA SRE RRA SAX LAX DCP ISC ANC ALR ARR
SBX`, `SBC #` at `$EB`, and the multi-byte NOPs (with their dummy reads).
Unstable: `ANE` computes `(A | M) & X & imm` and `LXA` `(A | M) & imm`, with
the magic constant `M = $EE` exposed as `pub const UNSTABLE_MAGIC` (verified
against all 20,000 Harte cases for `$8B`/`$AB`). `SHA SHX SHY TAS` store
`reg & (H + 1)` where H is the high byte of the address before indexing; on a
page crossing that value also replaces the address high byte (verified on
`$9C`). `LAS` as documented. JAM halts: after the fetch it reads PC, then
`$FFFF`, `$FFFE`, `$FFFE`, then `$FFFF` forever (Harte's `$02` trace), and
`jammed()` turns true. Decimal `ADC`/`SBC`/`ARR` formulas in the plan were
checked against Harte's `$69`, `$E9` and `$6B` files, 0 mismatches.

**The I/O port (Mos6510 only).** `$00` is the data direction register and `$01`
the data register. Reads of `$00`/`$01` return the registers (input bits come
from `set_port_input`). Writes update the register **and** also go out on the
bus, as on real hardware, so the RAM underneath receives the value. Power-on:
DDR `$00`, data `$00` (all inputs, so the pins read as pulled high).

## Internal structure

- `lib.rs`: `Cpu`, `Bus`, `Model`, the public API, `tick` dispatch, and the
  cycle sequences as `impl Cpu` methods (they need its private state; a
  separate file would not buy a cleaner boundary). One per addressing mode × access kind
  (read, write, read-modify-write) for implied/accumulator, immediate, zp, zp,X,
  zp,Y, abs, abs,X, abs,Y, (zp,X), (zp),Y, plus hand-written sequences for
  relative branches, `PHA PHP PLA PLP`, `JSR RTS RTI`, `JMP abs`, `JMP (ind)`
  with the page-wrap bug, `BRK`/IRQ/NMI/reset, and JAM. Each sequence is a
  small state machine advanced one cycle per tick, issuing exactly one bus
  access per step, including the documented dummy accesses (RMW double write,
  page-cross dummy read, stack dummy reads).
- `ops.rs`: pure operation functions on (registers, operand) → (registers,
  result). No bus access, so each is unit-testable in isolation.
- `decode.rs`: a 256-entry `const` table mapping each opcode to (sequence,
  op), with no other logic.
- `port.rs`: the 6510 port registers and pin logic.

Expected size about 900 lines of source excluding tests.

## Testing

1. **Harte SingleStepTests** (`tests/harte.rs`), Model `Mos6502`. For every
   opcode file, every one of the 10,000 cases: load the initial state, tick
   until the instruction is done, compare each cycle's (address, value,
   read/write) to the trace, then the final registers and RAM. The data
   (~1.3 GB) is fetched by `scripts/fetch-harte.sh` into a cache directory
   given by `HARTE_DIR`; with the variable unset or the directory missing, the
   test prints that it is skipped and passes. `serde_json` is a dev-dependency.
   Both fetch scripts default to `target/test-data/` inside the crate, which
   git already ignores.
   The test reports per-opcode failure counts and the first failing case.
2. **Klaus Dormann** (`tests/klaus.rs`): the functional test, binary fetched by
   `scripts/fetch-klaus.sh` into `KLAUS_DIR`, skipped when absent, same as
   above. Pass means reaching the success trap `$3469`. The decimal test has
   no prebuilt binary upstream (it would need the author's assembler), so
   decimal mode is covered by Harte instead: about 5,000 of the 10,000 cases
   for each of `ADC SBC ISC RRA ARR` run with D set, with arbitrary operands.
   This replaces `rust-sid/examples/cpu_conformance.rs`.
3. **Unit tests** for what neither suite covers: the interrupt timing rules
   above (IRQ around `CLI`/`SEI`/`PLP`, the taken-branch delay, NMI hijacking
   `BRK`, level-triggered IRQ re-entering after `RTI` while still asserted, NMI
   not re-firing while held), the 6510 port (register read-back, input pins,
   write reaching RAM, `Mos6502` treating `$00/$01` as RAM), `jammed()` and
   recovery by `reset`, and the reset sequence's vector fetch.

## Integration into rust-sid

- `rust-sid/Cargo.toml`: replace dev-dependency `lib6502` with
  `mos6510 = { path = "../mos6510" }`.
- `examples/render.rs`: `Cpu::new(Model::Mos6510)` with the port set to the
  Kernal's `$2F`/`$37` and input pins `$10` (cassette sense high, bits 6-7
  faded low), so `$01` reads `$37` as on a real C64. The harness's `irq_entry`
  goes away and the CIA line feeds `cpu.set_irq(...)` each tick; the "IRQs
  taken" count uses `interrupt_pending()` at boundaries, so it still counts
  interrupts the CPU actually took (the irq fixture test depends on that).
  `BusHandle` implements the new `Bus`; `Machine` keeps its `Rc<RefCell<..>>`
  sharing, which the harness still needs for the chip and the trace. Frame-clock
  entry into the play stub uses `cpu.jump(...)` at an instruction boundary.
  `cpu.cycles == 0` checks become `cpu.at_instruction_boundary()`.
- The trace's stubbed-opcode counter is removed (nothing is stubbed any more).
- Delete `examples/render/cpu_fixups.rs`, `examples/cpu_conformance.rs` and the
  `NO_FIXUPS` switch.
- Update `HANDSOFF.md`: the upstream lib6502 bug note becomes history.
- Note on licences: the harness keeps its GPL-2.0 licence; it now depends on a
  permissive crate, which is compatible.

## Acceptance

- Harte: 256/256 opcode files pass with per-cycle comparison.
- Klaus functional and decimal tests reach their success traps.
- The new crate's unit tests pass; `cargo clippy -- -D warnings` and
  `cargo fmt --check` are clean in both crates.
- rust-sid's 91 tests stay green.
- The 16 sampled PSIDs and `last_ninja.sid` render tonal audio, measured with
  the same autocorrelation check as before (medians recorded in the handoff).
  `Axel_F` (0.41) and `Ark_Pandora` (0.47) are re-measured and the change
  reported either way.
