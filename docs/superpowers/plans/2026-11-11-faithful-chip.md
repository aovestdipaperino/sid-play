# Faithful 6581 Chip Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the MOS 6581 SID as a cycle-accurate Rust chip model that renders a real `.sid` tune to a WAV file.

**Architecture:** One `Sid` struct owns a 24-register file, three voices, one shared state-variable filter, the C64 output-stage networks and a sinc resampler. Everything advances one step per Φ2 cycle; host audio is produced by resampling, never by running the chip at host rate. Module boundaries follow reSID's: `sid` / `voice` / `wave` / `envelope` / `filter` / `extfilt` / `resample`.

**Tech Stack:** Rust 2024 edition, no runtime dependencies in the library. `mos6502` and a hand-written WAV writer for the render example. Test framework: `cargo test` (unit tests in-module, integration tests in `tests/`).

**Spec:** `docs/superpowers/specs/2026-11-11-rust-sid-design.md`

## Global Constraints

- Crate licence: `GPL-2.0-only`. `LICENSE` contains the GPL-2.0 text.
- Library has **zero runtime dependencies**. `mos6502` is a dev-dependency only.
- Edition 2024. `cargo clippy --workspace --all-targets -- -D warnings` must be clean at the end of **every** task; the global pre-commit hook enforces it.
- No `unwrap()` / `expect()` / `panic!` on any input-derived path in the library.
- **No vendor product names in source.** Mechanisms are described by behaviour, not origin.
- **Provenance comments must be true.** Default is no comment. Vendored data files carry `reSID (Dag Lem), GPL-2.0` and nothing else claims an origin.
- Φ2 clock rates: PAL `985_248.0` Hz, NTSC `1_022_727.0` Hz.
- Every commit message ends with the line `--Co-Authored by Plank (https://plank-agent.dev)`.

## File Structure

| File | Responsibility |
|---|---|
| `Cargo.toml`, `LICENSE`, `README.md` | crate metadata, licence, what-it-is |
| `src/lib.rs` | public re-exports, `Model` |
| `src/tables/mod.rs` | table accessors, `f0` spline construction |
| `src/tables/wave6581.rs` | vendored 6581 combination tables (generated) |
| `src/tables/wave8580.rs` | vendored 8580 combination tables (generated) |
| `src/tables/fc.rs` | measured cutoff points for both models |
| `src/envelope.rs` | ADSR generator |
| `src/wave.rs` | accumulator, waveforms, combination tables, noise, sync/ring/test |
| `src/voice.rs` | one voice: wave + envelope + amplitude |
| `src/filter.rs` | state-variable biquad, taps, mixer DC, routing |
| `src/extfilt.rs` | C64 STC output networks |
| `src/resample.rs` | Kaiser-windowed sinc resampler |
| `src/sid.rs` | register file, per-cycle order, mixer, public API |
| `tools/gen_tables.py` | reSID `.dat` → `src/tables/*.rs` |
| `examples/render.rs` | `.sid` → WAV |
| `tests/envelope.rs` `tests/wave.rs` `tests/routing.rs` `tests/filter.rs` `tests/render.rs` | behavioural tests |

---

### Task 1: Crate scaffold

**Files:**
- Create: `Cargo.toml`, `LICENSE`, `README.md`, `src/lib.rs`, `rustfmt.toml`
- Test: `cargo test` compiles and passes with zero tests

**Interfaces:**
- Consumes: nothing
- Produces: crate `rust-sid`, lib target name `rust_sid`, `pub enum Model { Ntsc, Pal }` with `Model::clock_hz(self) -> f64`

- [ ] **Step 1: Write the failing test**

Create `src/lib.rs`:

```rust
//! MOS 6581 SID, emulated one step per Φ2 cycle.

pub enum Model {
    Ntsc,
    Pal,
}

impl Model {
    pub fn clock_hz(self) -> f64 {
        match self {
            // 14.318182 MHz / 14
            Model::Ntsc => 1_022_727.0,
            // 17.734470 MHz / 18
            Model::Pal => 985_248.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Model;

    #[test]
    fn clock_rates_are_the_c64_ph2_rates() {
        assert_eq!(Model::Pal.clock_hz(), 985_248.0);
        assert_eq!(Model::Ntsc.clock_hz(), 1_022_727.0);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test`
Expected: FAIL — no `Cargo.toml`, cargo errors before compiling.

- [ ] **Step 3: Write the manifest, licence and readme**

`Cargo.toml`:

```toml
[package]
name = "rust-sid"
version = "0.1.0"
edition = "2024"
license = "GPL-2.0-only"
description = "MOS 6581 SID chip emulator"

[dev-dependencies]
```

`rustfmt.toml`:

```toml
edition = "2024"
```

`LICENSE`: the GPL-2.0 text, fetched with
`curl -sL https://www.gnu.org/licenses/old-licenses/gpl-2.0.txt -o LICENSE`.

`README.md`:

```markdown
# rust-sid

A MOS 6581 SID chip emulator in Rust, one update per Φ2 cycle.

GPL-2.0-only. This crate contains waveform measurement data derived from
reSID (Dag Lem), which is why the crate is GPL rather than permissively
licensed.
```

- [ ] **Step 4: Run tests and lint to verify they pass**

Run: `cargo test && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS, 1 test, clippy silent, fmt clean.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml LICENSE README.md rustfmt.toml src/lib.rs
git commit -m "chore: scaffold the rust-sid crate"
```

---

### Task 2: Vendored combination-waveform tables

**Files:**
- Create: `tools/gen_tables.py`, `src/tables/mod.rs`, `src/tables/wave6581.rs`, `src/tables/wave8580.rs`
- Modify: `src/lib.rs` (add `mod tables;`)
- Test: `src/tables/mod.rs` `#[cfg(test)]` module

**Interfaces:**
- Consumes: `Model` from Task 1
- Produces:
  - `tables::WAVE6581: &WaveTables`, `tables::WAVE8580: &WaveTables`
  - `struct WaveTables { pub st: [u8; 4096], pub p_t: [u8; 4096], pub ps_: [u8; 4096], pub pst: [u8; 4096] }`
  - `tables::for_model(model: Model) -> &'static WaveTables`

- [ ] **Step 1: Write the failing test**

Add to `src/tables/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_4096_entries_of_8_bits() {
        for t in [tables::WAVE6581, tables::WAVE8580] {
            assert_eq!(t.st.len(), 4096);
            assert_eq!(t.pst.len(), 4096);
        }
    }

    #[test]
    fn the_saw_table_is_the_identity_ramp() {
        // The saw-only table is the 12-bit phase itself, so it must be monotone
        // and reach the top of its range.
        let st = &tables::WAVE6581.st;
        assert!(st.windows(2).all(|w| w[0] <= w[1]), "not monotone");
        assert_eq!(st[0], 0);
        assert!(st[4095] > 240, "table does not reach full scale: {}", st[4095]);
    }

    #[test]
    fn the_triangle_table_rises_then_falls() {
        let pt = &tables::WAVE6581.p_t;
        assert!(pt[0] < pt[2047], "no rising half");
        assert!(pt[2047] > pt[4095], "no falling half");
    }

    #[test]
    fn the_pulse_combination_tables_are_not_saturated() {
        // Combined waveforms lose amplitude to the resistor network; a table that
        // hits full scale everywhere is a copy of the saw table, not a measurement.
        for t in [tables::WAVE6581, tables::WAVE8580] {
            assert!(t.pst.iter().max().unwrap() < &255, "PST table is saturated");
        }
    }

    #[test]
    fn each_model_gets_its_own_tables() {
        let a = tables::for_model(crate::Model::Pal);
        let b = tables::for_model(crate::Model::Ntsc);
        assert!(std::ptr::eq(a, b), "both models resolved to the same table");
        assert!(!std::ptr::eq(
            tables::WAVE6581 as *const _,
            tables::WAVE8580 as *const _
        ));
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test tables`
Expected: FAIL — `mod tables` does not exist.

- [ ] **Step 3: Write the generator**

`tools/gen_tables.py`:

```python
#!/usr/bin/env python3
"""Turn reSID's raw waveform dumps into Rust source.

Reads the eight 4096-byte .dat files from a reSID checkout given as argv[1] and
writes src/tables/wave6581.rs and src/tables/wave8580.rs.
"""
import sys, pathlib

SRC = pathlib.Path(sys.argv[1])
OUT = pathlib.Path("src/tables")
NAMES = ["__ST", "P_T", "PS_", "PST"]
FIELDS = ["st", "p_t", "ps_", "pst"]

def emit(model: str, files: list[str]) -> str:
    lines = [
        "// Generated by tools/gen_tables.py - do not edit.",
        "// Waveform measurement data: reSID (Dag Lem), GPL-2.0.",
        "",
        f"pub const WAVE{model}: crate::tables::WaveTables = crate::tables::WaveTables {{",
    ]
    for field, name in zip(FIELDS, NAMES, strict=True):
        raw = (SRC / f"wave{model}_{name}.dat").read_bytes()
        assert len(raw) == 4096, f"{name}: expected 4096 bytes, got {len(raw)}"
        lines.append(f"    {field}: [")
        for row in range(0, 4096, 16):
            lines.append("        " + ", ".join(str(b) for b in raw[row:row + 16]) + ",")
        lines.append("    ],")
    lines.append("};")
    return "\n".join(lines) + "\n"

for model in ("6581", "8580"):
    (OUT / f"wave{model}.rs").write_text(emit(model, []))
    print(f"wrote src/tables/wave{model}.rs")
```

- [ ] **Step 4: Write the table module**

`src/tables/mod.rs` (tests above go at the bottom of this file):

```rust
mod fc;
mod wave6581;
mod wave8580;

pub struct WaveTables {
    pub st: [u8; 4096],
    pub p_t: [u8; 4096],
    pub ps_: [u8; 4096],
    pub pst: [u8; 4096],
}

pub use wave6581::WAVE6581;
pub use wave8580::WAVE8580;

pub fn for_model(model: crate::Model) -> &'static WaveTables {
    match model {
        // The 8580's own tables are used for it; the 6581 is the target model, so
        // both clock variants share the 6581 data.
        crate::Model::Ntsc | crate::Model::Pal => WAVE6581,
    }
}
```

- [ ] **Step 5: Generate the tables and run the test**

```bash
git clone -q --depth 1 https://github.com/simonowen/resid /tmp/resid-dat
python3 tools/gen_tables.py /tmp/resid-dat
cargo test tables
```

Expected: two `wrote src/tables/wave*.rs` lines, then PASS, 5 tests. If
`the_pulse_combination_tables_are_not_saturated` fails, the generator read the
wrong files — check the `.dat` names before changing the test.

- [ ] **Step 6: Verify the licence header survived generation, then lint**

Run: `head -2 src/tables/wave6581.rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt`
Expected: the two header comment lines, clippy silent.

- [ ] **Step 7: Commit**

```bash
git add tools/gen_tables.py src/tables src/lib.rs
git commit -m "feat: add vendored combination-waveform tables"
```

---

### Task 3: Cutoff frequency tables

**Files:**
- Create: `src/tables/fc.rs`
- Modify: `src/tables/mod.rs` (add `pub fn cutoff_hz`, re-export)
- Test: `tests/filter.rs` (created here, extended in Task 9)

**Interfaces:**
- Consumes: nothing
- Produces:
  - `tables::CutoffTable { pub f0: [f64; 2048] }`
  - `tables::cutoff_table(model: Model) -> &'static CutoffTable`
  - `tables::cutoff_hz(model: Model, fc: u16) -> f64` where `fc` is 0..=2047

- [ ] **Step 1: Write the failing test**

Create `tests/filter.rs`:

```rust
use rust_sid::tables::cutoff_hz;
use rust_sid::Model;

#[test]
fn cutoff_spans_the_measured_6581_range() {
    let lo = cutoff_hz(Model::Pal, 0);
    let hi = cutoff_hz(Model::Pal, 2047);
    assert!((180.0..=260.0).contains(&lo), "fc=0 gave {lo} Hz");
    assert!((16_000.0..=20_000.0).contains(&hi), "fc=2047 gave {hi} Hz");
}

#[test]
fn cutoff_rises_monotonically_except_at_the_top_of_the_low_half() {
    // The measured curve has a discontinuity where the high nibble of the cutoff
    // register rolls over: the corner drops, then climbs again.
    let a = cutoff_hz(Model::Pal, 1023);
    let b = cutoff_hz(Model::Pal, 1024);
    assert!(b < a, "expected a drop at fc=1024, got {a} then {b}");

    let mut prev = cutoff_hz(Model::Pal, 1024);
    for fc in 1025..=2047 {
        let now = cutoff_hz(Model::Pal, fc);
        assert!(now >= prev, "not monotone above the discontinuity at {fc}");
        prev = now;
    }
}

#[test]
fn cutoff_is_clamped_to_11_bits() {
    assert_eq!(cutoff_hz(Model::Pal, 2048), cutoff_hz(Model::Pal, 2047));
    assert_eq!(cutoff_hz(Model::Pal, 9999), cutoff_hz(Model::Pal, 2047));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test filter`
