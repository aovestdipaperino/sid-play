<p align="center">
  <img src="crates/sid-play/assets/logo.svg" alt="sid-play: a singing 6581 chip sending waveforms past a play button" width="560">
</p>

<p align="center">
  <b>Commodore 64 music, played by a cycle-stepped SID emulator in Rust.</b><br>
  Permissively licensed, with no GPL anywhere. Honest about what that costs.
</p>

---

`sid-play` takes a PSID tune from the High Voltage SID Collection, runs the
tune's own 6510 machine code one clock cycle at a time, and feeds every
register write to an emulated MOS 6581 SID chip, which renders a WAV file.

```sh
cargo run --release -p sid-play --bin render -- tune.sid --seconds 60 --out tune.wav
```

Playing out loud needs the `playback` feature, which adds `cpal` and opens the
default output device:

```sh
cargo run --release -p sid-play --features playback --bin render -- tune.sid --sink audio
```

## The crates

Two crates in two repositories, both on crates.io:

| | Crate | What it is | Licence |
|---|---|---|---|
| <img src="https://raw.githubusercontent.com/aovestdipaperino/mos6581/main/assets/logo.svg" width="120"> | [`mos6581`](https://github.com/aovestdipaperino/mos6581) | The SID chip: oscillators, envelopes, filter, output stage, resampler. Stepped once per Φ2 cycle. | MIT OR Apache-2.0 |
| <img src="crates/sid-play/assets/logo.svg" width="120"> | [`sid-play`](crates/sid-play) | The player: PSID loader, a minimal C64 around the tune (CIA timer, memory, call stubs), the `render` binary and a WAV writer. | MIT OR Apache-2.0 |

The CPU is a separate project: [`mos6510`](https://github.com/aovestdipaperino/mos6510)
([crates.io](https://crates.io/crates/mos6510)), a cycle-exact 6510 that passes
Tom Harte's per-cycle test suite and Klaus Dormann's functional test.

## How it got here

This started as one crate called rust-sid, licensed GPL-2.0 because a few of its
tables came from reSID. For a long time 13 of 16 sampled tunes played nothing
at all, and the investigation pointed everywhere except at the CPU crate it was
using, which turned out to ignore Y in `LDA (ptr),Y` and to never set the carry
flag in binary `ADC`. Replacing that CPU with `mos6510` made all 16 play.

With the music working, the licence was the next problem. One GPL table makes
the whole crate GPL. So every source file went through a provenance audit
(recorded in [`docs/provenance.md`](https://github.com/aovestdipaperino/mos6581/blob/main/docs/provenance.md)): each table, constant
and formula was traced to a public source, an own derivation, or a GPL origin.
Everything with a GPL origin moved into a separate GPL-2.0 data crate behind an
off-by-default feature. The values the chip cannot run without were then
written again from public sources only (the MOS datasheet, the chip designer's
published interview, the SID patent, C64 schematics) by an author who was not
allowed to look at any emulator's code. Each value carries its source; the few
that are plain judgement calls say so.

A second clean-room round then modelled the last borrowed pieces: the filter
cutoff curve, the combined waveforms and the power-on RAM image. With nothing
left to borrow, mos6581 0.2.0 and sid-play 0.2.0 dropped that feature and the
dependency on the data crate. The data crate still exists, unchanged, under
GPL-2.0-only, but nothing here uses it.

## What the clean-room build gives up

Measured, not guessed. Every one of the 16 HVSC tunes and Last Ninja plays, with
tonality medians from 0.34 to 0.85 over 10 s renders (the per-tune numbers are
in [`HANDSOFF.md`](HANDSOFF.md)). The differences from real silicon that matter:

- The filter cutoff curve closes at about the right frequency, but its mid-range
  shape rests on unsourced DAC-voltage choices and is off by a median of about
  0.7 octave.
- The combined waveforms (triangle, sawtooth and pulse mixed) come from a model
  that is much closer to the measured chip than the old bitwise AND, but pulse
  with triangle comes out silent, which is quieter than the real chip.
- Envelope curve shapes, the noise generator's wiring, the resonance range and
  the DC levels are clean-room values, so they sound close but not identical.
- The power-on RAM image is built from documentation and matches 973 of 1024
  captured bytes. `render --poweron FILE` loads a 1024-byte image of your own
  instead.

The full list is in the "Known default-build fidelity gaps" section of
[`docs/provenance.md`](https://github.com/aovestdipaperino/mos6581/blob/main/docs/provenance.md).

## Testing

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

`crates/sid-play/scripts/tonality.py out.wav` prints a render's median
autocorrelation peak (0.00 is silence, higher is more tonal).

## Credits

The SID chip was designed by Bob Yannes, whose 1996 interview is one of the
public sources the clean-room values cite.

## Licence

`mos6581` and `sid-play`: MIT OR Apache-2.0, at your option, with no GPL code or
data and no optional GPL build. Each lives in its own repository; this one holds
the player and its documents, which are MIT OR Apache-2.0.
