# rust-sid

A MOS 6581 SID chip emulator in Rust, one update per Φ2 cycle, split into three crates:

- [`mos6581`](crates/mos6581): the chip. MIT OR Apache-2.0.
- [`sid-play`](crates/sid-play): a PSID player and `render` binary. MIT OR Apache-2.0.
- [`sid-gpl-data`](crates/sid-gpl-data): measured reSID and libsidplayfp data. GPL-2.0-only,
  used only when you build with `--features gpl-data`, which makes that build GPL-2.0.

The default build is GPL-free and approximates the combined waveforms and the filter
cutoff; see `docs/provenance.md` and `HANDSOFF.md` for the audit and the fidelity gap.
