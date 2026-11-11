# rust-sid — design

Date: 2026-11-11
Status: approved for spec review
Location: `chipsynth64/rust-sid/`

## Summary

`rust-sid` is a fresh Rust implementation of the MOS 6581 SID, structured after reSID's
module boundaries, using reSID's measured waveform data (vendored, GPL-2.0), extended with a
measured-amplitude and filter-nonlinearity layer, a synth layer above the chip, and a demo
that plays `.sid` files.

It shares no code with `sid6581` (the crate beside it). It is a reimplementation, not a port.

## Decisions taken, and why

| Decision | Choice | Reason |
|---|---|---|
| Base | Fresh crate following reSID's structure | Clean lineage, true reimplementation; nothing inherited |
| Waveform data | Vendor reSID's measured dumps; crate is GPL-2.0-only | Best combined-waveform accuracy on day one; attribution travels with the data |
| Behaviour scope | Full: measured amplitude, nonlinear filter, Hz/cents cutoff, 31 combined-wave modes, >3 voices | Requested |
| Architecture | One faithful chip core; amplitude/nonlinearity as a swappable profile inside it; `Ensemble` above it | Chip fidelity is load-bearing for `.sid` playback; the amplitude wins never touch the time domain |
| The 31 combine modes | Synth layer only, unreachable from the faithful chip path | They are sound-design algorithms, not hardware behaviour |

## Non-goals

- No 8580 waveform fidelity claims beyond the vendored 8580 tables and its documented
  differences (`wave_zero`, `voice_DC`, linear cutoff curve, no mixer DC).
- No bankswitching / EXO / relocating second-part support in the demo.
- No VST/AU or any plugin wrapper.
- No MIDI, no sequencer, no GUI.
- No attempt to be cycle-exact to a specific silicon revision beyond what reSID models.

## Why the chip and the synth must be separate

A real 6581 has three voices sharing one filter, with oscillator sync and ring modulation
wired between specific voices (`v0←v2`, `v1←v0`, `v2←v1`). That coupling, plus per-cycle
register response, is what makes digi and register playback work, and it is why a faithful
emulator is cycle-accurate.

Polyphony requires the opposite: each voice with its own filter state, and a pitch domain in
Hz rather than in 24-bit accumulator increments. Those two requirements cannot share one
engine without one degrading the other.

So: `Sid` is the chip — three voices, one shared filter, real sync and ring chains, one tick
per Φ2 cycle. `Ensemble` is a synth — N chip-voices, each with its own filter state, mixed
together, cutoff in Hz with cents modulation. The `.sid` demo uses exactly one `Sid`.
Polyphony is opt-in and never claims to be hardware.

## Crate layout

```
rust-sid/
  Cargo.toml            GPL-2.0-only; no default deps, `playback` feature adds cpal
  LICENSE               GPL-2.0
  README.md             what it is, licence, where the vendored data came from
  src/
    lib.rs              Sid, Model, Profile, public re-exports
    sid.rs              clocking, register file, mixer, output scaling
    voice.rs            oscillator + envelope + amplitude for one voice
    wave.rs             waveform generation, combination tables, noise, sync/ring/test
    envelope.rs         ADSR: rate table, exponential periods, delay bug, hold_zero
    filter.rs           two-integrator biquad; linear and nonlinear variants
    extfilt.rs          C64 output-stage STC networks
    resample.rs         Kaiser-windowed sinc FIR; four sampling methods
    profile.rs          Profile, CutoffLaw, ShaperBank
    bank.rs             Pls2 reader — runtime, from a caller-supplied path
    sink.rs             Sink trait + WavSink; AudioSink behind the `playback` feature
    tables/
      mod.rs
      wave6581.rs       vendored combination tables
      wave8580.rs       vendored combination tables
      fc.rs             spline points -> 2048-entry f0 tables, both models
    synth/
      mod.rs
      ensemble.rs       N chip-voices, per-voice filter state, mixing
      combine.rs        the 31 combined-waveform algorithms
  examples/
    psid.rs             .sid player; the only binary in the crate
  data/                 gitignored; where a user drops a measured bank
  tests/
    envelope.rs filter.rs wave.rs routing.rs render.rs
```

## Components

### `sid.rs`
Owns the 24-register file, the three voices, the shared filter, the external filter and the
resampler. Public surface:

```rust
pub struct Sid { /* ... */ }
impl Sid {
    pub fn new(model: Model, sample_rate: f64) -> Self;
    pub fn with_profile(model: Model, sample_rate: f64, profile: Profile) -> Self;
    pub fn write(&mut self, addr: usize, value: u8);
    pub fn read(&self, addr: usize) -> u8;
    pub fn set_ext_in(&mut self, sample: f32);
    pub fn reset(&mut self);
    pub fn render(&mut self, out: &mut [f32]);            // host-rate API
    pub fn render_cycles(&mut self, cycles: u64, out: &mut Vec<f32>); // CPU-driven API
}
pub enum Model { Ntsc, Pal }
```

Per-cycle order: voices → sync resolution → mixer split (routed vs direct) → filter →
extfilt → resampler. Register writes take effect on the next cycle; no latching, no glide.

