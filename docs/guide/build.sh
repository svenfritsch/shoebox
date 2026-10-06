#!/bin/sh
# Build shoebox-guide-en.pdf and shoebox-guide-de.pdf from build-guide.py and
# the screenshots in shots/. The release workflow runs this on every release
# and attaches the PDFs; the PDFs are not kept in git.
# Needs python3, a Chrome or Chromium (CHROME=/path, else found on PATH) and
# the Inter font (Ubuntu: fonts-inter). Regenerate shots/ with screenshots.sh
# when the UI changed.
set -eu
cd "$(dirname "$0")"
CHROME=${CHROME:-$(command -v google-chrome || command -v chromium || command -v chromium-browser || true)}
[ -n "$CHROME" ] || { echo "no Chrome or Chromium found (set CHROME)"; exit 1; }
python3 build-guide.py
for lang in en de; do
    "$CHROME" --headless --no-sandbox --disable-gpu --no-pdf-header-footer \
        --run-all-compositor-stages-before-draw --virtual-time-budget=20000 \
        --print-to-pdf="$PWD/shoebox-guide-$lang.pdf" "file://$PWD/_html/$lang.html" 2>/dev/null
done
ls -l shoebox-guide-en.pdf shoebox-guide-de.pdf
