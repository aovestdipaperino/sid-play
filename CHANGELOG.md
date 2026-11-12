# Changelog

## Unreleased

### Fixed (harness timing and interrupt model)

- CIA-speed songs now run on timer A in the KERNAL post-reset state, so latch writes change the tempo. `Axel_F` had played 9% fast.
- VBL songs run on the VIC-II frame: 312 x 63 = 19656 cycles PAL, 263 x 65 = 17095 NTSC (6567R8). Before, the period was clock/50, about 0.25% slow.
- A KERNAL-like IRQ chain: $FFFE saves registers and jumps through ($0314); the default handler acknowledges $DC0D and returns.
- A non-zero play address is always driven by the harness, and the KERNAL timer A runs for every song.
- Header length is 0x76 for v1 and 0x7C for v2 and later.
- The CIA CRA force-load strobe is honoured.

Effect: tempo and timing now follow the hardware. The median spectral distance to the reference player fell from 5.9 dB to 4.5 dB on these fixes alone (see `HANDSOFF.md`). Known stand-in: VBL songs with play=0 get the CIA's 60 Hz rather than a 50 Hz raster IRQ, since there is no VIC-II.

## 0.2.0 (2026-11-12)

### Breaking

- Removed the `gpl-data` feature and the optional `sid-gpl-data` dependency.
  There is now a single build, licensed MIT OR Apache-2.0 with no GPL code or
  data.

### Changed

- Every render starts from the clean-room power-on RAM image (`CLEAN_POWERON`).
- Depends on `mos6581` 0.2, which has dropped its own `gpl-data` feature and
  uses clean-room models for the filter cutoff and the combined waveforms.

### Added

- `render --poweron FILE` loads a 1024-byte power-on RAM image from a file
  instead of the built-in one.
- `playback` feature: `sink::AudioSink` plays through the default output device
  via `cpal`. Off by default; the default build still depends only on `mos6510`
  and `mos6581`.
- `render --sink wav|audio` picks where samples go. `wav` stays the default and
  is unchanged; `audio` needs the `playback` feature and otherwise exits with an
  error saying so.

### Removed

- `scripts/fidelity-gap.sh`, which compared the default and `gpl-data` builds.
  `scripts/tonality.py` stays.
