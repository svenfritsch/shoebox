#!/bin/sh
# Browser tests of the photo app (core/web): drawn libraries in a temporary
# folder, scanned and served on localhost, driven by Playwright. Safe to run
# anywhere: it touches nothing but its own temporary folder.
#
#   core/tests/browser/run.sh [path-to-shoebox]
#
# keyboard.mjs runs on a library of plain photos (make-library.py);
# unnamed.mjs and people.mjs (the Unnamed page, groups and the people list) on
# one with faces (make-faces.py), found by the fake recognizer that is built
# next to shoebox (FAKE=path to use another one). ONLY=text runs only those
# tests of unnamed.mjs and people.mjs whose name has the text in it;
# APP_JS=file has their pages run that app.js instead of the one built into
# shoebox.
#
# Needs a built shoebox binary (default: the host's release build), Python
# with Pillow, and Node with Playwright and a Chromium (CHROME=path to use
# another one; NODE_PATH where Playwright is, default the global modules).
# CI runs it in .github/workflows/guide.yml.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
BIN=${1:-$(cd "$HERE/../../.." && pwd)/core/target/$(rustc -vV | sed -n 's/^host: //p')/release/shoebox}
[ -x "$BIN" ] || { echo "no shoebox binary at $BIN (build it with scripts/build.sh)" >&2; exit 1; }
FAKE=${FAKE:-$(dirname "$BIN")/shoebox-fake-recognizer}
[ -x "$FAKE" ] || { echo "no fake recognizer at $FAKE (scripts/build.sh builds it next to shoebox)" >&2; exit 1; }
PORT=${PORT:-7950}
export NODE_PATH=${NODE_PATH:-$(npm root -g)}

WORK=$(mktemp -d "${TMPDIR:-/tmp}/shoebox-browser.XXXXXX")
pids=
cleanup() {
    for p in $pids; do kill "$p" 2>/dev/null || true; done
    rm -rf "$WORK"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

# serve <library> <port>: in the background, until it answers.
serve() {
    "$BIN" serve "$1" --port "$2" >"$1.server.log" 2>&1 &
    pid=$!
    pids="$pids $pid"
    i=0
    until curl -s -o /dev/null "http://localhost:$2/"; do
        i=$((i + 1))
        if [ $i -gt 300 ] || ! kill -0 "$pid" 2>/dev/null; then
            echo "server on port $2 did not start:" >&2; cat "$1.server.log" >&2; exit 1
        fi
        sleep 0.1
    done
}

python3 "$HERE/make-library.py" "$WORK/Photos"
"$BIN" scan "$WORK/Photos" >"$WORK/scan.log" 2>&1 || { cat "$WORK/scan.log" >&2; exit 1; }

python3 "$HERE/make-faces.py" "$WORK/Faces"
{ "$BIN" scan "$WORK/Faces" && "$BIN" recognize "$WORK/Faces" --recognizer "$FAKE"; } >"$WORK/faces.log" 2>&1 \
    || { cat "$WORK/faces.log" >&2; exit 1; }

serve "$WORK/Photos" "$PORT"
serve "$WORK/Faces" $((PORT + 1))

failed=0
node "$HERE/keyboard.mjs" "http://localhost:$PORT/" || failed=1
for t in unnamed people; do
    node "$HERE/$t.mjs" "http://localhost:$((PORT + 1))/" || failed=1
done
exit $failed
