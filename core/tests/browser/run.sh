#!/bin/sh
# Browser tests of the photo app (core/web): a drawn library in a temporary
# folder, scanned and served on localhost, driven by Playwright. Safe to run
# anywhere: it touches nothing but its own temporary folder.
#
#   core/tests/browser/run.sh [path-to-shoebox]
#
# Needs a built shoebox binary (default: the host's release build), Python
# with Pillow, and Node with Playwright and a Chromium (CHROME=path to use
# another one; NODE_PATH where Playwright is, default the global modules).
# CI runs it in .github/workflows/guide.yml.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
BIN=${1:-$(cd "$HERE/../../.." && pwd)/core/target/$(rustc -vV | sed -n 's/^host: //p')/release/shoebox}
[ -x "$BIN" ] || { echo "no shoebox binary at $BIN (build it with scripts/build.sh)" >&2; exit 1; }
PORT=${PORT:-7950}
export NODE_PATH=${NODE_PATH:-$(npm root -g)}

WORK=$(mktemp -d "${TMPDIR:-/tmp}/shoebox-browser.XXXXXX")
pid=
cleanup() {
    [ -z "$pid" ] || kill "$pid" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

python3 "$HERE/make-library.py" "$WORK/Photos"
"$BIN" scan "$WORK/Photos" >"$WORK/scan.log" 2>&1 || { cat "$WORK/scan.log" >&2; exit 1; }

url=http://localhost:$PORT/
"$BIN" serve "$WORK/Photos" --port "$PORT" >"$WORK/server.log" 2>&1 & pid=$!
i=0
until curl -s -o /dev/null "$url"; do
    i=$((i + 1))
    if [ $i -gt 300 ] || ! kill -0 "$pid" 2>/dev/null; then
        echo "server at $url did not start:" >&2; cat "$WORK/server.log" >&2; exit 1
    fi
    sleep 0.1
done

node "$HERE/keyboard.mjs" "$url"