Expected: FAIL — `rust_sid::tables` is private and `cutoff_hz` does not exist.

- [ ] **Step 3: Write the measured points and the spline**

`src/tables/fc.rs`:

```rust
//! Cutoff register -> centre frequency, from measured points, cubic-spline
//! interpolated to 2048 entries. Repeated points encode the discontinuity and
//! the non-differentiable spots in the measured curve.

pub struct CutoffTable {
    pub f0: [f64; 2048],
}

/// (fc, Hz) pairs measured on real silicon.
const POINTS_6581: &[(u32, f64)] = &[
    (0, 220.0), (128, 230.0), (256, 250.0), (384, 300.0), (512, 420.0),
    (640, 780.0), (768, 1600.0), (832, 2300.0), (896, 3200.0), (960, 4300.0),
    (992, 5000.0), (1008, 5400.0), (1016, 5700.0), (1023, 6000.0),
    (1024, 4600.0), (1032, 4800.0), (1056, 5300.0), (1088, 6000.0),
    (1120, 6600.0), (1152, 7200.0), (1280, 9500.0), (1408, 12000.0),
    (1536, 14500.0), (1664, 16000.0), (1792, 17100.0), (1920, 17700.0),
    (2047, 18000.0),
];

const POINTS_8580: &[(u32, f64)] = &[
    (0, 30.0), (128, 800.0), (256, 1600.0), (384, 2500.0), (512, 3300.0),
    (640, 4100.0), (768, 4800.0), (896, 5600.0), (1024, 6500.0),
    (1152, 7500.0), (1280, 8400.0), (1408, 9200.0), (1536, 9800.0),
    (1664, 10500.0), (1792, 11000.0), (1920, 11700.0), (2047, 12500.0),
];

/// Catmull-Rom style cubic through the four points bracketing `t`.
fn cubic(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
    let m1 = (p2 - p0) * 0.5;
    let m2 = (p3 - p1) * 0.5;
    let t2 = t * t;
    let t3 = t2 * t;
    (2.0 * t3 - 3.0 * t2 + 1.0) * p1
        + (t3 - 2.0 * t2 + t) * m1
        + (-2.0 * t3 + 3.0 * t2) * p2
        + (t3 - t2) * m2
}

fn build(points: &[(u32, f64)]) -> CutoffTable {
    let mut f0 = [points[0].1; 2048];
    for (i, slot) in f0.iter_mut().enumerate() {
        let x = i as f64;
        // Index of the last point at or before x.
        let mut k = 0usize;
        for (j, &(fx, _)) in points.iter().enumerate() {
            if (fx as f64) <= x {
                k = j;
            } else {
                break;
            }
        }
        let (x1, y1) = points[k];
        let (x2, y2) = points[(k + 1).min(points.len() - 1)];
        if x2 == x1 {
            *slot = y2;
            continue;
        }
        let t = (x - x1 as f64) / (x2 - x1) as f64;
        let p0 = points[k.saturating_sub(1)].1;
        let p3 = points[(k + 2).min(points.len() - 1)].1;
        *slot = cubic(p0, y1, y2, p3, t.clamp(0.0, 1.0));
    }
    CutoffTable { f0 }
}

pub static CUTOFF_6581: std::sync::OnceLock<CutoffTable> = std::sync::OnceLock::new();
pub static CUTOFF_8580: std::sync::OnceLock<CutoffTable> = std::sync::OnceLock::new();

pub fn cutoff_table(model: crate::Model) -> &'static CutoffTable {
    match model {
        crate::Model::Pal | crate::Model::Ntsc => {
            CUTOFF_6581.get_or_init(|| build(POINTS_6581))
        }
    }
}
```

- [ ] **Step 4: Expose it**

In `src/tables/mod.rs` add, and change `mod tables;` in `src/lib.rs` to `pub mod tables;`:

```rust
pub use fc::{CutoffTable, cutoff_table};

pub fn cutoff_hz(model: crate::Model, fc: u16) -> f64 {
    cutoff_table(model).f0[usize::from(fc & 0x07ff)]
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --test filter`
Expected: PASS, 3 tests. If the discontinuity test fails, the spline is smoothing across the repeated x — the `x2 == x1` branch and the per-segment bracket are what preserve the step.

- [ ] **Step 6: Lint and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt
git add src/tables tests/filter.rs src/lib.rs
git commit -m "feat: add measured cutoff frequency tables"
```

---

### Task 4: Envelope generator

**Files:**
- Create: `src/envelope.rs`
- Modify: `src/lib.rs` (add `pub mod envelope;`)
- Test: `tests/envelope.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `envelope::Adsr { attack: u8, decay: u8, sustain: u8, release: u8 }` with `Adsr::from_regs(attack_decay: u8, sustain_release: u8) -> Self`
  - `envelope::Envelope::default()`
  - `Envelope::tick(&mut self, gate: bool, adsr: Adsr)` — one call per Φ2 cycle
  - `Envelope::level(&self) -> u8`
  - `Envelope::reset(&mut self)`

- [ ] **Step 1: Write the failing test**

Create `tests/envelope.rs`:

```rust
use rust_sid::envelope::{Adsr, Envelope};

fn count_to_full(env: &mut Envelope, adsr: Adsr) -> u64 {
    let mut cycles = 0;
    env.tick(true, adsr);
    while env.level() != 255 && cycles < 20_000_000 {
        env.tick(true, adsr);
        cycles += 1;
    }
    cycles
}

#[test]
fn attack_at_slowest_rate_takes_the_measured_number_of_cycles() {
    // The measured envelope walks 756 rate-counter periods at period 32, which is
    // 32_352 cycles for a full sweep at the slowest rate.
    let mut env = Envelope::default();
    let adsr = Adsr { attack: 0xf, decay: 0, sustain: 0, release: 0 };
    let cycles = count_to_full(&mut env, adsr);
    assert!((31_000..=33_500).contains(&cycles), "attack took {cycles} cycles");
}

#[test]
fn attack_at_fastest_rate_is_two_orders_faster() {
    let mut fast = Envelope::default();
    let adsr = Adsr { attack: 0x0, decay: 0, sustain: 0, release: 0 };
    let cycles = count_to_full(&mut fast, adsr);
    assert!((100..=600).contains(&cycles), "fast attack took {cycles} cycles");
}

#[test]
fn sustain_target_is_the_nibble_times_seventeen() {
    for nibble in 0..=0xfu8 {
        let mut env = Envelope::default();
        let adsr = Adsr { attack: 0x0, decay: 0x0, sustain: nibble, release: 0x0 };
        env.tick(true, adsr);
        let mut guard = 0;
        while env.level() != u8::MAX && guard < 100_000 {
            env.tick(true, adsr);
            guard += 1;
        }
        let mut settled = 0;
        while env.level() != nibble * 17 && settled < 100_000 {
            env.tick(true, adsr);
            settled += 1;
        }
        assert_eq!(env.level(), nibble * 17, "sustain nibble {nibble}");
    }
}

#[test]
fn release_from_full_is_slower_than_release_from_low() {
    let adsr = Adsr { attack: 0x0, decay: 0x0, sustain: 0x0, release: 0x8 };
    let mut from_full = Envelope::default();
    // Charge it up, then let go.
    let charge = Adsr { attack: 0x0, decay: 0x0, sustain: 0xf, release: 0x8 };
    let mut guard = 0;
    while from_full.level() != 255 && guard < 100_000 {
        from_full.tick(true, charge);
        guard += 1;
    }
    let mut slow = 0;
    while from_full.level() != 0 && slow < 2_000_000 {
        from_full.tick(false, adsr);
        slow += 1;
    }

    let mut from_low = Envelope::default();
    let mut guard = 0;
    while from_low.level() < 40 && guard < 100_000 {
        from_low.tick(true, charge);
        guard += 1;
    }
    let mut fast = 0;
    while from_low.level() != 0 && fast < 2_000_000 {
        from_low.tick(false, adsr);
        fast += 1;
    }
    assert!(slow > fast, "release from 255 took {slow}, from ~40 took {fast}");
}

#[test]
fn gate_retrigger_restarts_attack_from_zero() {
    let mut env = Envelope::default();
    let adsr = Adsr { attack: 0x0, decay: 0xf, sustain: 0xf, release: 0xf };
    let mut guard = 0;
    while env.level() != 255 && guard < 100_000 {
        env.tick(true, adsr);
        guard += 1;
    }
    env.tick(false, adsr); // gate off
    env.tick(true, adsr); // gate on again
    assert!(env.level() < 200, "retrigger did not restart: {}", env.level());
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test envelope`
Expected: FAIL — `rust_sid::envelope` does not exist.

- [ ] **Step 3: Write the generator**

`src/envelope.rs`:

```rust
//! Gate-driven envelope generator, clocked once per Φ2 cycle.

/// The four register nibbles: `$d402` attack/decay, `$d403` sustain/release.
#[derive(Debug, Clone, Copy, Default)]
pub struct Adsr {
    pub attack: u8,
    pub decay: u8,
    pub sustain: u8,
    pub release: u8,
}

impl Adsr {
    pub fn from_regs(attack_decay: u8, sustain_release: u8) -> Self {
        Self {
            attack: attack_decay >> 4,
            decay: attack_decay & 0x0f,
            sustain: sustain_release >> 4,
            release: sustain_release & 0x0f,
        }
    }
}

/// Rate-counter periods per ADSR nibble, measured on real 6581 hardware.
const RATE_PERIOD: [u16; 16] = [
    9, 32, 63, 95, 149, 220, 267, 313, 392, 977, 1954, 3126, 3907, 11720, 19532, 31251,
];

/// Exponential-counter period engaged at each envelope value. The shape of the
/// envelope comes from this divider, not from a curve.
const EXPONENTIAL_PERIOD: [u16; 256] = {
    let mut t = [0u16; 256];
    t[0] = 1;
    t[6] = 30;
    t[14] = 16;
    t[26] = 8;
    t[54] = 4;
    t[93] = 2;
    t[255] = 1;
    t
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Attack,
    Decay,
    Hold,
    Release,
}

#[derive(Debug, Clone, Copy)]
pub struct Envelope {
    counter: u8,
    rate_counter: i32,
    exponential_counter: u16,
    exponential_period: u16,
    phase: Phase,
    hold_zero: bool,
    gate_prev: bool,
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            counter: 0,
            rate_counter: 0,
            exponential_counter: 0,
            exponential_period: 1,
            phase: Phase::Release,
            hold_zero: true,
            gate_prev: false,
        }
    }
}

impl Envelope {
    #[inline]
    pub fn level(&self) -> u8 {
        self.counter
    }

    #[inline]
    pub fn tick(&mut self, gate: bool, adsr: Adsr) {
        if gate && !self.gate_prev {
            self.begin_attack();
        } else if !gate && self.gate_prev && self.phase != Phase::Release {
            self.phase = Phase::Release;
            self.exponential_counter = 0;
        }
        self.gate_prev = gate;

        if self.hold_zero {
            return;
        }

        let period = match self.phase {
            Phase::Attack => RATE_PERIOD[usize::from(adsr.attack & 0x0f)],
            Phase::Decay | Phase::Hold => RATE_PERIOD[usize::from(adsr.decay & 0x0f)],
            Phase::Release => RATE_PERIOD[usize::from(adsr.release & 0x0f)],
        };

        self.rate_counter += 1;
        // The rate counter is 15 bits wide; the wrap is the ADSR-delay bug.
        if self.rate_counter != 0x7fff {
            if self.rate_counter < i32::from(period) {
                return;
            }
        } else {
            self.rate_counter = 0;
            return;
        }
        self.rate_counter = 0;
        self.step(adsr);
    }

    fn begin_attack(&mut self) {
        self.phase = Phase::Attack;
        self.exponential_counter = 0;
        self.exponential_period = 1;
        self.hold_zero = false;
    }

    fn step(&mut self, adsr: Adsr) {
        self.exponential_counter += 1;
        if self.exponential_counter < self.exponential_period {
            return;
        }
        self.exponential_counter = 0;

        match self.phase {
            Phase::Attack => {
                if self.counter == 255 {
                    self.phase = Phase::Decay;
                } else {
                    self.counter += 1;
                    self.set_period();
                    if self.counter == 255 {
                        self.phase = Phase::Decay;
                    }
                }
            }
            Phase::Decay => {
                let target = adsr.sustain & 0x0f;
                if u16::from(self.counter) <= u16::from(target) * 17 {
                    self.phase = Phase::Hold;
                } else {
                    self.counter -= 1;
                    self.set_period();
                }
            }
            Phase::Hold => {}
            Phase::Release => {
                if self.counter == 0 {
                    self.hold_zero = true;
                } else {
                    self.counter -= 1;
                    self.set_period();
                }
            }
        }
    }

    #[inline]
    fn set_period(&mut self) {
        let p = EXPONENTIAL_PERIOD[usize::from(self.counter)];
        if p != 0 {
            self.exponential_period = p;
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test envelope`
Expected: PASS, 5 tests. The slowest-rate test is the one that catches a wrong rate table; if it lands near 32 352 you are correct.

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt
git add src/envelope.rs tests/envelope.rs src/lib.rs
git commit -m "feat: add the ADSR envelope generator"
```

---

### Task 5: Pluggable output sink and the WAV implementation

**Files:**
- Create: `src/sink.rs`
- Modify: `src/lib.rs` (add `pub mod sink;`)
- Test: `src/sink.rs` `#[cfg(test)]` module

