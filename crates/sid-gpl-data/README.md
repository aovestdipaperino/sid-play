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

## Versioning and publishing

Removing an item from this crate is a breaking change for `mos6581` and
`sid-play` built with `gpl-data`. It ships as a 0.x minor bump of this crate
together with a `mos6581` bump that stops reading the item. Publish in
dependency order: `sid-gpl-data`, then `mos6581`, then `sid-play`.

## NOTICE / upstream copyright

The data in this crate comes from the files below. Their copyright header
comments are reproduced verbatim; only the header comments were read when
recording them.

### reSID

Repository <https://github.com/libsidplayfp/resid>, commit
`81055e69908086292b65244ecbeba49124a36513` (the reSID 0.16 import, 2008-03-30).
The same header, byte for byte, heads every file the data came from:
`envelope.h`, `envelope.cc` (`ENV_*`), `extfilt.cc` (`EXTFILT_*`), `filter.cc`
(`FILTER_Q_*`, `MIXER_DC`, `CUTOFF_POINTS_*`), `voice.cc` (`VOICE_*`), `wave.h`,
`wave.cc` (`NOISE_*`), and the waveform sources `wave6581__ST.cc`,
`wave6581_P_T.cc`, `wave6581_PS_.cc`, `wave6581_PST.cc`, `wave8580__ST.cc`,
`wave8580_P_T.cc`, `wave8580_PS_.cc`, `wave8580_PST.cc` (`WAVE6581`, `WAVE8580`;
the matching `.dat` files are raw bytes with no header, and `samp2src.pl`, which
turns them into the `.cc` files, carries the same notice).

```
//  ---------------------------------------------------------------------------
//  This file is part of reSID, a MOS6581 SID emulator engine.
//  Copyright (C) 2004  Dag Lem <resid@nimrod.no>
//
//  This program is free software; you can redistribute it and/or modify
//  it under the terms of the GNU General Public License as published by
//  the Free Software Foundation; either version 2 of the License, or
//  (at your option) any later version.
//
//  This program is distributed in the hope that it will be useful,
//  but WITHOUT ANY WARRANTY; without even the implied warranty of
//  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//  GNU General Public License for more details.
//
//  You should have received a copy of the GNU General Public License
//  along with this program; if not, write to the Free Software
//  Foundation, Inc., 59 Temple Place, Suite 330, Boston, MA  02111-1307  USA
//  ---------------------------------------------------------------------------
```

### libsidplayfp

Repository <https://github.com/libsidplayfp/libsidplayfp>, tag `v3.1.2`
(commit `b1e8f1deedf41d343123319237c0d008d72cbea7`). `POWERON` is decoded from
`src/poweron.bin` (last changed in commit
`d47cca8b8fd61df9b19adf764bfe6e5a3959af35`); decoding that file at `v3.1.2`
with `tools/gen_poweron.py` reproduces `POWERON` byte for byte. `poweron.bin`
has no header of its own: it is a C array fragment included by
`src/psiddrv.cpp`, whose header is:

```
/*
 * This file is part of libsidplayfp, a SID player engine.
 *
 * Copyright 2011-2015 Leandro Nini <drfiemost@users.sourceforge.net>
 * Copyright 2007-2010 Antti Lankila
 * Copyright 2001 Simon White
 *
 * This program is free software; you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation; either version 2 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301, USA.
 */
```

Both upstream notices grant "version 2 of the License, or (at your option) any
later version". This crate is distributed under GPL-2.0-only, which is one of
the options those notices allow.
