# Fidelity against a reference player: design

Date: 2026-11-12. Status: approved in brainstorming.

## Goal

Close the audible gap between sid-play/mos6581 renders and a reference player without
reintroducing GPL material. Today, tonality differs by more than 0.05 on four of the 17
corpus tunes:

| Tune | Ours | VICE |
|---|---|---|
| Axel_F | 0.34 | 0.61 |
| 05-09-87 | 0.60 | 0.73 |
| All_Terrain_Vehicle_Simulator | 0.70 | 0.81 |
| Alternative_World_Games | 0.70 | 0.76 |

## Reference

VICE 3.10 `vsid` (Homebrew), engine ReSID 6581 (`-sidenginemodel 256`). It renders a WAV
headless with `-console -sounddev wav -soundarg FILE -soundrate 44100 -limitcycles N`.
VICE's engine is reSID (GPL-2.0), so its output is a GPL-derived reference.

## Rule for using the reference (decided by the user)

- **Oracle.** Constants are tuned only clean-room. An isolated author gets PASS/FAIL
  against reference renders, with at most 3 rounds per item. Thresholds are fixed before
  authoring. Values chosen after feedback are labelled feedback-driven. A public-sources
  reviewer checks the result. This is the same method as the GPL-removal round.
- **Bug-hunting.** The controller may compare our renders with the reference directly, to
  find structural bugs: harness timing, state machines, mixing, register semantics. Each
  fix must be justified from public sources or our own reasoning, with a test. A number
  observed in a reference render is never copied into code, tests or comments.
- Reference renders, the oracle tool and its outputs stay in a private scratch directory.
  They are never committed or published. Recorded outcomes in `provenance.md` (evaluator
  numbers after an item closes) are allowed, as before.

## Components

1. **Oracle tool** (private, Python with numpy in a scratch virtualenv).
   - Render the same tune and subtune with both players for a fixed length.
   - Align the two renders: VICE boots first, so find the offset by cross-correlating
     short-time RMS envelopes. Report the offset and the alignment confidence.
   - Metrics per tune:
     - log-spectrogram distance in octave bands (dB RMS over aligned frames)
     - RMS-envelope correlation
     - per-band mean level difference (dB)
     - tonality median, kept for continuity
   - Output: a per-tune table and a corpus summary. It can also render a single probe PSID.
2. **Diagnosis** (controller, bug-hunting rule), for each tune whose gap exceeds the
   corpus median by a clear margin:
   - Locate the divergence: time span, frequency band, and which voice or feature (using
     `--trace` and register logs from our harness).
   - Confirm with a small probe PSID when needed.
   - Classify each finding:
     - **structural bug**: fix directly, with independent justification and a test
     - **constant gap**: queue for clean room, with a probe and a threshold
3. **Clean-room rounds** for the queued constants. The method is as above. Thresholds are
   expressed in oracle-tool metrics on the item's probe and, where relevant, on target
   tunes.
4. **Acceptance for every change.** No corpus tune's spectral distance gets worse by more
   than a small tolerance (0.5 dB). The targeted tunes improve. `provenance.md` and
   HANDSOFF record the change.

## Out of scope

The 8580. Changing the CPU core, except for a structural bug found by diagnosis. Republishing
until a batch of fixes is accepted; publishing still needs the user's go-ahead.

## Order

1. Build the oracle tool.
2. Take a baseline of the 17 tunes.
3. Diagnose the four target tunes.
4. Fix the structural bugs.
5. Run clean-room rounds for the queued constants.
6. Update docs, then ask about a 0.2.1 or 0.3.0 release.