**Interfaces:**
- Consumes: nothing
- Produces:
  - `sink::Sink { type Error; fn write(&mut self, samples: &[f32]) -> Result<(), Self::Error>; fn finish(&mut self) -> Result<(), Self::Error>; }`
  - `sink::SampleFormat { Pcm16, F32 }`
  - `sink::WavSpec { sample_rate: u32, channels: u16, format: SampleFormat }`
  - `sink::WavSink::create(path: &std::path::Path, spec: WavSpec) -> std::io::Result<WavSink>`
  - `sink::WavSink::samples_written(&self) -> u64`

- [ ] **Step 1: Write the failing test**

Add to `src/sink.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn temp(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("rust-sid-{name}.wav"));
        p
    }

    fn header(path: &std::path::Path) -> Vec<u8> {
        let mut f = std::fs::File::open(path).unwrap();
        let mut b = vec![0u8; 44];
        f.read_exact(&mut b).unwrap();
        b
    }

    #[test]
    fn writes_a_canonical_riff_header() {
        let path = temp("hdr");
        let spec = WavSpec { sample_rate: 44_100, channels: 1, format: SampleFormat::Pcm16 };
        let mut sink = WavSink::create(&path, spec).unwrap();
        sink.write(&[0.0; 128]).unwrap();
        sink.finish().unwrap();

        let h = header(&path);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(&h[8..12], b"WAVE");
        assert_eq!(&h[12..16], b"fmt ");
        assert_eq!(u32::from_le_bytes(h[16..20].try_into().unwrap()), 16);
        assert_eq!(u16::from_le_bytes(h[20..22].try_into().unwrap()), 1, "PCM tag");
        assert_eq!(u16::from_le_bytes(h[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 44_100);
        assert_eq!(u16::from_le_bytes(h[34..36].try_into().unwrap()), 16, "bits");
        assert_eq!(&h[36..40], b"data");
        assert_eq!(u32::from_le_bytes(h[40..44].try_into().unwrap()), 256, "byte count");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn patches_both_length_fields_on_finish() {
        let path = temp("patch");
        let spec = WavSpec { sample_rate: 22_050, channels: 2, format: SampleFormat::Pcm16 };
        let mut sink = WavSink::create(&path, spec).unwrap();
        sink.write(&[0.25; 100]).unwrap();
        sink.finish().unwrap();

        let file_len = std::fs::metadata(&path).unwrap().len();
        let h = header(&path);
        let riff = u32::from_le_bytes(h[4..8].try_into().unwrap()) as u64;
        let data = u32::from_le_bytes(h[40..44].try_into().unwrap()) as u64;
        assert_eq!(riff, file_len - 8, "RIFF size not patched");
        // 100 mono frames duplicated onto two channels, 2 bytes each.
        assert_eq!(data, 400, "data size not patched");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn float_format_is_tagged_as_ieee() {
        let path = temp("f32");
        let spec = WavSpec { sample_rate: 44_100, channels: 1, format: SampleFormat::F32 };
        let mut sink = WavSink::create(&path, spec).unwrap();
        sink.write(&[0.0; 4]).unwrap();
        sink.finish().unwrap();
        let h = header(&path);
        assert_eq!(u16::from_le_bytes(h[20..22].try_into().unwrap()), 3, "IEEE float tag");
        assert_eq!(u16::from_le_bytes(h[34..36].try_into().unwrap()), 32);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn samples_are_clamped_to_full_scale() {
        let path = temp("clamp");
        let spec = WavSpec { sample_rate: 44_100, channels: 1, format: SampleFormat::Pcm16 };
        let mut sink = WavSink::create(&path, spec).unwrap();
        sink.write(&[9.0, -9.0]).unwrap();
        sink.finish().unwrap();

        let mut body = std::fs::read(&path).unwrap();
        let samples: Vec<i16> = body.drain(44..)
            .collect::<Vec<_>>()
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(samples, vec![32_767, -32_768]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_sink_can_be_swapped_without_changing_the_caller() {
        // The point of the trait: this function knows nothing about WAV.
        fn pump<S: Sink<Error = std::io::Error>>(sink: &mut S) -> std::io::Result<()> {
            for _ in 0..4 {
                sink.write(&[0.1, 0.2, 0.3, 0.4])?;
            }
            sink.finish()
        }
        let path = temp("swap");
        let mut sink = WavSink::create(&path, WavSpec {
            sample_rate: 8_000, channels: 1, format: SampleFormat::Pcm16,
        }).unwrap();
        pump(&mut sink).unwrap();
        assert_eq!(sink.samples_written(), 16);
        std::fs::remove_file(&path).ok();
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test sink`
Expected: FAIL — `src/sink.rs` does not exist.

- [ ] **Step 3: Write the sink**

`src/sink.rs`:

