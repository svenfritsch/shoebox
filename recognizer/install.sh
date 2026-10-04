#!/bin/sh
# Install the recognizer into a library's .shoebox/ folder, with a standalone
# Python (python-build-standalone), OpenCV, numpy and the models, so
# `shoebox recognize` finds it without anything installed on the computer.
#
#   recognizer/install.sh <library-root>
#
# Run it on each kind of computer that will run recognition (Intel Mac,
# Apple Silicon Mac, Linux): the runtime goes to runtime/<os>-<arch>/, and pip
# picks the OpenCV and numpy builds this computer's system can load.
#
# Everything is assembled in a temporary folder on this computer and then
# copied with symlinks resolved, as exFAT has none.
set -eu

ROOT=${1:?usage: install.sh <library-root>}
HERE=$(cd "$(dirname "$0")" && pwd)
DEST="$ROOT/.shoebox/recognizer"
PBS_TAG=20251014
PY=3.12.12

case "$(uname -s)" in
    Darwin) os=macos vendor=apple-darwin ;;
    Linux) os=linux vendor=unknown-linux-gnu ;;
    *) echo "unsupported system: $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
    x86_64) arch=x86_64 ;;
    arm64 | aarch64) arch=aarch64 ;;
    *) echo "unsupported CPU: $(uname -m)" >&2; exit 1 ;;
esac
# Must match Rust's std::env::consts::{OS, ARCH} (see core/src/recognize.rs).
PLATFORM=$os-$arch
URL="https://github.com/astral-sh/python-build-standalone/releases/download/$PBS_TAG/cpython-$PY+$PBS_TAG-$arch-$vendor-install_only_stripped.tar.gz"

[ -d "$ROOT/.shoebox" ] || { echo "$ROOT has no .shoebox folder (run shoebox scan first)" >&2; exit 1; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "Python $PY for ${PLATFORM}…"
curl -fsSL -o "$TMP/python.tar.gz" "$URL"
tar -xzf "$TMP/python.tar.gz" -C "$TMP"
PYTHON="$TMP/python/bin/python3"

echo "OpenCV and numpy…"
# Ready-made wheels only: pip then takes the newest release built for this
# macOS (OpenCV 4.11+ needs macOS 13 on Intel, so macOS 12 gets 4.10) instead
# of compiling OpenCV from source, which takes hours and usually fails.
"$PYTHON" -m pip install --no-cache-dir --disable-pip-version-check --progress-bar on \
    --only-binary :all: opencv-python-headless numpy
"$PYTHON" -c 'import cv2, numpy; print("  OpenCV", cv2.__version__, "numpy", numpy.__version__)'

echo "Models…"
"$HERE/fetch-models.sh" "$TMP/models"

echo "Copying to ${DEST}…"
mkdir -p "$DEST/runtime"
rm -rf "$DEST/runtime/$PLATFORM"
# Only what running recognizer.py needs: copies of symlinks would double the
# interpreter, and Tk, IDLE and the headers are of no use here.
P="$TMP/python"
find "$P" -name __pycache__ -type d -prune -exec rm -rf {} +
( cd "$P/bin" && cp -L python3 .python3 && rm -f -- * && mv .python3 python3 )
if [ -L "$P/lib/libpython3.12.so" ]; then rm -f "$P/lib/libpython3.12.so"; fi
rm -rf "$P/include" "$P/share" "$P/lib/pkgconfig" "$P"/lib/tcl* "$P"/lib/tk* "$P"/lib/itcl* \
    "$P"/lib/thread* "$P"/lib/libtcl* "$P"/lib/libtk* \
    "$P/lib/python3.12/idlelib" "$P/lib/python3.12/tkinter" "$P/lib/python3.12/turtledemo" \
    "$P/lib/python3.12/ensurepip" "$P/lib/python3.12/lib-dynload/_tkinter"*
cp -RL "$TMP/python" "$DEST/runtime/$PLATFORM"
mkdir -p "$DEST/models"
cp "$TMP/models/"*.onnx "$DEST/models/"
cp "$HERE/recognizer.py" "$DEST/recognizer.py"

echo "Checking…"
HELLO=$(printf '' | "$DEST/runtime/$PLATFORM/bin/python3" "$DEST/recognizer.py" 2>/dev/null | head -n 1)
case "$HELLO" in
    *shoebox-recognizer*) echo "  $HELLO" ;;
    *) echo "the recognizer did not start" >&2; exit 1 ;;
esac
echo "Done. Run: shoebox recognize \"$ROOT\""
