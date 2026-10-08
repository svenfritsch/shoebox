# Guide (landing page and handbook)

The guide is plain HTML and plain text, one set per language:

| File | What |
|------|------|
| `shoebox-en.html`, `shoebox-de.html` | the guide: inline styles, no scripts, no build step |
| `shoebox-en.txt`, `shoebox-de.txt` | the same text, plain, for the long run (generated from the HTML) |
| `assets/en/`, `assets/de/` | the screenshots, named for what they show (`launcher-overview.jpg`, `ui-overview.jpg`, `multitag-search.jpg`, ...) |

Users find it in the release: `shoebox-macos.tar.gz` unpacks to a folder with
`shoebox-macos`, `Start shoebox.command`, `recognizer/` and `guide/`. Open
`guide/shoebox-en.html` (or `-de`) in any browser; the images sit next to it, so
keep the `assets` folder with the HTML.

## Editing

The guide must match the app: whoever changes what a person can see or do
updates it in the same pull request (see the rules in `CLAUDE.md`).

Edit the text in the two HTML files (keep both in step, and take UI names from
`core/i18n/en.json` / `de.json`), then refresh the text copies and commit them:

```sh
python3 docs/guide/html2txt.py          # rewrite shoebox-en.txt / shoebox-de.txt
python3 docs/guide/html2txt.py --check  # what CI runs: .txt up to date, images exist
```

CI (`.github/workflows/guide.yml`) runs the check on every push or pull request
that touches `docs/guide/`; the release job repeats it and puts the guide into
the tarball. Nothing else is built, so there is no Chrome, font or PDF step.
`html2txt.py` leaves out elements marked `data-txt="skip"`, `svg`, `style` and
`script`.

## Screenshots

Only when the UI changed. They come from a sample library of drawn, synthetic
pictures (a comic girl and a cat, created at /Volumes/Photos) with faces and
pets from a mock recognizer, so no models are needed. Needs a built shoebox
binary, Python with Pillow and numpy, and Node with Playwright and a Chromium.

```sh
CHROME=/path/to/chrome docs/guide/screenshots.sh core/target/release/shoebox
```

`comic.py` draws the pictures, `make-sample-library.py` builds the library,
`mock-recognizer.py` answers the recognizer protocol for it (docs/protocol.md).
`shrink-shots.py` turns the PNGs into the 1800 px JPEGs kept in `assets/`.
