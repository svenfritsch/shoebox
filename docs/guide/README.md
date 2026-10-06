# Guide (landing page and handbook)

`shoebox-guide-en.pdf` and `shoebox-guide-de.pdf` are the product's landing
page and handbook, one PDF per language. They are committed here and the
release workflow attaches both to every release (`.github/workflows/build.yml`,
job `release`).

Edit the text in `build-guide.py` (English and German side by side: keep both
in step, and take UI names from `core/i18n/en.json` / `de.json`).

```sh
# 1. only when the UI changed: new screenshots (sample library with synthetic
#    photos, created at /Volumes/Photos; needs a built shoebox binary,
#    Python with Pillow and numpy, Node with Playwright and a Chromium)
CHROME=/path/to/chrome docs/guide/screenshots.sh core/target/release/shoebox
# 2. the PDFs (also needs the Inter font installed)
CHROME=/path/to/chrome docs/guide/build.sh
# 3. commit shots/, the PDFs and whatever else changed
```

`make-sample-library.py` makes the sample photos (no real people or photos are
in the repo). Not in the screenshots: people and pets, which need the
recognizer and its models.
