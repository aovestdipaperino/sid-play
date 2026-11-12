# sid-play

Plays C64 PSID tunes through the [`mos6581`](../mos6581) SID emulator. The 6510
code runs on the cycle-exact [`mos6510`](https://crates.io/crates/mos6510) crate
and the chip output is written to a WAV file. RSID files are not supported.

Licence: MIT OR Apache-2.0.

## Usage

```sh
cargo run --release -p sid-play --bin render -- tune.sid --seconds 20 --out out.wav
```

`--subsong N` selects a subsong (0-based). `--trace` prints diagnostics about
where the tune ran, what it read and which chip registers it wrote. `--ntsc`
uses the 60 Hz frame clock.

## Fidelity and licence

The default build is GPL-free. It uses AND-approximated combined waveforms, a
linear 30 Hz to 12 kHz filter cutoff, clean-room chip constants and an all-zero
power-on RAM image. See the "Clean-room defaults" and "Known default-build
fidelity gaps" sections of [`docs/provenance.md`](../../docs/provenance.md).

The closed filter corner is the biggest audible difference: the scale fixture
renders at rms 53 by default and 1362 with `gpl-data`, about 25x quieter.

Building with `--features gpl-data` uses measured reSID tables and the
libsidplayfp power-on pattern from the `sid-gpl-data` crate. A build with that
feature is GPL-2.0.

Libraries should never enable `gpl-data`; leave the choice to the final binary. Cargo unifies features, so one dependency enabling it makes the whole program GPL-2.0.

## Fidelity gap on real tunes

`scripts/fidelity-gap.sh tune.sid...` (in the workspace checkout only; it is
not shipped in the published crate) renders each tune for 10 s with both builds
and prints the tonality (median autocorrelation peak; 0.00 is silence, higher is
more tonal) of each, plus the RMS of the difference between the two renders as a
fraction of full scale.

```
tune                                  default gpl-data   rms-diff
05-09-87                                 0.60     0.62     0.0748
4_Soccer_Sims_Soccer_Skills              0.66     0.66     0.0271
5_Title_Tunes                            0.73     0.71     0.0300
64_Dwarfs_on_XTC                         0.84     0.89     0.0783
720_Degrees                              0.87     0.84     0.0521
Acceleration-mix                         0.44     0.60     0.2531
ACE_II                                   0.65     0.66     0.0838
Action_Biker                             0.79     0.78     0.0389
All_Points_Bulletin                      0.77     0.74     0.0397
All_Terrain_Vehicle_Simulator            0.83     0.69     0.0623
Alternative_World_Games                  0.70     0.70     0.0101
Antics                                   0.85     0.87     0.0189
Ark_Pandora                              0.51     0.51     0.0352
Artura                                   0.81     0.80     0.0184
Athena                                   0.62     0.62     0.0129
Axel_F                                   0.34     0.40     0.0402
last_ninja                               0.58     0.58     0.0046
irq                                      0.86     0.85     0.0308
scale                                    0.82     0.71     0.0417
```

No tune is silent in the default build. The lowest default tonality is `Axel_F` at 0.34 (0.40 with `gpl-data`), then `Acceleration-mix` at 0.44 (0.60 with `gpl-data`); no tune is at 0.00 or below 0.20. `Acceleration-mix` also differs most from its `gpl-data` render (rms-diff 0.2531). Tonality shows that a tune plays, not that it sounds right; the rms-diff column shows how far the default build moves the waveform.

