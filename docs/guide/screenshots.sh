#!/bin/sh
# Regenerate docs/guide/shots/{en,de}/*.png from a synthetic sample library
# (no real photos). Needs a built shoebox binary, Python with Pillow and numpy,
# and Node with Playwright and a Chromium.
#   docs/guide/screenshots.sh [path-to-shoebox]
# The sample library is created at /Volumes/Photos so the paths in the
# screenshots look like a drive; this needs permission to create /Volumes.
set -eu
BIN=${1:-core/target/release/shoebox}
BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")
cd "$(dirname "$0")"
LIB=/Volumes/Photos
export NODE_PATH=${NODE_PATH:-$(npm root -g)}
for lang in en de; do
    out=$PWD/shots/$lang; rm -rf "$out"; mkdir -p "$out"
    rm -rf "$LIB" /tmp/shoebox-guide-home; mkdir -p "$LIB" /tmp/shoebox-guide-home
    python3 make-sample-library.py "$LIB" "$lang"
    "$BIN" scan "$LIB" >/dev/null 2>&1
    "$BIN" serve "$LIB" --port 7900 >/dev/null 2>&1 & pid=$!
    sleep 2
    node screenshots.mjs app "$lang" "$out" http://localhost:7900/ "$LIB"
    kill $pid
    HOME=/tmp/shoebox-guide-home "$BIN" >/dev/null 2>&1 & pid=$!
    sleep 2
    node screenshots.mjs launcher "$lang" "$out" http://localhost:7879/ "$LIB"
    kill $pid
done
rm -rf "$LIB" /tmp/shoebox-guide-home
python3 shrink-shots.py
