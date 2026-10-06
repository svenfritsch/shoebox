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
- Originals change only through explicit user actions (move, rename, trash,
  import) in `organize.rs`/`import.rs`: rename only, never copy, never
  replace, and only files that still match the index.
- The one exception: turning a JPEG (`organize::rotate`) overwrites the two
  bytes of its EXIF Orientation tag in place, only when the file matches the
  index, and checks the full hash afterwards (old content plus those two
  bytes). Never anything else in an original.
- UI text is never hard-coded: `tr()`/`trn()`/`data-i18n` with a key in both
  `core/i18n/en.json` and `de.json` (see docs/phase8.md; `core/tests/i18n.rs`
  checks it).
- Paths may contain spaces and decomposed Unicode: quote everything, compare
  NFC-normalised.

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
