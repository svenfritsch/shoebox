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

# macOS (Homebrew rustup is keg-only)
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
scripts/build.sh x86_64-apple-darwin
scripts/build.sh aarch64-apple-darwin
scripts/build.sh aarch64-apple-darwin test
```

Fixture generation needs ffmpeg, heif-enc and exiftool (all in the Docker
image). Pitfalls from earlier phases are listed at the end of docs/plan.md.
