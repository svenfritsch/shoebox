# shoebox: plan

Portable photo library manager that lives on the external drive next to the
photos. This document is the single source of truth for scope, decisions and
progress. Update the status section when a phase moves.

## Goal and principles

- **No copies, no changes to originals.** shoebox only indexes files by path
  and hash. Originals are opened read-only; the only operations that touch
  them are explicit user actions (move, import, delete duplicate).
- **Timestamps are sacred.** EXIF `DateTimeOriginal` and the file's created
  date must never change. Moving within the drive uses `rename`, which keeps
  all timestamps. Every scan path is covered by the *guard* (see below).
- **Local only.** No cloud services; everything runs on the machine the drive
  is plugged into.
- **Portable, few dependencies.** One binary per platform on the drive. Must
  run on an older Intel MacBook with an older macOS.
- **Web UI**, so an iPad on the same network can browse the library.

## Decisions

| Topic | Decision |
|---|---|
| Name | **shoebox** |
| Core | Rust, one static binary per platform: scanner, SQLite, web server, UI |
| Recognition | Separate, optional Python worker (`recognizer`), JSON lines over stdin/stdout |
| Thumbnails | **Not** written into EXIF of originals (would change hashes and mtimes). Stored as BLOBs in `.shoebox/thumbs.db` (exFAT's 128 KB clusters would waste ~5× space with one file per thumbnail) |
| HEIC | libheif + libde265 built from source as static, decoder-only libs (`scripts/build-deps.sh`). LGPL: fine for personal use, check before distributing |
| RAW | Indexed (hash, dates, duplicates, backup checks) but hidden in the UI; paired with JPEG/HEIC of the same stem and moved together |
| Video | Yes. Metadata via `nom-exif`; poster frame via a static `ffmpeg` next to the binary; playback streams the original (HTTP range requests), no transcoding |
| Drive format | exFAT |
| Scale | ~100,000 files, 150 GB today, up to 1 TB |
| Repo | `core/` (Rust), `recognizer/` (Python), `scripts/`, `docker/`, `docs/` |
| Dev environment | Docker for Linux builds/tests; native macOS builds with rustup; GitHub Actions for both |

## Architecture

### On the drive

```
/Volumes/<drive>/
  <library>/                     ← user's folders; read-only for shoebox
  .shoebox/
    bin/shoebox-macos            ← universal (x86_64 10.13+, arm64 11+)
    bin/shoebox-linux
    bin/shoebox.exe              ← later
    bin/ffmpeg                   ← optional, static, for video posters
    library.db                   ← SQLite index (+ rotating backup copy)
    thumbs.db                    ← preview BLOBs keyed by quick hash
    recognizer/                  ← optional
      runtime/<platform>/        ← standalone Python (copies, no symlinks: exFAT)
      recognizer.py
      models/                    ← ONNX: YuNet, SFace, animal detector, CLIP
  Start shoebox.command          ← launcher scripts per OS
```

### Responsibilities

- **Rust core** owns everything stateful: walking, hashing, metadata,
  SQLite (single writer), thumbnails, web UI (assets embedded with
  `rust-embed`), import/move/duplicates, backup verification, and the
  clustering/matching of recognition embeddings.
- **Python recognizer** is stateless: image path in, boxes + labels +
  embeddings out. No DB access, no knowledge of names. If it's missing, the
  app works without recognition features.

Worker protocol (to be specified in `docs/protocol.md` before phase 4):

```
→ {"id": 42, "path": "/Volumes/.../IMG_1.jpg", "tasks": ["faces","animals"]}
← {"id": 42, "faces": [{"bbox": [x,y,w,h], "score": 0.97, "emb": "<base64 f32×128>"}], "animals": [...]}
```

The core gets a fake worker for tests so it can be developed without Python.
Later option: run the ONNX models in Rust (`tract` or `ort`) and drop Python.

### Planned crates

`axum` + `tokio` (web), `rust-embed` (assets), `rusqlite` with `bundled`,
`walkdir`/`jwalk`, `blake3`, `nom-exif` (EXIF + MP4/MOV), `libheif-rs`,
`image` + `fast_image_resize`, `image_hasher` (perceptual hash),
`unicode-normalization`, `hnsw_rs` or `instant-distance` (face clustering),
`tracing`, `anyhow`/`thiserror`, `clap`.

## Library rules

### Folders

- Event folder: name matches `^(\d{4})-(0[1-9]|1[0-2])\s+(.+)$`, e.g.
  `2020-07 Urlaub Griechenland`. Its date contributes to sorting.
- Topic folder: anything else (people, places, things). Timeline position
  comes from each photo's capture date.
- Nesting is unlimited. Every path level becomes a tag, so a photo in
  `Familie/Weihnachten` is found under "Familie" and under "Weihnachten".
- Never assume every folder starts with a date.

### Import

Drag and drop in the browser (works from other devices too, as an upload).
Dialog asks year, month, event name and creates `YYYY-MM Name`. File modified
dates are set from the browser's `File.lastModified`.

### Scanner (phase 1)

- Skip: `._*`, `.DS_Store`, `.Spotlight-V100`, `.Trashes`, `.fseventsd`,
  `.TemporaryItems`, `.shoebox`, `System Volume Information`, `$RECYCLE.BIN`,
  `Thumbs.db` (already in `core/src/classify.rs`).
- Per file: size, mtime, created, quick hash (size + first/last 64 KiB),
  full BLAKE3 hash (background, resumable), capture date, dimensions,
  camera, duration. Perceptual hash moved to phase 2 (computed from the
  thumbnail decode).
- Incremental: unchanged size + mtime → skip without opening.
- exFAT time-zone shift: same size and mtime differs by a whole multiple of
  15 min (≤ 14 h) → compare quick hash instead of treating as changed.
- Moves: unknown path + missing record with the same size → confirm by quick
  hash, then full hash (skipped when name and mtime also match); update the
  path instead of creating a new record.
- Unicode: store the path as found (for disk access) plus NFC form (for
  comparison/search). macOS often writes decomposed umlauts.
- Case-only renames go through a temporary name (case-insensitive FS).
- SQLite with `synchronous=FULL`; copy `library.db` after each scan (no
  journaling on exFAT).

### Guard

Any code path that reads originals is tested by snapshotting size, mtime,
created and full hash before and after, and failing on any difference.
`shoebox probe` does this at runtime; `core/tests/scan.rs` does it for
`scan` and `verify`.

### Duplicates

Exact: same full hash. Near: perceptual hash Hamming distance ≤ 8. The UI
shows them side by side; the user keeps both, links them, or deletes one
(with confirmation).

### Recognition (phases 4–6)

- Faces: YuNet (detect) + SFace (128-d embedding) via OpenCV in the worker.
  ~150k faces expected → approximate nearest-neighbour index in Rust,
  incremental clustering, no all-pairs DBSCAN.
- Correction UI: name cluster, merge, split, "not this person". Confirmed
  faces become references; new faces auto-assign when close enough.
- Pets: COCO detector for cat/dog; individual pets ("Spooky") via CLIP or
  DINOv2 embeddings of the crop matched against labelled examples. CLIP also
  enables text search later.
- Expected first run on the old Intel Mac: ~4–5 h for faces over 100k photos,
  similar for pets. Background job, resumable, progress in the UI.

### Backups (phase 7)

shoebox doesn't copy; rsync / Carbon Copy Cloner do. shoebox registers
backup targets, verifies them against its hashes (missing, different, bit
rot) and shows "last backup N days ago, M files new since".

## Phases and status

| Phase | Scope | Status |
|---|---|---|
| 0 | Toolchain + portability probe (`shoebox probe`) | **Done except the real-hardware run** (see below) |
| 1 | Scanner + SQLite schema + incremental rescan + move detection + guard integration test. CLI: `shoebox scan`, `shoebox verify` | **Done except the real-hardware run** (see below) |
| 2 | `thumbs.db` + perceptual hash, web UI (virtualised timeline grid, folder tree, tag search, video playback), LAN access with PIN. `shoebox serve` | Next |
| 3 | Import dialog, move (with RAW pairs, case-only renames), duplicates UI, self-healing paths | |
| 4 | Worker protocol + Python recognizer (faces), worker supervision in Rust | |
| 5 | Face clustering in Rust + correction UI | |
| 6 | Pets | |
| 7 | Backup verification, launchers, packaging | |

### Phase 0 details

Done:
- `shoebox probe <folder>`: walks, hashes, reads metadata, renders previews,
  re-checks every original (guard), reports exFAT, non-NFC names, RAW/Live
  Photo pairs, truncated JPEGs. See [phase0.md](phase0.md).
- Universal macOS binary built locally (`dist/shoebox-macos`, 7.3 MB),
  links only system libs. Tested on macOS 26 arm64 and x86_64 via Rosetta.
- Linux build in Docker, also tested on a loop-mounted exFAT image.
- GitHub Actions workflow builds and tests macOS + Linux.

Open:
- [ ] Run `shoebox probe` on the old Intel MacBook against real folders on
      the exFAT drive (checklist in [phase0.md](phase0.md)).
- [ ] Confirm the GitHub Actions run is green.
- [ ] Get a static Intel `ffmpeg` for the drive and test video posters.

Known gaps: ORF/RW2 metadata may be unsupported by `nom-exif`; Motion
Photos (data after JPEG end marker) trigger a false "truncated" warning;
no Windows build yet.

### Phase 1 details

Done (see [phase1.md](phase1.md)):
- Crate split into a library (`core/src/lib.rs`) and a thin CLI.
- SQLite schema v1 (`folders`, `files`, `tags`, `file_tags`, `jobs`) in
  `core/src/db.rs`; rollback journal, `synchronous=FULL`, `library.db.bak`
  after each scan.
- `shoebox scan`: incremental (size + mtime), exFAT time-zone shifts, move
  detection, Unicode-form renames, missing records kept (`--forget-missing`),
  resumable full-hash pass (`--quick` skips it), per-file stamp check around
  every read.
- `shoebox verify`: missing / changed / damaged, database `quick_check`,
  least recently verified first (`--limit`, `--quick`).
- Integration tests (`core/tests/scan.rs`) including the guard; they use the
  `make-fixtures.sh` output when `SHOEBOX_FIXTURES` is set (CI does).

Open:
- [ ] Scan, rescan and verify on the old Intel MacBook against the exFAT
      drive (checklist in [phase1.md](phase1.md)).
- [ ] Confirm the GitHub Actions run is green.

## Build notes and pitfalls (learned in phases 0–1)

- **Spaces in paths.** The repo may live under a path with spaces.
  `build-deps.sh` uses bash arrays for CMake args; never unquote paths.
- **No pkg-config for libheif.** Homebrew's libheif leaked into the link once.
  `scripts/build.sh` sets `SYSTEM_DEPS_LIBHEIF_NO_PKG_CONFIG` and points
  `SEARCH_NATIVE`/`LIB`/`INCLUDE` at our static build; `build-deps.sh` gives
  pkg-config an empty search path and disables CMake system paths.
- **C++ runtime.** `core/build.rs` links `c++` on macOS and passes
  `-lstdc++` as a trailing link arg on Linux (GNU ld is order-sensitive).
- **x86_64 macOS** needs `libclang_rt.osx.a` for libde265's runtime AVX2
  detection (`__builtin_cpu_supports`); `build.rs` locates it via clang.
- **HEIC EXIF**: `nom-exif` misses EXIF in some HEIC layouts; we extract the
  EXIF block with libheif and parse only the TIFF payload with `nom-exif`.
- **Truncated JPEGs decode "successfully"** (grey bottom); check the FF D9
  end marker.
- **Toolchain on the dev Mac:** Homebrew `rustup` is keg-only; use
  `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`. Homebrew's own `rust`
  (1.82) is too old for the crates.
- **CMake can't find make/cc.** `build-deps.sh` turns off
  `CMAKE_FIND_USE_SYSTEM_ENVIRONMENT_PATH` (so nothing leaks in from
  Homebrew), which also stops CMake searching PATH for the build tool and
  compilers. The script passes `CMAKE_MAKE_PROGRAM` and the compilers (from
  `$CC`/`$CXX`, default `cc`/`c++`) explicitly. A cached deps dir hides this
  bug; it only shows on a cache miss.
- **Integration tests and fixtures.** `core/tests/scan.rs` copies
  `$SHOEBOX_FIXTURES` into each test library when set; an unset or empty
  variable just uses the synthetic files the test writes itself.
- **Git push** uses SSH via the 1Password agent with the "GitHub" key pinned
  in this repo's `core.sshCommand` (the keychain's HTTPS login belongs to a
  different account, `svenfritschpeers`).

## Next step

Start phase 2: `thumbs.db` (BLOBs keyed by quick hash, perceptual hash from
the same decode), `shoebox serve` with the virtualised timeline grid, folder
tree, tag search and video playback via range requests.
