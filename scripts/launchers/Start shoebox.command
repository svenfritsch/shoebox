#!/bin/sh
# Double-click in Finder to start shoebox. Keep this file next to the program
# "shoebox" (the downloaded folder, wherever you put it: on your computer or on
# a drive). macOS (Gatekeeper) refuses to open a downloaded binary by
# double-click, but a script that starts it is fine.
here=$(cd "$(dirname "$0")" && pwd)
bin=
for candidate in "$here/shoebox" "$here/.shoebox/bin/shoebox-macos" "$here/shoebox-macos"; do
    [ -f "$candidate" ] && bin=$candidate && break
done
if [ -z "$bin" ]; then
    echo "The program shoebox was not found next to this file. Keep the files of the downloaded folder together."
    echo "Press Return to close."; read -r _; exit 1
fi
# Downloads carry a quarantine mark that makes Gatekeeper block the program.
xattr -d com.apple.quarantine "$bin" 2>/dev/null
chmod +x "$bin" 2>/dev/null
echo "Starting shoebox. Leave this window open while you use it; close it to stop."
exec "$bin"
