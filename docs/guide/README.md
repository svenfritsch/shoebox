# Guide (landing page and handbook)

`shoebox-guide-en.pdf` and `shoebox-guide-de.pdf` are the product's landing
page and handbook, one PDF per language. They are **not kept in git**: the
`guide` job of `.github/workflows/build.yml` builds them from these sources on
every push and pull request (artifact `shoebox-guide`), and the `release` job
attaches the two PDFs to each release, so every release carries a fresh guide.

Edit the text in `build-guide.py` (English and German side by side: keep both
in step, and take UI names from `core/i18n/en.json` / `de.json`). The
screenshots in `shots/` are committed.

```sh
# the PDFs (needs python3, Chrome/Chromium and the Inter font)
CHROME=/path/to/chrome docs/guide/build.sh

# only when the UI changed: new screenshots. They come from a sample library
# of drawn, synthetic pictures (a comic girl and a cat, created at
# /Volumes/Photos) with faces and pets from a mock recognizer, so no models
# are needed. Needs a built shoebox binary, Python with Pillow and numpy, and
# Node with Playwright and a Chromium.
CHROME=/path/to/chrome docs/guide/screenshots.sh core/target/release/shoebox
```

`comic.py` draws the pictures, `make-sample-library.py` builds the library,
`mock-recognizer.py` answers the recognizer protocol for it (docs/protocol.md).
