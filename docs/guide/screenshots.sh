#!/bin/sh
# Regenerate docs/guide/assets/{en,de}/*.png from a synthetic sample library
# (no real photos). Needs a built shoebox binary, Python with Pillow and numpy,
# and Node with Playwright and a Chromium.
#   docs/guide/screenshots.sh [path-to-shoebox]
# The sample library is created at /Volumes/Photos so the paths in the
# screenshots look like a drive; this needs permission to create /Volumes.
set -eu
# A step that fails must not leave a server running (it would hold its port).
pid=
trap '[ -z "$pid" ] || kill $pid 2>/dev/null || true' EXIT
BIN=${1:-core/target/release/shoebox}
BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")
cd "$(dirname "$0")"
LIB=/Volumes/Photos
# Where this system keeps the drives (the disk name is read from the path).
if [ "$(uname)" = Darwin ]; then DRIVES=/Volumes; else DRIVES=/mnt; fi
export NODE_PATH=${NODE_PATH:-$(npm root -g)}
for lang in en de; do
    out=$PWD/assets/$lang; rm -rf "$out"; mkdir -p "$out"
    rm -rf "$LIB" /tmp/shoebox-guide-home; mkdir -p "$LIB" /tmp/shoebox-guide-home
    python3 make-sample-library.py "$LIB" "$lang" /tmp/shoebox-guide-ann.json
    "$BIN" scan "$LIB" >/dev/null 2>&1
    # faces and cats from the mock recognizer (no models needed)
    SHOEBOX_MOCK_ANN=/tmp/shoebox-guide-ann.json "$BIN" recognize "$LIB" --recognizer "$PWD/mock-recognizer.py" --pets >/dev/null 2>&1
    "$BIN" serve "$LIB" --port 7900 >/dev/null 2>&1 & pid=$!
    sleep 2
    node screenshots.mjs app "$lang" "$out" http://localhost:7900/ "$LIB"
    kill $pid
    # The suggestion lists (tag field, name field) want a fresh library.
    rm -rf "$LIB"; mkdir -p "$LIB"
    python3 make-sample-library.py "$LIB" "$lang" /tmp/shoebox-guide-ann.json
    "$BIN" scan "$LIB" >/dev/null 2>&1
    SHOEBOX_MOCK_ANN=/tmp/shoebox-guide-ann.json "$BIN" recognize "$LIB" --recognizer "$PWD/mock-recognizer.py" --pets >/dev/null 2>&1
    "$BIN" serve "$LIB" --port 7900 >/dev/null 2>&1 & pid=$!
    sleep 2
    node screenshots.mjs suggestions "$lang" "$out" http://localhost:7900/ "$LIB"
    kill $pid
    HOME=/tmp/shoebox-guide-home "$BIN" >/dev/null 2>&1 & pid=$!
    sleep 2
    node screenshots.mjs launcher "$lang" "$out" http://localhost:7879/ "$LIB"
    kill $pid
    # Two drives, an original and its backup, for Manage drives and the
    # comparison views. The disk names come from the path, so the drives sit
    # where this system looks for drives.
    ORIG="$DRIVES/Extreme SSD/Photos"; BAK="$DRIVES/Backup Drive/Photos Backup"
    rm -rf "$DRIVES/Extreme SSD" "$DRIVES/Backup Drive"; mkdir -p "$ORIG" "$BAK"
    python3 make-sample-library.py "$ORIG" "$lang" /tmp/shoebox-guide-ann2.json
    python3 make-drives-pair.py "$ORIG" "$BAK"
    "$BIN" scan "$ORIG" >/dev/null 2>&1
    "$BIN" scan "$BAK" >/dev/null 2>&1
    "$BIN" serve "$ORIG" "$BAK" --port 7901 >/dev/null 2>&1 & pid=$!
    sleep 2
    node screenshots.mjs drives "$lang" "$out" http://localhost:7901/ "$ORIG"
    kill $pid
    rm -rf "$DRIVES/Extreme SSD" "$DRIVES/Backup Drive"
done
rm -rf "$LIB" /tmp/shoebox-guide-home
python3 shrink-shots.py
