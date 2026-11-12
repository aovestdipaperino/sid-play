# Changelog

## 0.2.0 (2026-11-12)

### Breaking

- Removed the `gpl-data` feature and the optional `sid-gpl-data` dependency.
  There is now a single build, licensed MIT OR Apache-2.0 with no GPL code or
  data.

### Changed

- Every render starts from the clean-room power-on RAM image
  (`sid_play::poweron::POWERON` is now `CLEAN_POWERON`).
- Depends on `mos6581` 0.2, which has dropped its own `gpl-data` feature and
  uses clean-room models for the filter cutoff and the combined waveforms.

### Added

- `render --poweron FILE` loads a 1024-byte power-on RAM image from a file
  instead of the built-in one.

### Removed

- `scripts/fidelity-gap.sh`, which compared the default and `gpl-data` builds.
  `scripts/tonality.py` stays.
