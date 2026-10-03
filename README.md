# shoebox

A small, portable photo library that lives on the external drive next to the
photos. Originals are never modified or copied; shoebox only keeps an index
(SQLite) and a preview cache in `.shoebox/` on the same drive.

```
shoebox/
  core/         Rust: scanner, database, web UI (single binary per platform)
  recognizer/   Python: face/pet recognition worker (optional, later phases)
  scripts/      build and fixture scripts
  docker/       development container
  docs/         design notes and phase guides
```

## Status

Phase 0 (toolchain and portability probe). See [docs/phase0.md](docs/phase0.md).

## Building

Development happens in Docker (Linux build):

```sh
docker build -t shoebox-dev -f docker/Dockerfile.dev docker
docker/dev.sh sh -c 'DEPS_DIR=/target/deps scripts/build.sh'
docker/dev.sh sh -c 'scripts/make-fixtures.sh /target/fixtures && /target/*/release/shoebox probe /target/fixtures'
```

macOS binaries (universal: Intel macOS 10.13+ and Apple Silicon) are built by
GitHub Actions (`.github/workflows/build.yml`), or locally with rustup:

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH   # Homebrew's rustup is keg-only
rustup target add x86_64-apple-darwin aarch64-apple-darwin
scripts/build.sh x86_64-apple-darwin
scripts/build.sh aarch64-apple-darwin
mkdir -p dist && lipo -create -output dist/shoebox-macos \
    core/target/x86_64-apple-darwin/release/shoebox \
    core/target/aarch64-apple-darwin/release/shoebox
```

Building needs CMake and Xcode's clang. The build never uses Homebrew's
libheif: `scripts/build.sh` points the linker straight at the static libraries
from `scripts/build-deps.sh`.

HEIC support comes from libheif + libde265, built by `scripts/build-deps.sh`
as static, decoder-only libraries (both LGPL; fine for personal use, check the
LGPL terms before distributing binaries).
