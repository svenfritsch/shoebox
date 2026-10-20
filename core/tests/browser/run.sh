#!/bin/sh
# Browser tests of the photo app (core/web), driven by Playwright. Safe to run
# anywhere: they touch nothing but their own temporary folder.
#
#   core/tests/browser/run.sh [path-to-shoebox]
#
# faces-mocked.mjs (the Unnamed page, groups and the people list) needs no
# server: a fake API in the test (fake-api.mjs) and a fake clock. The others
# run on drawn libraries, scanned and served on localhost: keyboard.mjs on
# plain photos (make-library.py), faces-e2e.mjs on faces (make-faces.py) that
# the fake recognizer finds, which is built next to shoebox (FAKE=path to use
# another one). Which test goes where: "Browser tests" in docs/plan.md.
# ONLY=text runs only those face tests whose name has the text in it;
# APP_JS=file has their pages run that app.js instead of core/web's (mocked)
# or the one built into shoebox (e2e).
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
# Regroup after two cards or 1.5 s (the app's own rules are 10 cards or 30 s).
export SHOEBOX_REGROUP=${SHOEBOX_REGROUP:-2,1500,300}
export NODE_PATH=${NODE_PATH:-$(npm root -g)}

failed=0
node "$HERE/faces-mocked.mjs" || failed=1

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

node "$HERE/keyboard.mjs" "http://localhost:$PORT/" || failed=1
node "$HERE/faces-e2e.mjs" "http://localhost:$((PORT + 1))/" || failed=1
exit $failed
