# shoebox

A small, portable photo library that lives on the external drive next to the
photos. Originals are never modified or copied; shoebox only keeps an index
(SQLite) and a preview cache in `.shoebox/` on the same drive.

```
shoebox/
  core/         Rust: scanner, database, web UI (single binary per platform)
  recognizer/   Python: face recognition worker (optional)
  scripts/      build and fixture scripts
  docker/       development container
  docs/         design notes and phase guides
```

## Status

Phases 0–4 (probe, scan and verify, web UI, import and organising, face
detection with `shoebox recognize`) done except the runs on real hardware.
Plan and status: [docs/plan.md](docs/plan.md); usage per phase in `docs/`
([recognizer](docs/phase4.md)).

## Video previews (optional)

shoebox makes the grid previews of videos with `ffmpeg`. It is not bundled;
put a static build next to the shoebox binary on the drive (or have one on
PATH):

1. Download a static `ffmpeg` for the computer that runs shoebox:
   - macOS on Intel: [evermeet.cx/ffmpeg](https://evermeet.cx/ffmpeg/)
     (the zip with the `ffmpeg` binary). It also runs on Apple Silicon
     through Rosetta, so one copy serves both Macs.
   - macOS on Apple Silicon only: [osxexperts.net](https://www.osxexperts.net/)
     has native arm64 builds.
   - Linux: [johnvansickle.com/ffmpeg](https://johnvansickle.com/ffmpeg/)
     (`ffmpeg-release-amd64-static.tar.xz`).
2. Copy the `ffmpeg` binary to `.shoebox/bin/ffmpeg` on the drive, next to
   `shoebox-macos` / `shoebox-linux`. The name must be exactly `ffmpeg`.
3. On macOS, in Terminal:

   ```sh
   cd "/Volumes/<drive>/.shoebox/bin"
   xattr -d com.apple.quarantine ffmpeg   # only if downloaded via a browser
   chmod +x ffmpeg
   ./ffmpeg -version                      # must print a version, not an error
   ```

4. Restart `shoebox serve` (it looks for ffmpeg when it starts) or run
   `shoebox scan` again: videos that had no preview get one, and the "no
   ffmpeg" note disappears from the status line in the web UI.

shoebox only ever runs ffmpeg to read a video and pipe one frame to itself;
it never writes next to the original. Without ffmpeg, or for a video ffmpeg
cannot read, the web UI grabs a frame in the browser instead (not stored, so
it costs a little time on each visit, and iPad Safari often refuses).
Static ffmpeg builds are GPL; fine for your own drive, check the terms
before passing the drive's binaries on.

## Building

Development happens in Docker (Linux build):

```sh
docker build -t shoebox-dev -f docker/Dockerfile.dev docker
docker/dev.sh sh -c 'DEPS_DIR=/target/deps scripts/build.sh'
docker/dev.sh sh -c 'scripts/make-fixtures.sh /target/fixtures && SHOEBOX_FIXTURES=/target/fixtures DEPS_DIR=/target/deps scripts/build.sh "" test'
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
