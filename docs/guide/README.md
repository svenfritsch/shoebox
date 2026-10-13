# Guide (landing page and handbook)

The guide is plain HTML, one file per language, written by hand:

| File | What | In git |
|------|------|--------|
| `shoebox-en.html`, `shoebox-de.html` | the guide: inline styles, no scripts, no build step | yes |
| `shoebox-en.txt`, `shoebox-de.txt` | the same text, plain, for the long run (`html2txt.py`) | no, made by CI |
| `assets/en/`, `assets/de/` | the screenshots, named for what they show (`ui-overview.jpg`, `launcher-overview.jpg`, ...) | no, made by CI |

Users find it in the release: `shoebox-macos.tar.gz` unpacks to a folder with
`shoebox` (the program), `Start shoebox.command`, `recognizer/` and `guide/`. Open
`guide/shoebox-en.html` (or `-de`) in any browser; the images sit next to it.

## Editing

The guide must match the app: whoever changes what a person can see or do
updates it in the same pull request. The steps are in the `guide` skill
(`.claude/skills/guide/SKILL.md`). Edit the two HTML files (keep both in step,
take UI names from `core/i18n/en.json` / `de.json`), then:

```sh
python3 docs/guide/check.py   # en/de parity; with assets/ also images and the dots
```

## What CI does

`.github/workflows/guide.yml` runs for pull requests and pushes to main that
touch `core/web/`, `core/i18n/` or `docs/guide/`, and for every release (called
from `build.yml`). It builds a Linux shoebox, makes the screenshots, writes the
`.txt` copies, runs `check.py` and uploads the finished guide as the artifact
`guide` (open the run's summary page to download it). The release puts that
folder into the tarball, so a release always shows the screen of its own code.

The screenshots are made with the fonts in `fonts.conf` (Inter for the Mac's
system font, Noto for emoji), as in the Claude Code cloud containers.

## Screenshots

Only in CI: `screenshots.sh` (and `make-drives-pair.py`, which it calls)
refuse to run anywhere but on a GitHub Actions runner, and never on a Mac.
They create and delete folders where drives live (`/Volumes/Photos`,
`/mnt/Extreme SSD`, `/mnt/Backup Drive`): shoebox reads a drive's name from its
path, and on a real computer those can be real drives with real photos. To see
the screenshots of a change, push it and open the run's artifact `guide`.

They come from a sample library of drawn, synthetic pictures (comic people, a
dog and a cat) with faces and pets from a mock recognizer, so no models are
needed. The passes (`screenshots.mjs <pass>`), each on a fresh copy of the
library; `LANGS` and `PASSES` limit a run (CI runs one language per job):

| Pass | Screenshots |
|------|-------------|
| `app` | the photo app: overview, viewer, duplicates, people, tags, search, export, trash, settings, iPad |
| `launcher` | the Control Panel, and a scan that finds new copies |
| `drives` | Manage drives and the backup comparison views (two drives, an original and a drifted backup) |
| `suggestions` | the tag and name suggestion lists |
| `text` | words in photos: the search suggestions and results, the viewer's text list, Settings → Data cleanup (the library gets a folder of drawn documents, a street sign and a phone screenshot, `SHOEBOX_SAMPLE_TEXT=1`; the mock recognizer's `text` task "reads" them from the annotations) |

`comic.py` draws the pictures, `make-sample-library.py` builds the library
(once per language; each pass gets a copy), `mock-recognizer.py` answers the
recognizer protocol for it (docs/protocol.md), `make-drives-pair.py` makes a
backup of that library that has drifted from it (photos not on the backup yet,
only on the backup, different on the backup) for the `drives` pass.
`shrink-shots.py` turns the PNGs into the 1800 px JPEGs in `assets/`.
`screenshots.mjs` also writes `assets/<lang>/ui-overview.json`: where each
control of "The screen at a glance" is, for `check.py`'s dot check.
