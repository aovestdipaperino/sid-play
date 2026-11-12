# Changelog

## 0.3.1 - 2026-11-13

### Added

- RSID files are accepted. They share the PSID header and always have play
  address 0, so they run on the existing interrupt-driven path.
- A minimal VIC-II raster interrupt: $D011 bit 7 and $D012 (beam position and
  compare line), the $D019 latch and the $D01A enable, mirrored through
  $D000-$D3FF. It shares the IRQ input with CIA 1, so a tune that silences
  CIA 1 and drives itself from a raster IRQ now plays instead of staying silent.
- CIA 2 timer A, wired to NMI, with the KERNAL's NMI entry ($FFFA leads to
  `SEI / JMP ($0318)`) and a default $0318 handler that acknowledges CIA 2.
  Tunes that pace sample playback with NMIs now get them.

Effect: `Tetris` (RSID, raster IRQ plus NMI digi drums) plays; the rest of the
corpus renders byte-identical.

## 0.3.0 - 2026-11-13

Builds on mos6581 0.3. The sound changes in this release come from that
crate's clean-room DC model and output stage; this crate's own changes are the
harness fixes below.

### Fixed (PSID loader)

- Songs past 32 take their CIA/VBL speed from bit 31 of the speed field, as the
  PSID format says. Before, `--subsong 32` and above overflowed the shift (a
  panic in debug builds).

### Fixed (harness timing and interrupt model)

- CIA-speed songs now run on timer A in the KERNAL post-reset state, so latch writes change the tempo. `Axel_F` had played 9% fast.
- VBL songs run on the VIC-II frame: 312 x 63 = 19656 cycles PAL, 263 x 65 = 17095 NTSC (6567R8). Before, the period was clock/50, about 0.25% slow.
- A KERNAL-like IRQ chain: $FFFE saves registers and jumps through ($0314); the default handler acknowledges $DC0D and returns.
- A non-zero play address is always driven by the harness, and the KERNAL timer A runs for every song.
- Header length is 0x76 for v1 and 0x7C for v2 and later.
- The CIA CRA force-load strobe is honoured.

Effect: tempo and timing now follow the hardware. The median spectral distance to the reference player fell from 5.9 dB to 4.5 dB on these fixes alone (see `HANDSOFF.md`). Known stand-in: VBL songs with play=0 are driven only by the KERNAL's CIA 1 timer A interrupt (60 Hz) rather than a 50 Hz raster IRQ, since there is no VIC-II. That works only for tunes that leave the CIA interrupt enabled; a tune that writes $7F to $DC0D and uses raster IRQs ($D01A/$D012) gets no interrupts and plays silent.

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
