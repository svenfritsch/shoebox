#!/bin/sh
# Build docs/guide/shoebox-guide-en.pdf and -de.pdf from build-guide.py and
# the screenshots in shots/ (regenerate those with screenshots.sh when the UI
# changed). Needs Python 3, Node with Playwright and a Chromium, and the Inter
# font installed. The release workflow attaches the two PDFs to a release.
set -eu
cd "$(dirname "$0")"
export NODE_PATH=${NODE_PATH:-$(npm root -g)}
python3 build-guide.py
node pdf.mjs en de
ls -l shoebox-guide-*.pdf
