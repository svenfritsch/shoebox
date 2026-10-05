#!/bin/sh
# Linux: run this file (right-click > "Run as a Program", or from a terminal).
# Put it in the drive's top folder, next to .shoebox/.
here=$(cd "$(dirname "$0")" && pwd)
bin=
for candidate in "$here/.shoebox/bin/shoebox-linux" "$here/shoebox-linux"; do
    [ -f "$candidate" ] && bin=$candidate && break
done
if [ -z "$bin" ]; then
    echo "shoebox-linux was not found next to this file or in .shoebox/bin/."
    printf 'Press Return to close. '; read -r _; exit 1
fi
chmod +x "$bin" 2>/dev/null
# Drives are often mounted noexec: run a copy of the program (not of any
# photo) from a temporary folder then.
if ! [ -x "$bin" ] || ! "$bin" --version >/dev/null 2>&1; then
    tmp=$(mktemp -d) && cp "$bin" "$tmp/shoebox" && chmod +x "$tmp/shoebox" && bin=$tmp/shoebox
fi
echo "Starting shoebox. Leave this window open while you use it; close it to stop."
exec "$bin"
