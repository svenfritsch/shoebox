#!/usr/bin/env bash
# Lay out the folder a person downloads, the same on every system
# (docs/plan.md, "The release folder"):
#
#   shoebox-<system>/
#     Start shoebox.command | Start shoebox.bat | Start shoebox.sh
#     guide/                 (left out when no guide folder is given)
#     program/               the program, its two libraries, recognizer/,
#                            sources/, LICENSE.txt, THIRD-PARTY-LICENSES.txt
#
#   scripts/package.sh <macos|windows|linux> <build dir> <guide dir or ""> <out dir>
#
# <build dir> holds what the build jobs upload: the program (shoebox-macos,
# shoebox.exe, shoebox-linux) and its two libraries. The folder is made as
# <out dir>/shoebox-<system>/; packing it (tar.gz, zip) is the caller's job.
# Needs the network once for the library sources (build-deps.sh --sources).
set -euo pipefail

system=${1:?usage: package.sh <macos|windows|linux> <build dir> <guide dir or ""> <out dir>}
build=${2:?build dir}
guide=${3-}
out=${4:?out dir}
here=$(cd "$(dirname "$0")/.." && pwd)

case "$system" in
    macos)   exe=shoebox-macos; program=shoebox;     libs="libheif.1.dylib libde265.0.dylib"; start="Start shoebox.command"; scripts="install.sh fetch-models.sh" ;;
    windows) exe=shoebox.exe;   program=shoebox.exe; libs="heif.dll libde265.dll";            start="Start shoebox.bat";     scripts="install.ps1 fetch-models.ps1" ;;
    linux)   exe=shoebox-linux; program=shoebox;     libs="libheif.so.1 libde265.so.0";       start="Start shoebox.sh";      scripts="install.sh fetch-models.sh" ;;
    *) echo "unknown system: $system" >&2; exit 1 ;;
esac

top="$out/shoebox-$system"
prog="$top/program"
rm -rf "$top"
mkdir -p "$prog/recognizer"

# The program is just "shoebox" (shoebox.exe) inside its own folder, with
# libheif and libde265 next to it (LGPL: replaceable) and their unmodified
# sources.
cp "$build/$exe" "$prog/$program"
for lib in $libs; do cp "$build/$lib" "$prog/"; done
"$here/scripts/build-deps.sh" --sources "$prog/sources"
cp "$here/LICENSE.txt" "$here/THIRD-PARTY-LICENSES.txt" "$prog/"
for f in README.md recognizer.py $scripts; do cp "$here/recognizer/$f" "$prog/recognizer/"; done
if [ "$system" = windows ]; then
    # cmd.exe misreads a batch file with Unix line ends; git keeps it with LF.
    sed 's/\r*$/\r/' "$here/scripts/launchers/$start" > "$top/$start"
else
    cp "$here/scripts/launchers/$start" "$top/"
fi
if [ "$system" != windows ]; then
    chmod +x "$prog/$program" "$top/$start"
    for f in $scripts; do chmod +x "$prog/recognizer/$f"; done
fi
if [ -n "$guide" ]; then
    cp -R "$guide" "$top/guide"
fi

# Nothing else on top.
expected="$start|${guide:+guide|}program|"
actual=$(ls -A "$top" | LC_ALL=C sort | tr '\n' '|')
if [ "$actual" != "$expected" ]; then
    echo "on top of $top: $actual, expected $expected" >&2
    exit 1
fi
echo "packed $top"
