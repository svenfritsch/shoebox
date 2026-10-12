#!/bin/sh
# Make the guide's screenshots (docs/guide/assets/{en,de}/*.jpg) from a drawn
# sample library (no real photos).
#
# Runs ONLY in the pipeline (.github/workflows/guide.yml, on a GitHub Actions
# runner): it creates and deletes folders where drives live (/Volumes/Photos,
# /mnt/Extreme SSD, /mnt/Backup Drive), because shoebox reads a drive's name
# from its path. On a real computer those can be real drives with real
# photos, so the script refuses to run anywhere else, and never on a Mac.
#
#   docs/guide/screenshots.sh [path-to-shoebox]
#
# Needs a built shoebox binary (default: the host's release build), Python
# with Pillow and numpy, and Node with Playwright and a Chromium (CHROME=path
# to use another one).
#
# Environment:
#   LANGS="en de"                         languages to make
#   PASSES="app launcher drives suggestions"  passes to run (see screenshots.mjs)
#   OUT=docs/guide/assets                 where the JPEGs go; a pass replaces
#                                         only its own files
set -eu
if [ "${GITHUB_ACTIONS:-}" != true ] || [ "$(uname)" = Darwin ]; then
    echo "screenshots.sh runs only in CI (.github/workflows/guide.yml): it creates and" >&2
    echo "deletes folders where real drives live. Push, and take the screenshots from" >&2
    echo "the run's artifact \"guide\"." >&2
    exit 1
fi
HERE=$(cd "$(dirname "$0")" && pwd)
BIN=${1:-$(cd "$HERE/../.." && pwd)/core/target/$(rustc -vV | sed -n 's/^host: //p')/release/shoebox}
[ -x "$BIN" ] || { echo "no shoebox binary at $BIN (build it with scripts/build.sh)" >&2; exit 1; }
BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")
LANGS=${LANGS:-en de}
PASSES=${PASSES:-app launcher drives suggestions}
OUT=${OUT:-$HERE/assets}
LIB=/Volumes/Photos
# Where Linux keeps the drives (shoebox reads the disk name from the path).
DRIVES=/mnt
ORIG="$DRIVES/Extreme SSD/Photos"; BAK="$DRIVES/Backup Drive/Photos Backup"
export NODE_PATH=${NODE_PATH:-$(npm root -g)}

# These folders are made and deleted again. Even on a runner, never touch one
# that is there before we start: it is not ours.
for d in "$LIB" "$DRIVES/Extreme SSD" "$DRIVES/Backup Drive"; do
    if [ -e "$d" ]; then
        echo "$d exists (a real drive, or left over from a run that was killed?)." >&2
        echo "Not touching it; remove or unmount it yourself first." >&2
        exit 1
    fi
done

mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
WORK=$(mktemp -d "${TMPDIR:-/tmp}/shoebox-guide.XXXXXX")
pid=
# A step that fails must not leave a server running (it would hold its port).
cleanup() {
    [ -z "$pid" ] || kill "$pid" 2>/dev/null || true
    rm -rf "$WORK" "$LIB" "$DRIVES/Extreme SSD" "$DRIVES/Backup Drive"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

log() { printf '%s  %s\n' "$(date +%H:%M:%S)" "$*"; }

# Start a server in the background and wait until it answers (not a fixed sleep).
start() { # url command...
    url=$1; shift
    "$@" >"$WORK/server.log" 2>&1 & pid=$!
    i=0
    until curl -s -o /dev/null "$url"; do
        i=$((i + 1))
        if [ $i -gt 300 ] || ! kill -0 "$pid" 2>/dev/null; then
            echo "server at $url did not start:" >&2; cat "$WORK/server.log" >&2; exit 1
        fi
        sleep 0.1
    done
}
stop() { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; pid=; }

# A fresh copy of the sample library at $1 (dates kept), scanned; with
# "recognise", also faces and pets from the mock recognizer (no models needed).
library() { # path [recognise]
    rm -rf "$1"; mkdir -p "$(dirname "$1")"
    cp -Rp "$WORK/sample" "$1"
    "$BIN" scan "$1" >/dev/null 2>&1
    if [ "${2:-}" = recognise ]; then
        SHOEBOX_MOCK_ANN="$WORK/ann.json" "$BIN" recognize "$1" --recognizer "$HERE/mock-recognizer.py" --pets >/dev/null 2>&1
    fi
}

shots() { # mode url library-path
    node "$HERE/screenshots.mjs" "$1" "$lang" "$png" "$2" "$3"
}

for lang in $LANGS; do
    png=$WORK/png/$lang; mkdir -p "$png"
    log "$lang: drawing the sample library"
    rm -rf "$WORK/sample"
    python3 "$HERE/make-sample-library.py" "$WORK/sample" "$lang" "$WORK/ann.json" >/dev/null
    for pass in $PASSES; do
        log "$lang: $pass"
        case $pass in
        app|suggestions)
            # Both change the library (names, tags, trash), so each gets a fresh one.
            library "$LIB" recognise
            start http://localhost:7900/ "$BIN" serve "$LIB" --port 7900
            shots "$pass" http://localhost:7900/ "$LIB"
            stop ;;
        launcher)
            library "$LIB"
            mkdir -p "$WORK/home"
            start http://localhost:7879/ env HOME="$WORK/home" "$BIN"
            shots launcher http://localhost:7879/ "$LIB"
            stop ;;
        drives)
            # Two drives, an original and its backup, for Manage drives and the
            # comparison views. The disk names come from the path, so the drives
            # sit where Linux keeps drives.
            rm -rf "$DRIVES/Extreme SSD" "$DRIVES/Backup Drive"
            mkdir -p "$(dirname "$ORIG")"; cp -Rp "$WORK/sample" "$ORIG"
            python3 "$HERE/make-drives-pair.py" "$ORIG" "$BAK" >/dev/null
            "$BIN" scan "$ORIG" >/dev/null 2>&1
            "$BIN" scan "$BAK" >/dev/null 2>&1
            start http://localhost:7901/ "$BIN" serve "$ORIG" "$BAK" --port 7901
            shots drives http://localhost:7901/ "$ORIG"
            stop
            rm -rf "$DRIVES/Extreme SSD" "$DRIVES/Backup Drive" ;;
        *) echo "unknown pass: $pass (app, launcher, drives, suggestions)" >&2; exit 1 ;;
        esac
    done
    # All passes: start from an empty folder, so a screenshot nobody makes any
    # more goes away. Some passes: replace only what they made.
    if [ "$PASSES" = "app launcher drives suggestions" ]; then rm -rf "${OUT:?}/$lang"; fi
    mkdir -p "$OUT/$lang"
    python3 "$HERE/shrink-shots.py" "$png" "$OUT/$lang"
done
log "done: $OUT"
