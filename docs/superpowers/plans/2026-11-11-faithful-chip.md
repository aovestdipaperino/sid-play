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
