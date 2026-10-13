# Phase 0: portability probe

Goal: prove that one shoebox binary runs on every machine the drive will be
plugged into (especially the older Intel MacBook) and can read every file type
in the library from the exFAT drive, without changing a single original.

## What `shoebox probe` does

1. Walks a folder recursively, skipping macOS bookkeeping (`._*`, `.DS_Store`,
   `.Spotlight-V100`, …).
2. Records size, modified date, created date and full BLAKE3 hash of every
   media file.
3. Reads metadata (capture date, size, camera) and renders a 256 px preview of
   every JPEG, PNG, HEIC and video. RAW files: metadata only.
4. Records everything again and requires it to be identical (**guard**).
5. Prints a summary and writes a JSON report.

Previews go to the system temp folder, never into the photo folder.

## Running it on the Intel Mac

1. Get `shoebox-macos`: download `shoebox-macos.tar.gz` from the latest
   GitHub release (published by the `build` workflow when a `v*` tag is
   pushed) and unpack it, or use `dist/shoebox-macos` from a local build
   (see README). Copy it to the drive, e.g. into `.shoebox/bin/`.
2. Optional, for video previews: put a static `ffmpeg` for Intel macOS next
   to the binary (or have one on PATH); see
   [Video previews](../README.md#video-previews-optional).
3. In Terminal:

   ```sh
   xattr -d com.apple.quarantine shoebox-macos   # only if downloaded via a browser
   chmod +x shoebox-macos
   ./shoebox-macos probe "/Volumes/<drive>/<library>/<an event folder>" --limit 300
   ```

   The probe is read-only, so it can run on real folders. Pick one folder
   with HEIC photos and videos from an iPhone and one older folder with JPEGs
   from a camera. If you have RAW files, include a folder with those too.

## Success criteria

- [x] Binary starts on the Intel Mac (no "unsupported macOS version", no missing library)
- [x] `filesystem: exfat` is detected
- [x] HEIC, JPEG and video: metadata and preview for (almost) every file
- [ ] RAW: capture date found. `.CR2` files are indexed (hash, dates, duplicates, backup) but hidden in the photo app by design (plan.md, "RAW"), so they are not searchable there; check the probe's output for the RAW folder, not the UI.
- [ ] `GUARD OK`: the probe records size, modified and created date and the full hash of every file it reads, reads them all, then records them again. `GUARD OK` means nothing differed; `FAILED` lists the files and what changed. This is the rule that originals are never touched, tested on your real folders.
- [ ] Hash throughput noted: the "Hashing…" line prints the bytes read, the seconds and MB/s. Divide the library size by that speed to estimate the first full scan (for example 500 GB at 100 MB/s is about 1.5 hours). It shows whether the old Mac or the drive is the slow part.

Send back the terminal output. The JSON report contains file paths and camera
models, so only share it if you're fine with that.

## Verified so far (in Docker, Linux arm64)

- Static libheif/libde265 link; binary depends only on system libc/libstdc++.
  (Since then they are shared libraries next to the program, for the LGPL;
  see "Build notes and pitfalls" in plan.md.)
- Fixtures: JPEG, PNG, HEIC, Live Photo pair, MP4/MOV, TIFF-based RAW
  stand-in, decomposed umlaut folder name, truncated JPEG, `._*` files.
- Same run on a real exFAT filesystem (loop-mounted): detected, guard OK.
- Universal macOS binary (7.3 MB) built locally: x86_64 needs macOS 10.13+,
  arm64 needs 11.0+; links only system libraries (libc++, libSystem,
  CoreFoundation, libiconv).
- Fixtures on macOS 26 (APFS): arm64 natively and x86_64 under Rosetta, both
  GUARD OK including the created date. Video previews need ffmpeg (not
  installed on the build Mac).

## Known gaps

- Only TIFF-based RAW (CR2, NEF, ARW, DNG) and CR3/RAF are parsed; ORF/RW2
  may report "unsupported format" for metadata.
- JPEGs with data after the end marker (Motion Photos) trigger a false
  "may be truncated" warning.
- Windows build not set up yet.