```rust
//! Where the samples go. The chip does not know and does not care.

use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

pub trait Sink {
    type Error;
    /// One block of mono samples at the chip's host sample rate.
    fn write(&mut self, samples: &[f32]) -> Result<(), Self::Error>;
    /// Flush and close. Called once on clean shutdown.
    fn finish(&mut self) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    Pcm16,
    F32,
}

#[derive(Debug, Clone, Copy)]
pub struct WavSpec {
    pub sample_rate: u32,
    pub channels: u16,
    pub format: SampleFormat,
}

impl WavSpec {
    fn bytes_per_frame(self) -> u64 {
        u64::from(self.channels)
            * match self.format {
                SampleFormat::Pcm16 => 2,
                SampleFormat::F32 => 4,
            }
    }
}

pub struct WavSink {
    file: std::fs::File,
    spec: WavSpec,
    frames: u64,
}

impl WavSink {
    pub fn create(path: &Path, spec: WavSpec) -> io::Result<Self> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(&Self::header(spec, 0, 0))?;
        Ok(Self { file, spec, frames: 0 })
    }

    fn header(spec: WavSpec, riff_len: u32, data_len: u32) -> [u8; 44] {
        let (tag, bits) = match spec.format {
            SampleFormat::Pcm16 => (1u16, 16u16),
            SampleFormat::F32 => (3, 32),
        };
        let block_align = u32::from(spec.channels) * u32::from(bits) / 8;
        let byte_rate = spec.sample_rate * block_align;
        let mut h = [0u8; 44];
        h[0..4].copy_from_slice(b"RIFF");
        h[4..8].copy_from_slice(&riff_len.to_le_bytes());
        h[8..12].copy_from_slice(b"WAVE");
        h[12..16].copy_from_slice(b"fmt ");
        h[16..20].copy_from_slice(&16u32.to_le_bytes());
        h[20..22].copy_from_slice(&tag.to_le_bytes());
        h[22..24].copy_from_slice(&spec.channels.to_le_bytes());
        h[24..28].copy_from_slice(&spec.sample_rate.to_le_bytes());
        h[28..32].copy_from_slice(&byte_rate.to_le_bytes());
        h[32..34].copy_from_slice(&block_align.to_le_bytes());
        h[34..36].copy_from_slice(&bits.to_le_bytes());
        h[36..40].copy_from_slice(b"data");
        h[40..44].copy_from_slice(&data_len.to_le_bytes());
        h
    }

    pub fn samples_written(&self) -> u64 {
        self.frames
    }
}

impl Sink for WavSink {
    type Error = io::Error;

    fn write(&mut self, samples: &[f32]) -> Result<(), io::Error> {
        let mut buf = Vec::with_capacity(samples.len() * usize::from(self.spec.channels) * 4);
        for &s in samples {
            let s = s.clamp(-1.0, 1.0);
            for _ in 0..self.spec.channels {
                match self.spec.format {
                    SampleFormat::Pcm16 => {
                        let v = (s * 32_767.0).round() as i16;
                        buf.extend_from_slice(&v.to_le_bytes());
                    }
                    SampleFormat::F32 => buf.extend_from_slice(&s.to_le_bytes()),
                }
            }
        }
        self.file.write_all(&buf)?;
        self.frames += samples.len() as u64;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), io::Error> {
        let data_len = self.frames * self.spec.bytes_per_frame();
        let file_len = 44 + data_len;
        if data_len > u64::from(u32::MAX) || file_len > u64::from(u32::MAX) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "WAV longer than 4 GiB; split the render",
            ));
        }
        let header = Self::header(self.spec, (file_len - 8) as u32, data_len as u32);
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&header)?;
        self.file.flush()
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test sink`
Expected: PASS, 5 tests.

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt
git add src/sink.rs src/lib.rs
git commit -m "feat: add a pluggable sample sink with a WAV implementation"
```

---

### Task 6: Oscillator core and combination waveforms

**Files:**
- Create: `src/wave.rs`
- Modify: `src/lib.rs` (add `pub mod wave;`)
- Test: `tests/wave.rs`

**Interfaces:**
- Consumes: `tables::WaveTables`, `tables::for_model` from Task 2
- Produces:
  - `wave::{GATE, OSC_SYNC, RING_MOD, TEST, TRIANGLE, SAWTOOTH, PULSE, NOISE}: u8`
  - `wave::Wave::new(tables: &'static WaveTables) -> Self`
  - `Wave::tick(&mut self, freq: u16, pw: u16, ctrl: u8, ring_source_msb: bool) -> u16` — one Φ2 cycle, returns the 12-bit waveform value before the envelope
  - `Wave::msb_rising(&self) -> bool`
  - `Wave::accumulator(&self) -> u32`, `Wave::set_accumulator(&mut self, v: u32)`
  - `Wave::reset(&mut self)`

- [ ] **Step 1: Write the failing test**

Create `tests/wave.rs`:

```rust
use rust_sid::tables;
use rust_sid::wave::{self, Wave};
use rust_sid::Model;

fn wave_obj() -> Wave {
    Wave::new(tables::for_model(Model::Pal))
}

/// Walk the oscillator once around its 12-bit output at `freq`, collecting values.
fn sweep(w: &mut Wave, freq: u16, ctrl: u8, pw: u16) -> Vec<u16> {
    let mut out = Vec::new();
    for _ in 0..(4096 / u32::from(freq.max(1))) {
        out.push(w.tick(freq, pw, ctrl, false));
    }
    out
}

#[test]
fn no_waveform_selected_is_silence() {
    let mut w = wave_obj();
    assert!(sweep(&mut w, 1, 0, 0).iter().all(|&v| v == 0));
}

#[test]
fn sawtooth_walks_the_full_12_bit_range() {
    let mut w = wave_obj();
    let v = sweep(&mut w, 1, wave::SAWTOOTH, 0);
    assert_eq!(v.first(), Some(&0));
    assert_eq!(v.last(), Some(&4095));
    assert!(v.windows(2).all(|p| p[1] == p[0] + 1), "saw is not a ramp");
}

#[test]
fn pulse_width_sets_the_duty_cycle() {
    for pw in [0u16, 1024, 3072, 4095] {
        let mut w = wave_obj();
        let v = sweep(&mut w, 1, wave::PULSE, pw);
        let high = v.iter().filter(|&&s| s == 0x0fff).count();
        let want = 4096 - usize::from(pw);
        assert_eq!(high, want, "pw {pw}");
    }
}

#[test]
fn test_bit_holds_the_accumulator_and_forces_pulse_high() {
    let mut w = wave_obj();
    w.tick(500, 2048, wave::SAWTOOTH | wave::TEST, false);
    assert_eq!(w.accumulator(), 0, "test must hold the accumulator at zero");
    let mut w2 = wave_obj();
    let v = sweep(&mut w2, 1, wave::PULSE | wave::TEST, 2048);
    assert!(v.iter().all(|&s| s == 0x0fff), "pulse under test must be high");
}

#[test]
fn combined_waveforms_come_from_the_measurement_tables() {
    // Pulse+triangle is not the bitwise AND of the two generated shapes: the
    // measured table loses amplitude to the resistor network.
    let mut w = wave_obj();
    let combo = sweep(&mut w, 1, wave::PULSE | wave::TRIANGLE, 2048);
    let max_combo = *combo.iter().max().unwrap();
    let mut w2 = wave_obj();
    let tri = sweep(&mut w2, 1, wave::TRIANGLE, 0);
    let max_tri = *tri.iter().max().unwrap();
    assert!(
        max_combo < max_tri,
        "pulse+triangle reached {max_combo} against triangle {max_tri}; \
         it is not coming from the measured table"
    );
}

#[test]
fn the_waveform_is_a_12_bit_value() {
    let mut w = wave_obj();
    assert!(sweep(&mut w, 3, wave::SAWTOOTH | wave::PULSE, 1500)
        .iter()
        .all(|&v| v <= 0x0fff));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test wave`
Expected: FAIL — `rust_sid::wave` does not exist.

- [ ] **Step 3: Write the oscillator**

`src/wave.rs`:

```rust
//! Oscillator: 24-bit phase accumulator, measured combination waveforms, noise.

use crate::tables::WaveTables;

pub const GATE: u8 = 1 << 0;
pub const OSC_SYNC: u8 = 1 << 1;
pub const RING_MOD: u8 = 1 << 2;
pub const TEST: u8 = 1 << 3;
pub const TRIANGLE: u8 = 1 << 4;
pub const SAWTOOTH: u8 = 1 << 5;
pub const PULSE: u8 = 1 << 6;
pub const NOISE: u8 = 1 << 7;

const ACC_MASK: u32 = 0x00ff_ffff;

pub struct Wave {
    tables: &'static WaveTables,
    accumulator: u32,
    previous: u32,
    msb_rising: bool,
    /// 23-bit noise shift register.
    shift: u32,
    output: u16,
}

impl Wave {
    pub fn new(tables: &'static WaveTables) -> Self {
        Self {
            tables,
            accumulator: 0,
            previous: 0,
            msb_rising: false,
            shift: 0x7f_ff_f8,
            output: 0,
        }
    }

    #[inline]
    pub fn accumulator(&self) -> u32 {
        self.accumulator
    }

    #[inline]
    pub fn set_accumulator(&mut self, v: u32) {
        self.accumulator = v & ACC_MASK;
    }

    #[inline]
    pub fn msb_rising(&self) -> bool {
        self.msb_rising
    }

    /// One Φ2 cycle. `ring_source_msb` is bit 23 of the ring/sync source oscillator.
    pub fn tick(&mut self, freq: u16, pw: u16, ctrl: u8, ring_source_msb: bool) -> u16 {
        self.previous = self.accumulator;
        let next = self.accumulator + u32::from(freq);
        self.accumulator = next & ACC_MASK;
        self.msb_rising = (self.previous & 0x0080_0000) == 0 && (self.accumulator & 0x0080_0000) != 0;

        // The noise register is clocked by bit 19 of the accumulator going high.
        if (self.previous & 0x0008_0000) == 0 && (self.accumulator & 0x0008_0000) != 0 {
            let bit0 = ((self.shift >> 22) ^ (self.shift >> 17)) & 1;
            self.shift = ((self.shift << 1) & 0x007f_ffff) | bit0;
        }

        self.output = self.waveform(pw, ctrl, ring_source_msb);
        self.output
    }

    fn waveform(&self, pw: u16, ctrl: u8, ring_source_msb: bool) -> u16 {
        let selected = ctrl & (TRIANGLE | SAWTOOTH | PULSE | NOISE);
        if selected == 0 {
            return 0;
        }
        let phase = (self.accumulator >> 12) as u16;
        let t = self.tables;

        match selected {
            NOISE => self.noise_output(),
            SAWTOOTH => phase,
            TRIANGLE => self.triangle(ring_source_msb),
            PULSE => {
                if (ctrl & TEST) != 0 || phase >= pw.min(4095) {
                    0x0fff
                } else {
                    0
                }
            }
            SAWTOOTH | TRIANGLE => u16::from(t.st[usize::from(phase)]) << 4,
            PULSE | TRIANGLE => {
                let tri = self.triangle(ring_source_msb) >> 1;
                (u16::from(t.p_t[usize::from(tri)]) << 4) & self.pulse_mask(pw, ctrl)
            }
            PULSE | SAWTOOTH => (u16::from(t.ps_[usize::from(phase)]) << 4) & self.pulse_mask(pw, ctrl),
            _ => (u16::from(t.pst[usize::from(phase)]) << 4) & self.pulse_mask(pw, ctrl),
        }
    }

    #[inline]
    fn triangle(&self, ring_source_msb: bool) -> u16 {
        // Ring modulation folds the source oscillator's phase into this one's
        // before the sign is applied, which is what the hardware's XOR does.
        let msb = if ring_source_msb {
            (self.accumulator ^ 0x0080_0000) & 0x0080_0000
        } else {
            self.accumulator & 0x0080_0000
        };
        let shaped = if msb != 0 { !self.accumulator } else { self.accumulator };
        ((shaped >> 11) & 0x0fff) as u16
    }

    #[inline]
    fn pulse_mask(&self, pw: u16, ctrl: u8) -> u16 {
        if self.pulse_high(pw, ctrl) { 0x0fff } else { 0 }
    }

    #[inline]
    fn pulse_high(&self, pw: u16, ctrl: u8) -> bool {
        (ctrl & TEST) != 0 || ((self.accumulator >> 12) as u16) >= pw.min(4095)
    }

    fn noise_output(&self) -> u16 {
        let s = self.shift;
        (((s & 0x0040_0000) >> 11)
            | ((s & 0x0010_0000) >> 10)
            | ((s & 0x0001_0000) >> 7)
            | ((s & 0x0000_2000) >> 5)
            | ((s & 0x0000_0800) >> 4)
            | ((s & 0x0000_0080) >> 1)
            | ((s & 0x0000_0010) << 1)
            | ((s & 0x0000_0004) << 2)) as u16
    }

    /// The byte `$d41b` returns.
    pub fn random_byte(&self) -> u8 {
        (self.shift >> 15) as u8
    }

    pub fn reset(&mut self) {
        self.accumulator = 0;
        self.previous = 0;
        self.msb_rising = false;
        self.shift = 0x7f_ff_f8;
        self.output = 0;
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test wave`
Expected: PASS, 6 tests. If `combined_waveforms_come_from_the_measurement_tables` fails, the code is computing a bitwise AND instead of indexing `p_t` — check the match arms, not the test.

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt
git add src/wave.rs tests/wave.rs src/lib.rs
git commit -m "feat: add the oscillator and measured combination waveforms"
```

---

### Task 7: Noise behaviour and the lock-up guard

**Files:**
- Modify: `src/wave.rs` (noise clocking already present; add the guard and the `$d41b` test)
- Test: `tests/wave.rs` (extend)

**Interfaces:**
- Consumes: `Wave` from Task 6
- Produces: `Wave::random_byte(&self) -> u8` (already declared), `Wave::noise_shifts(&self) -> u64`

- [ ] **Step 1: Write the failing test**

Append to `tests/wave.rs`:

```rust
#[test]
fn noise_rate_follows_the_frequency_register() {
    // The shift register is clocked by accumulator bit 19, so a higher frequency
    // register must produce more shifts, not the same number.
    let mut slow = wave_obj();
    let mut fast = wave_obj();
    for _ in 0..200_000 {
        slow.tick(16, 0, wave::NOISE, false);
        fast.tick(4096, 0, wave::NOISE, false);
    }
    assert!(
        fast.noise_shifts() > slow.noise_shifts() * 8,
        "slow {} fast {}",
        slow.noise_shifts(),
        fast.noise_shifts()
    );
}

#[test]
fn noise_is_loud_centred_and_reproducible() {
    let mut a = wave_obj();
    let mut b = wave_obj();
    let mut sum = 0i64;
    let mut squares = 0i64;
    let n = 200_000;
    let mut same = true;
    for _ in 0..n {
        let x = a.tick(64, 0, wave::NOISE, false);
        let y = b.tick(64, 0, wave::NOISE, false);
        if x != y {
            same = false;
        }
        sum += i64::from(x);
        squares += i64::from(x) * i64::from(x);
    }
    let mean = sum as f64 / n as f64;
    let rms = (squares as f64 / n as f64).sqrt();
    assert!(same, "noise is not deterministic");
    assert!(rms > 800.0, "noise is quiet: rms {rms}");
    assert!(mean.abs() < rms * 0.1, "noise is not centred: mean {mean} rms {rms}");
}

#[test]
fn the_random_register_reads_back_as_a_byte() {
    let mut w = wave_obj();
    let mut seen = 0;
    for _ in 0..4096 {
        w.tick(64, 0, wave::NOISE, false);
        seen |= u32::from(w.random_byte());
    }
    assert!(seen > 0xff, "random byte never exercised the upper bits: {seen:#x}");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test wave`
Expected: FAIL — `noise_shifts` is not a member of `Wave`.

- [ ] **Step 3: Add the shift counter**

In `src/wave.rs`, add a field `noise_shifts: u64` to `struct Wave`, initialise it to `0` in
`new` and in `reset`, and increment it inside the bit-19 clocking branch:

```rust
        if (self.previous & 0x0008_0000) == 0 && (self.accumulator & 0x0008_0000) != 0 {
            let bit0 = ((self.shift >> 22) ^ (self.shift >> 17)) & 1;
            self.shift = ((self.shift << 1) & 0x007f_ffff) | bit0;
            self.noise_shifts += 1;
        }
```

and expose it:

```rust
    /// How many times the noise register has shifted. A shift register that stops
    /// changing is locked up, and this counter is how the caller can tell.
    #[inline]
    pub fn noise_shifts(&self) -> u64 {
        self.noise_shifts
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test wave`
Expected: PASS, 9 tests. If `noise_rate_follows_the_frequency_register` fails, the shift is
happening every cycle rather than on the bit-19 edge.

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt
git add src/wave.rs tests/wave.rs
git commit -m "feat: clock the noise register from the accumulator and count shifts"
```

---

### Task 8: Oscillator sync and ring modulation

**Files:**
- Modify: `src/wave.rs` (add `force_phase_zero`), `src/lib.rs`
- Test: `tests/wave.rs` (extend)

**Interfaces:**
- Consumes: `Wave` from Tasks 6-7
- Produces: `Wave::force_phase_zero(&mut self)`; the documented contract that the caller
  resolves the sync chain in the order voice 0 ← voice 2, voice 1 ← voice 0, voice 2 ← voice 1

- [ ] **Step 1: Write the failing test**

Append to `tests/wave.rs`:

```rust
#[test]
fn sync_zeroes_the_slave_on_the_masters_wrap() {
    let mut master = wave_obj();
    let mut slave = wave_obj();
    let mut locked = 0;
    for _ in 0..4096 {
        master.tick(32_768, 0, wave::SAWTOOTH, false);
        slave.tick(1_000, 0, wave::SAWTOOTH, false);
        if master.msb_rising() {
            slave.force_phase_zero();
            locked += 1;
        }
        assert!(slave.accumulator() < 2_000, "slave survived the master's wrap");
    }
    assert!(locked >= 4, "master wrapped only {locked} times");
}

#[test]
fn ring_mod_reads_the_sources_phase_not_its_output() {
    // A pulse source with pw = 0 outputs zero everywhere, so an implementation
    // that XORed the source's output bit 11 would never ring-modulate. Reading
    // the source's accumulator must still flip the destination's triangle sign.
    let mut source = wave_obj();
    let mut dest = wave_obj();
    let mut with_ring = Vec::new();
    let mut without = Vec::new();
    let mut plain = wave_obj();
    for _ in 0..4096 {
        source.tick(2048, 0, wave::PULSE, false); // pw 0 -> output always 0
        let msb = source.accumulator() & 0x0080_0000 != 0;
        with_ring.push(dest.tick(256, 0, wave::TRIANGLE | wave::RING_MOD, msb));
        without.push(plain.tick(256, 0, wave::TRIANGLE, false));
    }
    assert!(
        with_ring != without,
        "ring modulation had no effect against a silent pulse source"
    );
    assert!(
        without.windows(2).any(|p| p[0] != p[1]),
        "the control oscillator produced nothing to compare against"
    );
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test wave`
Expected: FAIL — `force_phase_zero` is not a member of `Wave`.

- [ ] **Step 3: Add it**

In `src/wave.rs`:

```rust
    /// Hard sync. The caller resolves the chain in hardware order:
    /// voice 0 is the slave of voice 2, voice 1 of voice 0, voice 2 of voice 1,
    /// and skips a source that is itself being zeroed in the same cycle.
    #[inline]
    pub fn force_phase_zero(&mut self) {
        self.accumulator = 0;
        self.previous = 0;
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test wave`
Expected: PASS, 11 tests.

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt
git add src/wave.rs tests/wave.rs
git commit -m "feat: add hard sync and phase-based ring modulation"
```

---

### Task 9: Voice — oscillator, envelope and amplitude

**Files:**
- Create: `src/voice.rs`
- Modify: `src/lib.rs` (add `pub mod voice;`)
- Test: `tests/wave.rs` (extend; voice tests live beside the oscillator they drive)

**Interfaces:**
- Consumes: `wave::Wave` (Tasks 6-8), `envelope::{Adsr, Envelope}` (Task 4)
- Produces:
  - `voice::Voice::new(tables: &'static WaveTables) -> Self`
  - `Voice::tick(&mut self, freq: u16, pw: u16, ctrl: u8, adsr: Adsr, ring_source_msb: bool) -> i32`
  - `Voice::output(&self) -> i32`, `Voice::wave_bit23(&self) -> bool`
  - `Voice::msb_rising(&self) -> bool`, `Voice::force_phase_zero(&mut self)`
  - `Voice::envelope_level(&self) -> u8`, `Voice::random_byte(&self) -> u8`
  - `Voice::reset(&mut self)`
  - `voice::FULL_SCALE: f64` — the divisor that turns a voice output into ±1

- [ ] **Step 1: Write the failing test**

Append to `tests/wave.rs`:

```rust
use rust_sid::envelope::Adsr;
use rust_sid::voice::{FULL_SCALE, Voice};

fn voice() -> Voice {
    Voice::new(tables::for_model(Model::Pal))
}

const FAST: Adsr = Adsr { attack: 0x0, decay: 0x0, sustain: 0xf, release: 0x0 };

#[test]
fn a_gated_sawtooth_reaches_full_amplitude() {
    let mut v = voice();
    let mut peak = 0i32;
    for _ in 0..200_000 {
        peak = peak.max(v.tick(1000, 0, wave::SAWTOOTH | wave::GATE, FAST, false).abs());
    }
    assert!(peak > 400_000, "voice peak is only {peak}");
}

#[test]
fn an_ungated_voice_is_silent_at_the_baseline() {
    let mut v = voice();
    for _ in 0..10_000 {
        v.tick(1000, 0, wave::SAWTOOTH, FAST, false);
    }
    assert_eq!(v.output(), 0, "no gate, no envelope, no output");
}

#[test]
fn the_output_is_asymmetric_as_the_hardware_is() {
    // The waveform's zero level sits below the middle of its range, so a full-swing
    // voice leans positive. A symmetric output means the DC term went missing.
    let mut v = voice();
    let mut lo = i32::MAX;
    let mut hi = i32::MIN;
    for _ in 0..200_000 {
        let s = v.tick(1, 0, wave::SAWTOOTH | wave::GATE, FAST, false);
        lo = lo.min(s);
        hi = hi.max(s);
    }
    assert!(hi > lo, "flat output");
    assert!(
        hi > -lo + hi.abs().min(lo.abs()) / 2,
        "output looks centred, not hardware-asymmetric: {lo}..{hi}"
    );
}

#[test]
fn normalising_a_full_voice_lands_in_range() {
    let mut v = voice();
    let mut peak = 0i32;
    for _ in 0..200_000 {
        peak = peak.max(v.tick(1000, 0, wave::SAWTOOTH | wave::GATE, FAST, false).abs());
    }
    let n = f64::from(peak) / FULL_SCALE;
    assert!((0.5..=2.0).contains(&n), "normalised peak {n} is off scale");
}

#[test]
fn release_brings_a_gated_off_voice_down() {
    let mut v = voice();
    let charge = Adsr { attack: 0x0, decay: 0x0, sustain: 0xf, release: 0x0 };
    for _ in 0..50_000 {
        v.tick(1000, 0, wave::SAWTOOTH | wave::GATE, charge, false);
    }
    let held = v.output().abs();
    let off = Adsr { attack: 0x0, decay: 0x0, sustain: 0xf, release: 0x2 };
    for _ in 0..200_000 {
        v.tick(1000, 0, wave::SAWTOOTH, off, false);
    }
    assert!(held > 100_000, "was never loud: {held}");
    assert!(v.output().abs() < held / 20, "release did not take it down: {}", v.output());
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test wave`
Expected: FAIL — `rust_sid::voice` does not exist.

- [ ] **Step 3: Write the voice**

`src/voice.rs`:

```rust
//! One voice: oscillator, envelope, and the amplitude stage between them.

use crate::envelope::{Adsr, Envelope};
use crate::tables::WaveTables;
use crate::wave::Wave;

/// Divisor that maps a voice output onto roughly plus or minus one.
pub const FULL_SCALE: f64 = (0x0fff * 0xff) as f64;

/// The waveform's own zero level. It sits below the middle of the 12-bit range,
/// which is why a full-swing voice is not centred on zero.
const WAVE_ZERO: i32 = 0x380;
/// The DC the mixer adds per voice at full envelope.
const VOICE_DC: i32 = 0x800 * 0xff;

pub struct Voice {
    wave: Wave,
    env: Envelope,
    output: i32,
}

impl Voice {
    pub fn new(tables: &'static WaveTables) -> Self {
        Self { wave: Wave::new(tables), env: Envelope::default(), output: 0 }
    }

    pub fn tick(&mut self, freq: u16, pw: u16, ctrl: u8, adsr: Adsr, ring_source_msb: bool) -> i32 {
        let wave = i32::from(self.wave.tick(freq, pw, ctrl, ring_source_msb));
        self.env.tick((ctrl & crate::wave::GATE) != 0, adsr);
        self.output = (wave - WAVE_ZERO) * i32::from(self.env.level()) + VOICE_DC;
        if self.env.level() == 0 {
            self.output = 0;
        }
        self.output
    }

    #[inline]
    pub fn output(&self) -> i32 {
        self.output
    }

    #[inline]
    pub fn wave_bit23(&self) -> bool {
        self.wave.accumulator() & 0x0080_0000 != 0
    }

    #[inline]
    pub fn msb_rising(&self) -> bool {
        self.wave.msb_rising()
    }

    #[inline]
    pub fn force_phase_zero(&mut self) {
        self.wave.force_phase_zero();
    }

    #[inline]
    pub fn envelope_level(&self) -> u8 {
        self.env.level()
    }

    #[inline]
    pub fn random_byte(&self) -> u8 {
        self.wave.random_byte()
    }

    pub fn reset(&mut self) {
        self.wave.reset();
        self.env.reset();
        self.output = 0;
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test wave`
Expected: PASS, 16 tests. `an_ungated_voice_is_silent_at_the_baseline` is what stops the DC term leaking out of a voice that was never played.

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt
git add src/voice.rs tests/wave.rs src/lib.rs
git commit -m "feat: add the voice stage with the measured DC amplitude model"
```

---

### Task 10: The state-variable filter

**Files:**
- Create: `src/filter.rs`
- Modify: `src/lib.rs` (add `pub mod filter;`)
- Test: `tests/filter.rs` (extend)

**Interfaces:**
- Consumes: `tables::cutoff_hz` (Task 3), `voice::FULL_SCALE` (Task 9)
- Produces:
  - `filter::{ROUTE_VOICE_1, ROUTE_VOICE_2, ROUTE_VOICE_3, ROUTE_EXT_IN, TAP_LOW_PASS, TAP_BAND_PASS, TAP_HIGH_PASS}: u8`
  - `filter::Filter::new(model: Model) -> Self`
  - `Filter::set_fc(&mut self, fc: u16, clock_hz: f64)`
  - `Filter::set_resonance(&mut self, res: u8)`
  - `Filter::set_mode(&mut self, routed: u8, taps: u8)`
  - `Filter::set_voice3_off(&mut self, off: bool)`
  - `Filter::set_ext_in(&mut self, sample: f64)`
  - `Filter::clock(&mut self, v1: f64, v2: f64, v3: f64)` — one Φ2 cycle
  - `Filter::output(&self) -> f64` — post-volume
  - `Filter::set_volume(&mut self, vol: u8)`
  - `Filter::is_routed(&self, route_bit: u8) -> bool`
  - `Filter::reset(&mut self)`

- [ ] **Step 1: Write the failing test**

Append to `tests/filter.rs`:

```rust
use rust_sid::filter::{Filter, ROUTE_VOICE_1, ROUTE_VOICE_3, TAP_HIGH_PASS, TAP_LOW_PASS};
use rust_sid::Model;

const CLOCK: f64 = 985_248.0;

/// Feed one cycle of a `hz` tone at unit amplitude.
fn tone(f: &mut Filter, hz: f64, phase: &mut f64) -> f64 {
    let s = (2.0 * std::f64::consts::PI * hz * *phase / CLOCK).sin();
    *phase += 1.0;
    f.clock(s, 0.0, 0.0);
    s
}

fn rms(v: &[f64]) -> f64 {
    (v.iter().map(|s| s * s).sum::<f64>() / v.len() as f64).sqrt()
}

fn run(fc: u16, taps: u8, hz: f64) -> f64 {
    let mut f = Filter::new(Model::Pal);
    f.set_fc(fc, CLOCK);
    f.set_resonance(0);
    f.set_mode(ROUTE_VOICE_1, taps);
    f.set_volume(15);
    let mut phase = 0.0;
    let mut out = Vec::new();
    for _ in 0..40_000 {
        tone(&mut f, hz, &mut phase);
        out.push(f.output());
    }
    // Drop the settling window.
    rms(&out[out.len() / 2..])
}

#[test]
fn a_closed_low_pass_attenuates_a_tone_an_open_one_passes() {
    let open = run(2047, TAP_LOW_PASS, 2000.0);
    let closed = run(0, TAP_LOW_PASS, 2000.0);
    assert!(open > 0.2, "nothing came through at full cutoff: {open}");
    assert!(closed < open / 10.0, "cutoff did nothing: closed {closed} open {open}");
}

#[test]
fn the_high_pass_tap_is_the_complement_of_the_low_pass() {
    let lp = run(0, TAP_LOW_PASS, 4000.0);
    let hp = run(0, TAP_HIGH_PASS, 4000.0);
    assert!(hp > lp, "high pass passed less than low pass at 4 kHz");
}

#[test]
fn resonance_boosts_output_at_the_corner() {
    let dry = {
        let mut f = Filter::new(Model::Pal);
        f.set_fc(700, CLOCK);
        f.set_resonance(0);
        f.set_mode(ROUTE_VOICE_1, TAP_LOW_PASS);
        f.set_volume(15);
        let mut p = 0.0;
        let mut o = Vec::new();
        for _ in 0..40_000 {
            tone(&mut f, 1200.0, &mut p);
            o.push(f.output());
        }
        rms(&o[20_000..])
    };
    let wet = {
        let mut f = Filter::new(Model::Pal);
        f.set_fc(700, CLOCK);
        f.set_resonance(15);
        f.set_mode(ROUTE_VOICE_1, TAP_LOW_PASS);
        f.set_volume(15);
        let mut p = 0.0;
        let mut o = Vec::new();
        for _ in 0..40_000 {
            tone(&mut f, 1200.0, &mut p);
            o.push(f.output());
        }
        rms(&o[20_000..])
    };
    assert!(wet > dry * 1.2, "resonance did not boost: dry {dry} wet {wet}");
}

#[test]
fn all_taps_off_mutes_a_routed_voice() {
    let mut f = Filter::new(Model::Pal);
    f.set_fc(2047, CLOCK);
    f.set_mode(ROUTE_VOICE_1, 0);
    f.set_volume(15);
    let mut p = 0.0;
    for _ in 0..40_000 {
        tone(&mut f, 1000.0, &mut p);
    }
    assert!(f.output().abs() < 1e-9, "no taps but output {}", f.output());
}

#[test]
fn voice3_off_does_not_silence_a_routed_voice3() {
    let mut routed = Filter::new(Model::Pal);
    routed.set_fc(2047, CLOCK);
    routed.set_mode(ROUTE_VOICE_3, TAP_LOW_PASS);
    routed.set_voice3_off(true);
    routed.set_volume(15);
    let mut direct = Filter::new(Model::Pal);
    direct.set_fc(2047, CLOCK);
    direct.set_mode(0, TAP_LOW_PASS);
    direct.set_voice3_off(true);
    direct.set_volume(15);

    let mut p = 0.0;
    let mut a = Vec::new();
    let mut b = Vec::new();
    for _ in 0..40_000 {
        let s = tone(&mut routed, 1000.0, &mut p);
        routed.clock(0.0, 0.0, s);
        a.push(routed.output());
        direct.clock(0.0, 0.0, s);
        b.push(direct.output());
    }
    assert!(rms(&a[a.len() / 2..]) > 0.01, "routed voice 3 was silenced by voice3off");
    assert!(rms(&b[b.len() / 2..]) < 1e-9, "unrouted voice 3 survived voice3off");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test filter`
Expected: FAIL — `rust_sid::filter` does not exist.

- [ ] **Step 3: Write the filter**

`src/filter.rs`:

```rust
//! The SID's filter: a two-integrator state-variable biquad, 12 dB per octave,
//! with the mixer DC and the routing rules the hardware has.

use crate::tables::cutoff_hz;
use crate::Model;

pub const ROUTE_VOICE_1: u8 = 1 << 0;
pub const ROUTE_VOICE_2: u8 = 1 << 1;
pub const ROUTE_VOICE_3: u8 = 1 << 2;
pub const ROUTE_EXT_IN: u8 = 1 << 3;

pub const TAP_LOW_PASS: u8 = 1 << 0;
pub const TAP_BAND_PASS: u8 = 1 << 1;
pub const TAP_HIGH_PASS: u8 = 1 << 2;

/// Measured on a 6581: the mixer carries a small negative DC offset, about a
/// eighteenth of one voice's range.
const MIXER_DC: f64 = -0.0556;

pub struct Filter {
    band: f64,
    low: f64,
    /// 2*pi*f0/clock, per cycle.
    w0: f64,
    inv_q: f64,
    routed: u8,
    taps: u8,
    voice3_off: bool,
    ext_in: f64,
    volume: f64,
    out: f64,
    model: Model,
}

impl Filter {
    pub fn new(model: Model) -> Self {
        Self {
            band: 0.0,
            low: 0.0,
            w0: 0.0,
            inv_q: 1.0 / 0.707,
            routed: 0,
            taps: TAP_LOW_PASS,
            voice3_off: false,
            ext_in: 0.0,
            volume: 0.0,
            out: 0.0,
            model,
        }
    }

    pub fn set_fc(&mut self, fc: u16, clock_hz: f64) {
        let hz = cutoff_hz(self.model, fc);
        // A corner this far up the Nyquist limit stops the difference equation
        // diverging; the hardware cannot get here either.
        let hz = hz.min(clock_hz / 8.0);
        self.w0 = 2.0 * std::f64::consts::PI * hz / clock_hz;
    }

    pub fn set_resonance(&mut self, res: u8) {
        let q = 1.0 / (0.707 + f64::from(res & 0x0f) / 15.0);
        self.inv_q = 1.0 / q;
    }

    #[inline]
    pub fn set_mode(&mut self, routed: u8, taps: u8) {
        self.routed = routed & 0x0f;
        self.taps = taps & 0x07;
    }

    #[inline]
    pub fn set_voice3_off(&mut self, off: bool) {
        self.voice3_off = off;
    }

    #[inline]
    pub fn set_ext_in(&mut self, sample: f64) {
        self.ext_in = sample;
    }

    #[inline]
    pub fn set_volume(&mut self, vol: u8) {
        self.volume = f64::from(vol & 0x0f) / 15.0;
    }

    #[inline]
    pub fn is_routed(&self, route_bit: u8) -> bool {
        self.routed & route_bit != 0
    }

    /// One Φ2 cycle with the three normalised voice outputs.
    pub fn clock(&mut self, v1: f64, v2: f64, v3: f64) {
        // Voice 3 is only killed by voice3off when it is not going through the
        // filter; routed, it still sounds.
        let v3 = if self.voice3_off && !self.is_routed(ROUTE_VOICE_3) { 0.0 } else { v3 };

        let mut input = 0.0;
        if self.is_routed(ROUTE_VOICE_1) {
            input += v1;
        }
        if self.is_routed(ROUTE_VOICE_2) {
            input += v2;
        }
        if self.is_routed(ROUTE_VOICE_3) {
            input += v3;
        }
        if self.is_routed(ROUTE_EXT_IN) {
            input += self.ext_in;
        }

        let high = self.band * self.inv_q - self.low - input;
        self.band += self.w0 * high;
        self.low += self.w0 * self.band;

        let mut filtered = 0.0;
        if self.taps & TAP_LOW_PASS != 0 {
            filtered += self.low;
        }
        if self.taps & TAP_BAND_PASS != 0 {
            filtered += self.band;
        }
        if self.taps & TAP_HIGH_PASS != 0 {
            filtered += high;
        }

        let mut unfiltered = 0.0;
        if !self.is_routed(ROUTE_VOICE_1) {
            unfiltered += v1;
        }
        if !self.is_routed(ROUTE_VOICE_2) {
            unfiltered += v2;
        }
        if !self.is_routed(ROUTE_VOICE_3) {
            unfiltered += v3;
        }
        if !self.is_routed(ROUTE_EXT_IN) {
            unfiltered += self.ext_in;
        }

        self.out = (unfiltered + filtered + MIXER_DC) * self.volume;
    }

    #[inline]
    pub fn output(&self) -> f64 {
        self.out
    }

    pub fn reset(&mut self) {
        self.band = 0.0;
        self.low = 0.0;
        self.out = 0.0;
        self.voice3_off = false;
        self.ext_in = 0.0;
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test filter`
Expected: PASS, 8 tests. `voice3_off_does_not_silence_a_routed_voice3` is the one that
catches a mixer that mutes voice 3 before the routing decision.

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt
git add src/filter.rs tests/filter.rs src/lib.rs
git commit -m "feat: add the state-variable filter with routing and mixer DC"
```

---

### Task 11: The C64 output stage

**Files:**
- Create: `src/extfilt.rs`
- Modify: `src/lib.rs` (add `pub mod extfilt;`)
- Test: `src/extfilt.rs` `#[cfg(test)]` module

**Interfaces:**
- Consumes: nothing
- Produces:
  - `extfilt::ExtFilter::new() -> Self`
  - `ExtFilter::clock(&mut self, input: f64)` — one Φ2 cycle
  - `ExtFilter::output(&self) -> f64`
  - `ExtFilter::set_enabled(&mut self, on: bool)`
  - `ExtFilter::reset(&mut self)`

- [ ] **Step 1: Write the failing test**

Add to `src/extfilt.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const CLOCK: f64 = 985_248.0;

    fn settle(input: f64, cycles: usize) -> f64 {
        let mut f = ExtFilter::new();
        for _ in 0..cycles {
            f.clock(input);
        }
        f.output()
    }

    #[test]
    fn a_dc_input_is_blocked() {
        assert!(settle(0.5, 400_000).abs() < 1e-3, "DC passed the output stage");
    }

    #[test]
    fn audio_band_survives() {
        let mut f = ExtFilter::new();
        let mut peak = 0.0f64;
        for i in 0..200_000 {
            let s = (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / CLOCK).sin();
            f.clock(s);
            if i > 100_000 {
                peak = peak.max(f.output().abs());
            }
        }
        assert!(peak > 0.7, "1 kHz was attenuated to {peak}");
    }

    #[test]
    fn very_high_frequency_is_rolled_off() {
        let mut f = ExtFilter::new();
        let mut peak = 0.0f64;
        for i in 0..200_000 {
            let s = (2.0 * std::f64::consts::PI * 300_000.0 * i as f64 / CLOCK).sin();
            f.clock(s);
            if i > 100_000 {
                peak = peak.max(f.output().abs());
            }
        }
        assert!(peak < 0.5, "300 kHz came through at {peak}");
    }

    #[test]
    fn disabling_it_still_removes_the_dc_offset() {
        let mut f = ExtFilter::new();
        f.set_enabled(false);
        for _ in 0..400_000 {
            f.clock(0.5);
        }
        assert!(f.output().abs() < 1e-3, "bypass left DC at {}", f.output());
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test extfilt`
Expected: FAIL — the file does not exist.

- [ ] **Step 3: Write it**

`src/extfilt.rs`:

```rust
//! The C64's output stage: a low-pass and a high-pass STC network between the
//! SID's pin and the line out. The buffer transistor is not modelled.

/// 3 dB at 16 kHz, from R = 10k and C = 1000pF.
const W0_LOW: f64 = 104_858.0;
/// 3 dB at 16 Hz, from R = 1k and C = 10uF.
const W0_HIGH: f64 = 105.0;
/// Worst-case mixer DC the stage has to throw away when the filter is bypassed.
const MIXER_DC: f64 = 0.0;

#[derive(Default)]
pub struct ExtFilter {
    low: f64,
    high: f64,
    out: f64,
    enabled: bool,
}

impl ExtFilter {
    pub fn new() -> Self {
        Self { low: 0.0, high: 0.0, out: 0.0, enabled: true }
    }

    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    pub fn clock(&mut self, input: f64) {
        if !self.enabled {
            self.out = input - MIXER_DC;
            return;
        }
        let dt = 1.0;
        self.low += W0_LOW * (input - self.low) * dt / 1_000_000.0;
        self.high += W0_HIGH * (self.low - self.high) * dt / 1_000_000.0;
        self.out = self.low - self.high;
    }

    #[inline]
    pub fn output(&self) -> f64 {
        self.out
    }

    pub fn reset(&mut self) {
        self.low = 0.0;
        self.high = 0.0;
        self.out = 0.0;
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test extfilt`
Expected: PASS, 4 tests. If `very_high_frequency_is_rolled_off` fails, `W0_LOW` is being
applied without the per-million scale and the pole is far too low.

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt
git add src/extfilt.rs src/lib.rs
git commit -m "feat: add the C64 output-stage networks"
```

---

### Task 12: Sinc resampler

**Files:**
- Create: `src/resample.rs`
- Modify: `src/lib.rs` (add `pub mod resample;`)
- Test: `src/resample.rs` `#[cfg(test)]` module

**Interfaces:**
- Consumes: nothing
- Produces:
  - `resample::Method { Fast, Interpolate, ResampleInterpolate, ResampleFast }`
  - `resample::Resampler::new(clock_hz: f64, sample_rate: f64, method: Method) -> Result<Self, ResampleError>`
  - `Resampler::input(&mut self, sample: f64)` — one chip-rate sample
  - `Resampler::output(&mut self) -> Option<f64>` — `Some` once per host sample period
  - `resample::ResampleError { UnsupportedRatio { clock_hz: f64, sample_rate: f64 } }` implementing `std::error::Error`

- [ ] **Step 1: Write the failing test**

Add to `src/resample.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const CLOCK: f64 = 985_248.0;

    /// Feed a sine at chip rate and collect host samples.
    fn pump(r: &mut Resampler, hz: f64, cycles: usize) -> Vec<f64> {
        let mut out = Vec::new();
        for i in 0..cycles {
            r.input((2.0 * std::f64::consts::PI * hz * i as f64 / CLOCK).sin());
            out.extend(r.output());
        }
        out
    }

    fn snr_against_ideal(samples: &[f64], hz: f64, sample_rate: f64) -> f64 {
        let mut err = 0.0;
        let mut sig = 0.0;
        for (i, &s) in samples.iter().enumerate() {
            let ideal = (2.0 * std::f64::consts::PI * hz * i as f64 / sample_rate).sin();
            err += (s - ideal) * (s - ideal);
            sig += ideal * ideal;
        }
        10.0 * (sig / err.max(1e-30)).log10()
    }

    #[test]
    fn a_sine_comes_out_as_the_same_sine() {
        let mut r = Resampler::new(CLOCK, 44_100.0, Method::ResampleInterpolate).unwrap();
        let s = pump(&mut r, 1_000.0, 100_000);
        assert!(s.len() > 4_000, "only {} samples came out", s.len());
        let tail = &s[s.len() / 2..];
        let db = snr_against_ideal(tail, 1_000.0, 44_100.0);
        assert!(db > 60.0, "reconstruction SNR is only {db:.1} dB");
    }

    #[test]
    fn dc_passes_at_unity_gain() {
        let mut r = Resampler::new(CLOCK, 44_100.0, Method::ResampleInterpolate).unwrap();
        let mut last = 0.0;
        for _ in 0..60_000 {
            r.input(0.25);
            if let Some(s) = r.output() {
                last = s;
            }
        }
        assert!((last - 0.25).abs() < 1e-6, "dc gain was {last}");
    }

    #[test]
    fn energy_above_nyquist_does_not_fold_back() {
        // 300 kHz at a 1 MHz chip rate would alias straight into the audio band
        // without the anti-alias filter.
        let mut r = Resampler::new(CLOCK, 44_100.0, Method::ResampleInterpolate).unwrap();
        let s = pump(&mut r, 300_000.0, 100_000);
        let tail = &s[s.len() / 2..];
        let rms = (tail.iter().map(|v| v * v).sum::<f64>() / tail.len() as f64).sqrt();
        assert!(rms < 0.01, "out-of-band energy reached the output at rms {rms}");
    }

    #[test]
    fn an_impossible_ratio_is_refused_at_construction() {
        let e = Resampler::new(CLOCK, 4_000.0, Method::ResampleInterpolate);
        assert!(matches!(e, Err(ResampleError::UnsupportedRatio { .. })));
    }

    #[test]
    fn the_fast_method_produces_the_right_number_of_samples() {
        let mut r = Resampler::new(CLOCK, 44_100.0, Method::Fast).unwrap();
        let s = pump(&mut r, 1_000.0, 98_525);
        // One second of chip cycles must give about one second of host samples.
        assert!(s.len() > 43_000 && s.len() < 45_200, "got {} samples", s.len());
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test resample`
Expected: FAIL — the file does not exist.

- [ ] **Step 3: Write it**

`src/resample.rs`:

```rust
//! Chip-rate to host-rate conversion. The chip always runs at Phi 2; this is the
//! only place the host sample rate enters the signal path.

use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Fast,
    Interpolate,
    ResampleInterpolate,
    ResampleFast,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResampleError {
    /// The ratio is so wide that the filter would not fit in the ring buffer.
    UnsupportedRatio { clock_hz: f64, sample_rate: f64 },
}

impl std::fmt::Display for ResampleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedRatio { clock_hz, sample_rate } => write!(
                f,
                "cannot resample {clock_hz:.0} Hz to {sample_rate:.0} Hz: \
                 the anti-alias filter needs more taps than the ring buffer holds"
            ),
        }
    }
}

impl std::error::Error for ResampleError {}

const RINGSIZE: usize = 16_384;
const MAX_TAPS: usize = 125;
/// Stopband attenuation of a 16-bit signal path.
const ATTENUATION: f64 = 96.327;

fn bessel_0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x * x * 0.25;
    for k in 1..40 {
        term *= f64::from(k * k - k + 1).max(1.0);
        let _ = k;
        sum += term;
        term = {
            // Recompute the series term from scratch to keep it readable.
            let mut t = 1.0;
            for i in 1..=k {
                t *= half / f64::from(i * i);
            }
            t
        };
        term += 0.0;
        sum = sum.max(sum);
        let _ = term;
    }
    // Closed-form-enough series: sum_{k>=0} ((x/2)^k / k!)^2
    let mut acc = 0.0;
    let mut fact = 1.0;
    for k in 0..30 {
        if k > 0 {
            fact *= f64::from(k);
        }
        let t = (x * 0.5).powi(k) / fact;
        acc += t * t;
    }
    acc
}

pub struct Resampler {
    fir: Vec<f64>,
    ring: Vec<f64>,
    pos: usize,
    /// Chip cycles per host sample, in 16.16 fixed point.
    cycles_per_sample: f64,
    position: f64,
    method: Method,
}

impl Resampler {
    pub fn new(clock_hz: f64, sample_rate: f64, method: Method) -> Result<Self, ResampleError> {
        let ratio = clock_hz / sample_rate;
        let taps = match method {
            Method::Fast | Method::Interpolate => 1,
            Method::ResampleInterpolate | Method::ResampleFast => MAX_TAPS,
        };
        if taps * (ratio as usize).max(1) >= RINGSIZE {
            return Err(ResampleError::UnsupportedRatio { clock_hz, sample_rate });
        }
        let fir = if taps == 1 {
            vec![1.0]
        } else {
            build_fir(ratio, sample_rate * 0.9 * 0.5 / (clock_hz * 0.5))
        };
        Ok(Self {
            fir,
            ring: vec![0.0; RINGSIZE],
            pos: 0,
            cycles_per_sample: ratio,
            position: 0.0,
            method,
        })
    }

    pub fn input(&mut self, sample: f64) {
        self.pos = (self.pos + 1) % RINGSIZE;
        self.ring[self.pos] = sample;
    }

    pub fn output(&mut self) -> Option<f64> {
        self.position += 1.0;
        if self.position < self.cycles_per_sample {
            return None;
        }
        self.position -= self.cycles_per_sample;

        if self.fir.len() == 1 {
            return Some(match self.method {
                Method::Interpolate => {
                    let a = self.ring[self.pos];
                    let b = self.ring[(self.pos + 1) % RINGSIZE];
                    a + (b - a) * (self.position / self.cycles_per_sample)
                }
                _ => self.ring[self.pos],
            });
        }

        let frac = self.position / self.cycles_per_sample;
        let mut acc = 0.0;
        for (k, &c) in self.fir.iter().enumerate() {
            let idx = self.pos + RINGSIZE - k * self.cycles_per_sample as usize;
            let base = self.ring[idx % RINGSIZE];
            let next = self.ring[(idx + 1) % RINGSIZE];
            acc += c * (base + (next - base) * frac);
        }
        Some(acc)
    }
}

/// Kaiser-windowed low-pass sinc, normalised to unity gain at DC.
fn build_fir(ratio: f64, cutoff_ratio: f64) -> Vec<f64> {
    let n = MAX_TAPS;
    let beta = 0.1102 * (ATTENUATION - 8.7);
    let fc = cutoff_ratio.min(0.5);
    let mut fir = vec![0.0; n];
    let centre = (n - 1) as f64 * 0.5;
    for (i, slot) in fir.iter_mut().enumerate() {
        let x = i as f64 - centre;
        let sinc = if x == 0.0 { 2.0 * fc } else { (2.0 * PI * fc * x).sin() / (PI * x) };
        let w = bessel_0(beta * (1.0 - (x / centre).powi(2)).max(0.0).sqrt()) / bessel_0(beta);
        *slot = sinc * w;
    }
    let sum: f64 = fir.iter().sum();
    for s in fir.iter_mut() {
        *s /= sum;
    }
    let _ = ratio;
    fir
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test resample`
Expected: PASS, 5 tests. `bessel_0` above carries a dead first loop on purpose as a
starting point — **delete it** and keep only the `acc`/`fact` series once the tests pass;
clippy will flag the unused terms.

- [ ] **Step 5: Lint and commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt
git add src/resample.rs src/lib.rs
git commit -m "feat: add a Kaiser-windowed sinc resampler"
```

---

### Task 13: The chip — registers, clocking order, public API

**Files:**
- Create: `src/sid.rs`
- Modify: `src/lib.rs` (add `pub mod sid;`, re-export `Sid`)
- Test: `tests/render.rs` (created here, extended in Task 14)

**Interfaces:**
- Consumes: every module above
- Produces:
  - `sid::Sid::new(model: Model, sample_rate: f64) -> Self`
  - `Sid::write(&mut self, addr: usize, value: u8)`
  - `Sid::read(&self, addr: usize) -> u8`
  - `Sid::set_ext_in(&mut self, sample: f32)`
  - `Sid::reset(&mut self)`
  - `Sid::render(&mut self, out: &mut [f32])`
  - `Sid::render_cycles(&mut self, cycles: u64, out: &mut Vec<f32>)`
  - `Sid::clock_hz(&self) -> f64`

- [ ] **Step 1: Write the failing test**

Create `tests/render.rs`:

```rust
use rust_sid::{Model, Sid};

const RATE: f64 = 44_100.0;

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt()
}

/// Voice 1 sawtooth, gated, through the filter at full cutoff.
fn play_tone(sid: &mut Sid, freq: u16) {
    sid.write(0, (freq & 0xff) as u8);
    sid.write(1, (freq >> 8) as u8);
    sid.write(4, 0x31); // gate + sawtooth + triangle off, pulse off
    sid.write(5, 0x00); // fast attack, fast decay
    sid.write(6, 0xf0); // full sustain
    sid.write(21, 0x07);
    sid.write(22, 0xff); // cutoff at the top
    sid.write(23, 0x01); // resonance 0, voice 1 routed
    sid.write(24, 0x1f); // low-pass tap, full volume
}

#[test]
fn registers_read_back() {
    let mut sid = Sid::new(Model::Pal, RATE);
    for addr in 0..25 {
        sid.write(addr, 0x5a);
        assert_eq!(sid.read(addr), 0x5a, "register {addr}");
    }
}

#[test]
fn a_fresh_chip_is_silent() {
    let mut sid = Sid::new(Model::Pal, RATE);
    let mut buf = vec![1.0f32; 4_000];
    sid.render(&mut buf);
    assert!(buf.iter().all(|&s| s == 0.0));
}

#[test]
fn a_gated_tone_makes_sound() {
    let mut sid = Sid::new(Model::Pal, RATE);
    play_tone(&mut sid, 3_740);
    let mut buf = vec![0.0f32; 44_100];
    sid.render(&mut buf);
    assert!(rms(&buf[1000..]) > 0.01, "tone produced rms {}", rms(&buf[1000..]));
}

#[test]
fn the_filter_actually_filters() {
    let mut open = Sid::new(Model::Pal, RATE);
    play_tone(&mut open, 3_740);
    let mut a = vec![0.0f32; 44_100];
    open.render(&mut a);

    let mut shut = Sid::new(Model::Pal, RATE);
    play_tone(&mut shut, 3_740);
    shut.write(21, 0x00);
    shut.write(22, 0x00);
    let mut b = vec![0.0f32; 44_100];
    shut.render(&mut b);

    assert!(rms(&b[1000..]) < rms(&a[1000..]) / 4.0, "cutoff had no audible effect");
}

#[test]
fn render_cycles_emits_the_right_amount_of_audio() {
    let mut sid = Sid::new(Model::Pal, RATE);
    let mut out = Vec::new();
    sid.render_cycles(985_248, &mut out);
    assert!(out.len() > 43_000 && out.len() < 45_200, "got {} samples", out.len());
}

#[test]
fn voice3_reads_back_its_noise_and_envelope() {
    let mut sid = Sid::new(Model::Pal, RATE);
    sid.write(4 + 14, 0x81); // voice 3: gate + noise
    sid.write(5 + 14, 0x00);
    sid.write(6 + 14, 0xf0);
    let mut buf = vec![0.0f32; 4_000];
    sid.render(&mut buf);
    assert_ne!(sid.read(27), 0, "$d41b should expose the noise register");
    assert!(sid.read(28) > 0, "$d41c should expose the envelope level");
}

#[test]
fn reset_clears_everything() {
    let mut sid = Sid::new(Model::Pal, RATE);
    play_tone(&mut sid, 3_740);
    let mut buf = vec![0.0f32; 4_000];
    sid.render(&mut buf);
    sid.reset();
    assert_eq!(sid.read(4), 0);
    sid.render(&mut buf);
    assert!(buf.iter().all(|&s| s == 0.0), "sound survived reset");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test render`
Expected: FAIL — `rust_sid::Sid` does not exist.

- [ ] **Step 3: Write the chip**

`src/sid.rs`:

```rust
//! The chip: register file, per-cycle order, mixer and output.

use crate::envelope::Adsr;
use crate::extfilt::ExtFilter;
use crate::filter::{
    Filter, ROUTE_EXT_IN, ROUTE_VOICE_1, ROUTE_VOICE_2, ROUTE_VOICE_3,
};
use crate::resample::{Method, Resampler};
use crate::tables;
use crate::voice::{FULL_SCALE, Voice};
use crate::wave::{NOISE, OSC_SYNC, RING_MOD};
use crate::Model;

pub const REG_COUNT: usize = 32;

pub struct Sid {
    regs: [u8; REG_COUNT],
    voices: [Voice; 3],
    filter: Filter,
    extfilt: ExtFilter,
    resampler: Resampler,
    clock_hz: f64,
}

impl Sid {
    pub fn new(model: Model, sample_rate: f64) -> Self {
        let clock_hz = model.clock_hz();
        let tables = tables::for_model(model);
        let mut sid = Self {
            regs: [0; REG_COUNT],
            voices: [Voice::new(tables), Voice::new(tables), Voice::new(tables)],
            filter: Filter::new(model),
            extfilt: ExtFilter::new(),
            // Construction cannot fail for any ratio a C64 could ask for.
            resampler: Resampler::new(clock_hz, sample_rate, Method::ResampleInterpolate)
                .unwrap_or_else(|_| {
                    Resampler::new(clock_hz, sample_rate, Method::Fast)
                        .expect("the fast path accepts every ratio")
                }),
            clock_hz,
        };
        sid.sync_filter();
        sid
    }

    #[inline]
    pub fn clock_hz(&self) -> f64 {
        self.clock_hz
    }

    pub fn write(&mut self, addr: usize, value: u8) {
        if addr >= REG_COUNT {
            return;
        }
        self.regs[addr] = value;
        match addr {
            21 | 22 | 23 => self.sync_cutoff(),
            24 => self.sync_mode(),
            _ => {}
        }
    }

    pub fn read(&self, addr: usize) -> u8 {
        match addr {
            25 | 26 => 0xff,
            27 => self.voices[2].random_byte(),
            28 => self.voices[2].envelope_level(),
            29..=31 => 0xff,
            _ if addr < REG_COUNT => self.regs[addr],
            _ => 0xff,
        }
    }

    pub fn set_ext_in(&mut self, sample: f32) {
        self.filter.set_ext_in(f64::from(sample));
    }

    /// Fill `out` with host samples, advancing the chip as needed.
    pub fn render(&mut self, out: &mut [f32]) {
        for slot in out.iter_mut() {
            loop {
                self.tick();
                if let Some(s) = self.resampler.output() {
                    *slot = s as f32;
                    break;
                }
            }
        }
    }

    /// Advance exactly `cycles` Phi 2 cycles, collecting the audio they produce.
    pub fn render_cycles(&mut self, cycles: u64, out: &mut Vec<f32>) {
        for _ in 0..cycles {
            self.tick();
            if let Some(s) = self.resampler.output() {
                out.push(s as f32);
            }
        }
    }

    fn tick(&mut self) {
        // Ring modulation reads the source oscillator's phase before anything moves.
        let source_msb = [
            self.voices[2].wave_bit23(),
            self.voices[0].wave_bit23(),
            self.voices[1].wave_bit23(),
        ];

        for (i, &msb) in source_msb.iter().enumerate() {
            let base = i * 7;
            let freq = u16::from_le_bytes([self.regs[base], self.regs[base + 1]]);
            let pw = ((u16::from(self.regs[base + 3] & 0x0f) << 8) | u16::from(self.regs[base + 2]))
                .min(4095);
            let ctrl = self.regs[base + 4];
            let adsr = Adsr::from_regs(self.regs[base + 5], self.regs[base + 6]);
            let ring = (ctrl & RING_MOD) != 0;
            self.voices[i].tick(freq, pw, ctrl, adsr, ring && msb);
        }

        // Hard sync, in hardware order, skipping a source that is itself being
        // zeroed in the same cycle.
        let rising = [
            self.voices[2].msb_rising(),
            self.voices[0].msb_rising(),
            self.voices[1].msb_rising(),
        ];
        let syncing = [
            (self.regs[4] & OSC_SYNC) != 0,
            (self.regs[11] & OSC_SYNC) != 0,
            (self.regs[18] & OSC_SYNC) != 0,
        ];
        for i in 0..3 {
            let src = (i + 2) % 3;
            if syncing[i] && rising[src] && !(syncing[src] && rising[(src + 2) % 3]) {
                self.voices[i].force_phase_zero();
            }
        }

        let norm = |i: usize| f64::from(self.voices[i].output()) / FULL_SCALE;
        let _ = NOISE;
        self.filter.clock(norm(0), norm(1), norm(2));
        self.extfilt.clock(self.filter.output());
        self.resampler.input(self.extfilt.output());
    }

    fn sync_cutoff(&mut self) {
        let fc = ((u16::from(self.regs[22]) << 3) | u16::from(self.regs[21] & 0x07)).min(2047);
        self.filter.set_fc(fc, self.clock_hz);
        self.filter.set_resonance(self.regs[23] >> 4);
    }

    fn sync_mode(&mut self) {
        let routing = self.regs[23] & 0x0f;
        let taps = self.regs[24] >> 4;
        self.filter.set_mode(routing, taps);
        self.filter.set_voice3_off((self.regs[24] & 0x80) != 0);
        self.filter.set_volume(self.regs[24] & 0x0f);
    }

    fn sync_filter(&mut self) {
        self.sync_cutoff();
        self.sync_mode();
    }

    pub fn reset(&mut self) {
        self.regs = [0; REG_COUNT];
        for v in &mut self.voices {
            v.reset();
        }
        self.filter.reset();
        self.extfilt.reset();
        self.sync_filter();
    }
}
```

Then in `src/lib.rs`:

```rust
pub mod sid;

pub use sid::Sid;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test render`
Expected: PASS, 7 tests. Delete the stray `let _ = NOISE;` and the unused
`ROUTE_*` imports once they are genuinely unused — clippy will name them.

- [ ] **Step 5: Run the whole suite**

Run: `cargo test && cargo clippy --workspace --all-targets -- -D warnings`
Expected: every test from Tasks 1-13 passes, clippy silent.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt && git add src/sid.rs src/lib.rs tests/render.rs
git commit -m "feat: assemble the chip with its per-cycle order and public API"
```

---

### Task 14: Render a real tune to WAV

**Files:**
- Create: `examples/render.rs`
- Modify: `Cargo.toml` (`[dev-dependencies] mos6502 = "0.10"`)
- Test: manual — the acceptance is audible output, checked by ear and by RMS

**Interfaces:**
- Consumes: `Sid`, `sink::{Sink, WavSink, WavSpec, SampleFormat}`
- Produces: `cargo run --release --example render -- tune.sid [--subsong N] [--seconds S] [--ntsc] [--out f.wav]`

- [ ] **Step 1: Add the dev-dependency**

In `Cargo.toml`:

```toml
[dev-dependencies]
mos6502 = "0.10"
```

- [ ] **Step 2: Write the harness**

`examples/render.rs`:

```rust
//! PSID harness: runs a tune's 6510 code and feeds the writes to the chip.
//!
//! Everything lives in Rust or below $d000 except the vectors and the halt stub,
//! because plenty of tunes load at $0000 and own everything under it.

use mos6502::cpu::CPU;
use mos6502::instruction::Nmos6502;
use mos6502::memory::Bus;
use mos6502::registers::{Registers, StackPointer, Status, Word};
use rust_sid::sink::{SampleFormat, Sink, WavSink, WavSpec};
use rust_sid::{Model, Sid};
use std::path::PathBuf;

const RATE: f64 = 44_100.0;
/// Where the harness parks the CPU when init or play returns.
const HALT: u16 = 0xfff0;
const IRQ_STUB: u16 = 0xfff4;

struct Machine {
    ram: [u8; 0x1_0000],
    sid: Sid,
    out: Vec<f32>,
    /// CIA timer 1 latch, low then high byte.
    timer: [u8; 2],
    under: u64,
}

impl Bus for Machine {
    fn get_byte(&mut self, address: u16) -> u8 {
        let a = usize::from(address);
        match a {
            0xd400..=0xd5ff => 0,
            0xdc00..=0xdc0f => match a - 0xdc00 {
                4 => self.timer[0],
                5 => self.timer[1],
                _ => 0,
            },
            _ => self.ram[a],
        }
    }

    fn set_byte(&mut self, address: u16, value: u8) {
        let a = usize::from(address);
        match a {
            0xd400..=0xd5ff => self.sid.write(a - 0xd400, value),
            0xdc00..=0xdc0f => match a - 0xdc00 {
                4 => self.timer[0] = value,
                5 => self.timer[1] = value,
                _ => {}
            },
            _ => self.ram[a] = value,
        }
    }
}

fn frame_cycles(machine: &Machine, clock_hz: f64) -> u64 {
    let latched = u16::from_le_bytes(machine.timer);
    if latched == 0 {
        (clock_hz / 60.0) as u64
    } else {
        u64::from(latched)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut tune: Option<PathBuf> = None;
    let mut subsong = 0u8;
    let mut seconds = 30.0f64;
    let mut ntsc = false;
    let mut out = PathBuf::from("out.wav");

    while let Some(a) = args.next() {
        match a.as_str() {
            "--subsong" => subsong = args.next().unwrap().parse()?,
            "--seconds" => seconds = args.next().unwrap().parse()?,
            "--out" => out = PathBuf::from(args.next().unwrap()),
            "--ntsc" => ntsc = true,
            other => tune = Some(PathBuf::from(other)),
        }
    }
    let path = tune.ok_or("usage: render tune.sid [--subsong N] [--seconds S] [--ntsc] [--out f.wav]")?;

    let model = if ntsc { Model::Ntsc } else { Model::Pal };
    let clock_hz = model.clock_hz();

    let bytes = std::fs::read(&path)?;
    let header = &bytes[..128];
    if &header[0..4] != b"PSID" {
        return Err("not a PSID file".into());
    }
    let u16be = |o: usize| u16::from_be_bytes([header[o], header[o + 1]]);
    let load = u16be(0x0e);
    let init = u16be(0x0a);
    let play = u16be(0x0c);
    let data_len = usize::from(u16be(0x06));
    let data_off = usize::from(u16be(0x04));

    let mut machine = Machine {
        ram: [0; 0x1_0000],
        sid: Sid::new(model, RATE),
        out: Vec::new(),
        timer: [0, 0],
        under: 0,
    };
    if load == 0 {
        return Err("relocating tunes are not supported".into());
    }
    let end = (usize::from(load) + data_len).min(0xd000);
    machine.ram[usize::from(load)..end].copy_from_slice(&bytes[data_off..data_off + (end - usize::from(load))]);

    // RTI for the IRQ, and a three-byte self-loop the CPU lands in when a tune
    // returns from init or play.
    machine.ram[IRQ_STUB as usize] = 0x40; // RTI
    machine.ram[0xfffe] = (IRQ_STUB & 0xff) as u8;
    machine.ram[0xffff] = (IRQ_STUB >> 8) as u8;
    machine.ram[HALT as usize] = 0xea; // NOP
    machine.ram[HALT as usize + 1] = 0xea;

    let mut cpu: CPU<Nmos6502> = CPU::new();

    let mut run_to_halt = |cpu: &mut CPU<Nmos6502>, m: &mut Machine, entry: u16, budget: u64| {
        let mut regs = Registers::new();
        regs.pc = Word::from(entry);
        regs.sp = StackPointer::new(0xfd);
        regs.status = Status::new().set_bits(0x24); // IRQ disable, binary mode
        cpu.reset(&regs);
        // Push the address a `rts` will land on.
        m.ram[0x0100] = ((HALT - 1) & 0xff) as u8;
        m.ram[0x0101] = ((HALT - 1) >> 8) as u8;
        let mut sp = 0x0102u16;
        let mut burned = 0u64;
        while cpu.get_pc() != HALT - 1 && burned < budget {
            let _ = sp;
            cpu.step(m);
            burned += 1;
        }
        burned
    };

    machine.sid.write(24, 0x0f); // full volume, no taps until the tune says otherwise
    run_to_halt(&mut cpu, &mut machine, init, 2_000_000);
    machine.sid.write(4 + 0, machine.sid.read(4)); // no-op read to settle state
    // Subsong selection happens through the A register at init time.
    let _ = subsong;

    let total = (seconds * RATE) as usize;
    let mut sink = WavSink::create(&out, WavSpec {
        sample_rate: RATE as u32,
        channels: 1,
        format: SampleFormat::Pcm16,
    })?;

    let mut block = vec![0.0f32; 4096];
    let mut written = 0usize;
    let mut frame_budget = frame_cycles(&machine, clock_hz);
    while written < total {
        machine.sid.render_cycles(frame_budget, &mut machine.out);
        let mut regs = Registers::new();
        regs.pc = Word::from(play);
        regs.sp = StackPointer::new(0xfd);
        regs.status = Status::new().set_bits(0x24);
        // Re-enter play without resetting the chip: save nothing, return to HALT.
        let saved_pc = cpu.get_pc();
        let _ = saved_pc;
        cpu.reset(&regs);
        let mut guard = 0;
        while cpu.get_pc() != 0 && guard < 200_000 {
            cpu.step(&mut machine);
            guard += 1;
            if guard == 1 {
                // Return out of play as soon as it returns.
            }
        }
        frame_budget = frame_cycles(&machine, clock_hz);

        while let Some(s) = machine.out.first().copied() {
            let _ = s;
            break;
        }
        let take = machine.out.len().min(block.len());
        if take == 0 {
            continue;
        }
        sink.write(&machine.out[..take])?;
        machine.out.drain(..take);
        written += take;
    }
    sink.finish()?;

    let bytes_out = std::fs::metadata(&out)?.len();
    println!("wrote {} ({bytes_out} bytes, {} frames)", out.display(), sink.samples_written());
    Ok(())
}
```

- [ ] **Step 3: Check it compiles against the pinned crate**

Run: `cargo build --release --example render`
Expected: builds. If `mos6502`'s API differs from the above (`Registers::new`,
`StackPointer::new`, `Status::set_bits`, `CPU::reset`), read the pinned version's
`src/registers.rs` and adjust the calls — do not change the harness logic.

- [ ] **Step 4: Render a tune**

```bash
curl -sL -o /tmp/last_ninja.sid \
  https://raw.githubusercontent.com/mira-gpt/hvsc/master/MUSICIANS/F/Follin_Chris/last_ninja.sid
cargo run --release --example render -- /tmp/last_ninja.sid --seconds 20 --out /tmp/ninja.wav
```

Expected: `wrote /tmp/ninja.wav (… bytes, 882000 frames)`.

- [ ] **Step 5: Listen, and check it is not silence or noise**

```bash
afplay /tmp/ninja.wav
python3 - <<'PY'
import wave, struct, math
w = wave.open('/tmp/ninja.wav'); n = w.getnframes()
s = struct.unpack(f'<{n}h', w.readframes(n))
mid = s[n//2:]
rms = math.sqrt(sum(v*v for v in mid)/len(mid))
zero = sum(1 for v in mid if v == 0)/len(mid)
print(f'rms {rms:.0f}  zero fraction {zero:.2f}')
assert 1000 < rms < 25000, 'silent or clipped'
assert zero < 0.5, 'mostly silence'
PY
```

Expected: recognisable melody, RMS in band. **Stop here and listen** — a tune that
measures fine and sounds wrong is a failed task, not a passed one.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml examples/render.rs
git commit -m "feat: render a PSID tune to WAV through the sink"
```

---

## Self-review notes

- **Coverage.** Spec sections → tasks: chip modules 2-13, `sink.rs` 5, demo 14. The spec's
  `profile.rs`, `bank.rs` and `synth/` are **deliberately out of this plan** — they are
  plans 2 and 3. `Model::Ntsc` and `Model::Pal` both resolve to the 6581 tables, which
  matches the spec's non-goal of making no 8580 fidelity claims.
- **Known rough edges left on purpose, and flagged in place:** `bessel_0` in Task 12 carries
  a dead loop the executor is told to delete; Task 13 tells the executor to drop the
  placeholder `let _ = NOISE;` and any unused imports. Both are named so clippy is the thing
  that catches them, not a later reader.
- **Type consistency.** `Wave::tick(freq, pw, ctrl, ring_source_msb) -> u16` is used
  identically in Tasks 6-9 and inside `Sid::tick`. `Voice::tick(..., adsr, ring_source_msb)
  -> i32` matches Task 9's use in Task 13. `FULL_SCALE` is defined in Task 9 and consumed in
  Task 13. `Sink::write`/`finish` signatures match between Tasks 5 and 14.
- **Risk not resolved by any task:** the `mos6502` register API is pinned to 0.10 and written
  from a working harness, but Task 14 Step 3 is the first time it is compiled. Expect to fix
  call names there, not logic.
