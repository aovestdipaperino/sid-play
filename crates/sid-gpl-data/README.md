# sid-gpl-data

GPL-2.0-only. Measurement data borrowed from other projects, kept in one crate
so the rest of the SID stack can be MIT OR Apache-2.0. Each item will be
replaced by clean-room data; when the crate is empty it will be deleted.

| Item | Origin | Retired by |
|---|---|---|
| `WAVE6581`, `WAVE8580` | reSID (Dag Lem) waveform measurements | clean-room combined-waveform model |
| `CUTOFF_POINTS_6581`, `CUTOFF_POINTS_8580` | reSID `f0_points` | own cutoff measurements or a public model |
| `POWERON` | libsidplayfp `poweron.bin` | public C64 power-on documentation, or removal |
| chip constants (`ENV_*`, `EXTFILT_*`, `FILTER_Q_*`, `MIXER_DC`, `VOICE_*`, `NOISE_*`) | reSID (Dag Lem), see `docs/provenance.md` | own measurements or public datasheet values |
