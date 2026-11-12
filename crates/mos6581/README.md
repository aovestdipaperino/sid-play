# mos6581

A cycle-exact MOS 6581 SID chip emulator in Rust. The chip is stepped one Φ2
cycle at a time and has no runtime dependencies in the default build. It is only
the chip: no CPU, no memory map, no player. See `sid-play` for a PSID player built
on it.

Licence: MIT OR Apache-2.0.

## Fidelity

The default build is GPL-free and clean-room. It approximates the chip in these
ways:

- Combined waveforms (the ones that mix triangle, sawtooth and pulse) are the
  bitwise AND of the pure waveforms, not the measured tables.
- The filter cutoff is linear from 30 Hz to 12 kHz over the 11-bit register, the
  datasheet's nominal range, not the measured curve.
- The envelope periods, voice and mixer DC, resonance, noise taps and the output
  stage constants come from the datasheet and public documents, or are plain
  choices. They are listed with their derivations in
  [`docs/provenance.md`](../../docs/provenance.md).

Building with `--features gpl-data` swaps in measured tables and constants from
the `sid-gpl-data` crate (reSID data, GPL-2.0). That makes the resulting build
GPL-2.0, so the combined work must be distributed under the GPL.

How far the default build is from the measured one:

- Combined waveform tables, from
  `cargo test -p mos6581 --features gpl-data --test gap -- --nocapture`
  (difference between the AND tables and the measured ones, in 0..255 units):
  `GAP st: rms 83.8 max 170`, `GAP p_t: rms 139.2 max 255`,
  `GAP ps_: rms 140.9 max 254`, `GAP pst: rms 84.4 max 170`.
- Cutoff: the default curve closes at 30 Hz, the measured one at about 200 Hz, so
  tunes that play through a mostly closed low-pass are much quieter. In the
  `sid-play` fixtures the scale tune renders at rms 53 by default and 1362 with
  `gpl-data`.

The full list of known gaps is in the "Known default-build fidelity gaps" section
of [`docs/provenance.md`](../../docs/provenance.md).
