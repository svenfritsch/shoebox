#!/bin/sh
# Double-click in Finder to install face recognition (about 330 MB, needs the
# internet). It goes into the recognizer folder next to this file, so every
# drive you open with shoebox on this computer can use it; nothing is put on
# the drives. Run it once per computer.
here=$(cd "$(dirname "$0")" && pwd)
# Downloads carry a quarantine mark; the scripts and the Python inside would be blocked.
xattr -dr com.apple.quarantine "$here/recognizer" 2>/dev/null
echo "Installing face recognition. This takes a few minutes; leave this window open."
echo
if sh "$here/recognizer/install.sh"; then
    echo
    echo "Done. Close this window and use Recognize in the shoebox launcher."
else
    echo
    echo "Something went wrong (see above). Check the internet connection and try again."
fi
echo "Press Return to close."; read -r _