### `wave.rs`
24-bit accumulator. Four combination tables indexed by the waveform-select bits, plus a
pulse comparator. Noise: 23-bit shift register, feedback `bit22 ^ bit17`, shifting left,
clocked once per rising edge of accumulator bit 19, output assembled from taps
{22, 20, 16, 13, 11, 7, 4, 2}. Ring modulation XORs using the **source oscillator's
accumulator MSB**. Sync zeroes the slave accumulator on the source's MSB rise, including the
case where the source is itself being synced that cycle. Test zeroes the accumulator and
forces the pulse output high; clearing test reloads the shift register.

### `envelope.rs`
15-bit rate counter with the wrap-at-`0x7fff` delay bug. Rate periods
`[9,32,63,95,149,220,267,313,392,977,1954,3126,3907,11720,19532,31251]`. Exponential
counter periods 1,2,4,8,16,30 engaged at envelope values 255,93,54,26,14,6 and 1 at 0.
Sustain target = nibble × 17. `hold_zero` freezes at zero until attack.

### `filter.rs`
Two-integrator state-variable biquad, 12 dB/oct taps. Cutoff from a 2048-entry `f0` table
built at construction from spline points, per model. Resonance to Q. Mixer DC for the 6581.
Voice 3 is silenced by voice-3-off **only when it is not routed through the filter**.
The nonlinear variant adds, in order: input bias terms, the shaper in the HP and BP paths,
per-mode tap weights, and bleed past a closed filter.

### `resample.rs`
Kaiser-windowed sinc FIR, 96 dB stopband, order ≤ 124, ring buffer 16384, four sampling
methods (fast / interpolate × resample / no-resample). Constraint checks on sample rate and
pass frequency, matching reSID's limits.

### `profile.rs` — the seam

```rust
pub struct Profile {
    pub env_curve: [f32; 256],        // envelope level -> amplitude; identity by default
    pub cutoff: CutoffLaw,            // Law::Model | Law::Table([f32; 2048])
    pub reso_q: [f32; 16],            // RES nibble -> Q
    pub shaper: Option<ShaperBank>,   // resonance-indexed curves, crossfaded
    pub tap_gain: [[f32; 3]; 8],      // filter mode -> LP/BP/HP weights
    pub volume: [[f32; 16]; 16],      // [filter_mode * 2 + voice3_off][resonance] -> gain
    pub bleed: f32,                   // signal leaking past a closed filter
}
impl Profile {
    pub fn faithful() -> Self;   // phase-1 behaviour: linear, identity curves
    pub fn expressive() -> Self; // nonlinear defaults, no external data
}
```

**The invariant:** the chip consults `Profile` at exactly four points — envelope multiply,
cutoff lookup, resonance→Q, output mix. None of them is in the time domain. A profile may
change what things sound like; it may never change when things happen. Every phase-1 timing
test must pass under every profile.

### `bank.rs`
Reads a Pls2 RIFF bank from a path the caller supplies and fills a `Profile`. No path is
compiled in, no file is read unless asked, and no measured bank is committed. Missing or
malformed file returns `Err(BankError)`, never a panic.

### `sink.rs`

Where the samples go is a consumer concern, but the seam is shared, so the trait lives in the
library and the two implementations are interchangeable.

```rust
pub trait Sink {
    type Error;
    /// One block of mono samples at the chip's host sample rate.
    fn write(&mut self, samples: &[f32]) -> Result<(), Self::Error>;
    /// Flush and, where relevant, close. Called once on clean shutdown.
    fn finish(&mut self) -> Result<(), Self::Error>;
}
```

- Mono in. Each implementation adapts: the WAV sink writes the channel count it was opened
  with, the audio sink duplicates onto the device's channels.
- `WavSink` is in the default build. It writes a canonical 44-byte RIFF/WAVE header, PCM16
  or float32, and patches the two length fields in `finish`. No dependency.
- `AudioSink` is behind the `playback` feature and is the only thing in the crate that pulls
  `cpal`. It opens the default output device, blocks on a ring buffer, and reports device
  failure as an error rather than falling back silently.
- The chip never knows which sink it is feeding; nothing in `sid.rs` references this module.

### `synth/ensemble.rs`
N chip-voices, each with independent filter state, mixed to a stereo bus. Cutoff expressed
in Hz with cents modulation: `cutoff = base * 2^(cents/1200)`, base from a semitone-spaced
table. Voice allocation with stealing. No shared filter, no cross-voice sync or ring — and
the type does not expose the hardware register file as if it did.

### `synth/combine.rs`
The 31 combined-waveform algorithms, selected by an enum. Reachable only from `synth`. The
faithful `Sid` path cannot select them.

### The demo
`cargo run --release --example psid -- tune.sid [--subsong N] [--ntsc] [--sink wav|audio]
[--out file.wav]` — PSID load, `mos6502` as a dev-dependency so the library stays
dependency-free, RAM below `$d000`, halt trick at `$f000`, CIA-timer IRQ at 60 Hz. The demo
holds a `Box<dyn Sink>` chosen by flag: `wav` is always available, `audio` is available when
the crate is built with `--features playback`. Adding a third destination must not require
touching the playback loop.

