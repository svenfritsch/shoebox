# shoebox

Portable photo library manager that lives on an external exFAT drive next to
the photos. Rust core (`core/`), optional Python recognition worker
(`recognizer/`).

**Read [docs/plan.md](docs/plan.md) first**: decisions, architecture, phases,
current status and next step. Update its status table when a phase moves.

## Rules

- Originals are read-only. Never write metadata or thumbnails into photo
  files; never change capture dates or created dates.
- Any code that reads originals must be covered by the guard (snapshot size,
  mtime, created, full hash before and after; fail on any difference).
- Originals change only through explicit user actions (move, rename, trash)
  in `organize.rs`: rename only, never copy, never replace, and only files
  that still match the index. There is no import: photos are copied onto the
  drive in the file manager (which keeps their dates) and a scan finds them.
- After a scan, `arrivals::cleanup` (launcher button "Delete the new copies")
  may remove files the scan added whose content (full hash) was on the drive
  before that scan, through `duplicates::remove_copies` (the older copy stays;
  trash or, if asked, for good). Never anything but those new copies.
- The one exception: turning a JPEG (`organize::rotate`) overwrites the two
  bytes of its EXIF Orientation tag in place, only when the file matches the
  index, and checks the full hash afterwards (old content plus those two
  bytes). Never anything else in an original.
- The other change to a drive that is not the photo app's own: `backup::cleanup`
  (launcher button "Delete duplicates from backup as well", after a backup
  check). It takes only copies named by `multi::removable_on_backup` off the
  backup: copies removed on the original's duplicates screen
  (`removed_copies`), still there with the same path and content, while the
  kept content is on both drives and the backup keeps another file with it.
  Each goes through `organize::trash_files` (a rename into the backup's own
  trash, only if the file still matches the backup's index), emptied again
  only if the user asked for "for good". Never anything on the original.
- Moving to the trash from the photo view and the timeline selection is off by
  default: the library setting `allow_trash` (Settings, "Allow move to trash")
  shows the trash icon and the selection button.
- Maps (phase 10) are an opt-in setting (off by default, one for the whole application: kept by the
  browser as `shoebox-maps`, not per drive). The browser
  loads the tiles; shoebox never fetches or stores one. A position the user
  gives a photo lives in `library.db` (`geo_overrides`) only, never in the file.
- Licences: `LICENSE.txt` (all rights reserved, private use) and the generated
  `THIRD-PARTY-LICENSES.txt` go into the release archive. After a change to
  `Cargo.lock`, `scripts/build-deps.sh` or `core/web/vendor/` run
  `python3 scripts/third-party-licenses.py`. Do not call shoebox "open source"
  in texts. Open items before giving it to others: see "Before giving shoebox
  to others" in docs/plan.md.
- UI text is never hard-coded: `tr()`/`trn()`/`data-i18n` with a key in both
  `core/i18n/en.json` and `de.json` (see docs/phase8.md; `core/tests/i18n.rs`
  checks it).
- Paths may contain spaces and decomposed Unicode: quote everything, compare
  NFC-normalised.
- **The guide is part of every change.** `docs/guide/shoebox-en.html` and
  `shoebox-de.html` (kept in step) are what users read about the app. A change
  that adds, removes or alters something a person can see or do (the photo app,
  the launcher, a button, a setting, what a command does, the safety rules
  above) updates both files in the same pull request, and removing a feature
  removes it from the guide. Before finishing, grep both files for the old
  behaviour and its UI names (`grep -n -i '<word>' docs/guide/shoebox-*.html`)
  and read the guide's pages that touch it: the feature cards, the "at a
  glance" legend (its numbered dots sit at fixed positions on `ui-overview`,
  one dot per legend item, in the order of the screen), the typical session,
  the launcher tasks, the sections on the feature, the FAQ and "Your photos are
  safe". If the screen changed visibly, regenerate the screenshots
  (`docs/guide/README.md`) and check that the dots still sit on the right
  controls; the sample screenshots must not use a word that is a feature now
  (the tag "Favorites" is the heart). Then run `python3 docs/guide/html2txt.py`
  and commit the `.txt` files. Say in the pull request what changed in the
  guide, or that it is not affected. Look at `main` first: a pull request
  stacked on a branch that is already merged never reaches `main`.

## Build and test

```sh
# Linux, in Docker
docker build -t shoebox-dev -f docker/Dockerfile.dev docker
docker/dev.sh bash -c 'DEPS_DIR=/target/deps scripts/build.sh'
docker/dev.sh bash -c 'DEPS_DIR=/target/deps scripts/build.sh "" test'
# integration tests with the full fixtures (HEIC, video, EXIF dates)
docker/dev.sh bash -c 'scripts/make-fixtures.sh /target/fixtures && SHOEBOX_FIXTURES=/target/fixtures DEPS_DIR=/target/deps scripts/build.sh "" test'
docker/dev.sh bash -c 'scripts/make-fixtures.sh /target/fixtures && /target/*/release/shoebox probe /target/fixtures'
# the real recognizer too (after recognizer/fetch-models.sh; the image has OpenCV)
docker/dev.sh bash -c 'python3 -m unittest -v recognizer/test_recognizer.py'
docker/dev.sh bash -c 'SHOEBOX_RECOGNIZER=$PWD/recognizer/recognizer.py DEPS_DIR=/target/deps scripts/build.sh "" test'

# macOS (Homebrew rustup is keg-only)
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
scripts/build.sh x86_64-apple-darwin
scripts/build.sh aarch64-apple-darwin
scripts/build.sh aarch64-apple-darwin test
```

Fixture generation needs ffmpeg, heif-enc and exiftool (all in the Docker
image). Recognizer tests use the fake worker unless `SHOEBOX_RECOGNIZER`
points at `recognizer/recognizer.py`. Pitfalls from earlier phases are listed at the end of docs/plan.md.
