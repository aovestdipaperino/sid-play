#!/bin/sh
# Render each tune with the default (GPL-free) build and the gpl-data build and
# print the tonality median of each and the rms-diff (RMS of the sample difference
# between the two renders over full scale). Usage: fidelity-gap.sh tune.sid...
set -eu
cd "$(dirname "$0")/../../.."
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
cargo build --release -q -p sid-play && cp target/release/render "$out/render-default"
cargo build --release -q -p sid-play --features gpl-data && cp target/release/render "$out/render-gpl"
printf '%-36s %8s %8s %10s\n' tune default gpl-data rms-diff
for f in "$@"; do
    n=$(basename "$f" .sid)
    "$out/render-default" "$f" --seconds 10 --out "$out/$n-d.wav" > /dev/null
    "$out/render-gpl" "$f" --seconds 10 --out "$out/$n-g.wav" > /dev/null
    printf '%-36s %8s %8s %10s\n' "$n" \
        "$(python3 -I crates/sid-play/scripts/tonality.py "$out/$n-d.wav")" \
        "$(python3 -I crates/sid-play/scripts/tonality.py "$out/$n-g.wav")" \
        "$(python3 -I crates/sid-play/scripts/tonality.py --rms-diff "$out/$n-d.wav" "$out/$n-g.wav")"
done