## Data flow

```
6510 writes $d400..$d418
        v
   Sid::write  --(next cycle)-->  register file
        v
  per Φ2 cycle:  wave.rs  ->  envelope.rs  ->  amplitude (Profile.env_curve)
        v
   sync / ring resolution
        v
   mixer: routed sum -> filter.rs -> extfilt.rs
          direct sum ----------------->|
        v
   Profile.volume, Profile.bleed
        v
   resample.rs  ->  host samples (f32)
```

## Error handling

- Library: no `panic!` on bad input. Out-of-range register writes are ignored; reads of
  unwritable registers return the shadow value; `$d41b` returns the noise register, `$d41c`
  the envelope level.
- `bank.rs`: `Result<Profile, BankError>` with variants for missing file, short chunk,
  unknown chunk, and a chunk whose size does not match its name.
- Sinks: `write`/`finish` return their own error type; a short write or a failed flush is
  propagated, never swallowed. `AudioSink` reports device-open failure as an error and does
  **not** silently fall back to a file — the demo prints the reason and exits nonzero,
  because a tune that played silently is worse than one that refused to start.
- Demo: `thiserror`-free, a small `Error` enum plus `main() -> Result<(), Box<dyn Error>>`.
- Resampler: reject sample rates that violate the FIR/ring-buffer constraint at
  construction, with the reason, rather than producing garbage silently.

## Testing

- **Analytic.** Envelope: 32 352 cycles per envelope at A=D=R=1. Cutoff: `f0[0] ≈ 220 Hz`,
  `f0[2047] ≈ 18 kHz`, and the discontinuity at `fc = 1024`. Table dimensions and the
  identity-saw / triangle table shapes.
- **Statistical.** Noise: fixed seed, mean near zero, RMS in band, and determinism across
  two runs.
- **Structural.** Voice 3 with voice-3-off set and routed through the filter must still be
  audible; unmuted and unrouted must be silent. All taps off mutes routed voices. Sync
  locks the slave phase. Ring mod with a triangle source differs from a sawtooth source
  (proves the source is read as phase, not output).
- **Invariant.** Every timing test runs under `Profile::faithful()` and
  `Profile::expressive()` and must produce identical cycle counts.
- **Render regression.** One fixed tune, checked in RMS and spectral-centroid bands, not
  byte-exact.
- **Lint.** `cargo clippy --workspace --all-targets -- -D warnings` clean from the first
  commit; the global pre-commit hook enforces it.

## Phases and definition of done

1. **Faithful chip.** reSID-equivalent DSP, vendored tables, extfilt, resampler, `Sink`
   trait and `WavSink`.
   *Done:* analytic, statistical and structural tests pass; a real `.sid` renders to a WAV
   you can hear.
2. **Profile layer.** Measured amplitude curves, cutoff law, resonance→Q, shaper bank,
   tap weights, volume table, bleed; `bank.rs` loader.
   *Done:* A/B render differs audibly as expected; all phase-1 timing tests pass under both
   profiles; no measured data in git.
3. **Synth layer.** `Ensemble`, Hz/cents cutoff, the 31 combine modes.
   *Done:* more than three simultaneous voices; phase-1 regression suite untouched.
4. **Demo.** The `psid` example with sink selection, plus `AudioSink` behind `playback`.
   *Done:* `--features playback -- --sink audio` plays a tune out loud; `--sink wav` renders
   with no audio device present; the playback loop is identical between the two.

## Licensing and provenance rules

- Crate licence: `GPL-2.0-only`. `LICENSE` present. README states the crate is GPL because
  it contains reSID's measured data.
- The default build pulls in no third-party code at all. `cpal` enters only through the
  `playback` feature, and nothing outside `sink.rs` may depend on it.
- Every vendored table carries its `reSID (Dag Lem), GPL-2.0` header. This attribution is
  required by the licence and is not negotiable.
- **No vendor product names anywhere in the source.** Mechanisms are described by behaviour,
  not by origin.
- **Provenance comments must be true.** Default is no comment. `measured on real 6581
  hardware` only where that is what it is. `per-chip, loaded at runtime` for bank-driven
  values. Never a claim about origin that cannot be backed.
- Third-party measured banks are loaded from a caller-supplied path at runtime and are never
  committed, embedded, or redistributed.

## Risks and open questions

- **Shaper bank layout.** The resonance-indexed curve bank's exact stride is not confirmed;
  the shipped `ShaperBank` will define its own layout and the loader will map into it rather
  than assume a file's internal geometry.
- **Envelope curve normalisation.** Normalising a measured curve between its own endpoints
  discards absolute level, which changes loudness between chips. Decide per profile whether
  absolute or normalised is used; default to normalised with a documented flag.
- **Nested repository.** `rust-sid` is a git repo inside `chipsynth64`, which itself has no
  commits. `rust-sid` gets its own `git init` and `chipsynth64/.gitignore` gains
  `/rust-sid/`.
- **`cpal` on this machine.** Live output is untested until phase 4; the `--render` path is
  the fallback and is what CI-equivalent checks use.
