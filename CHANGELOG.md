# Changelog

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
