# shoebox: plan

Portable photo library manager that lives on the external drive next to the
photos. This document is the single source of truth for scope, decisions and
progress. Update the status section when a phase moves.

## Goal and principles

- **No copies, no changes to originals.** shoebox only indexes files by path
  and hash. Originals are opened read-only; the only operations that touch
  them are explicit user actions (move, delete duplicate, and turning
  a JPEG, which changes the two bytes of its EXIF Orientation tag in place).
- **Timestamps are sacred.** EXIF `DateTimeOriginal` and the file's created
  date must never change. Moving within the drive uses `rename`, which keeps
  all timestamps. Every scan path is covered by the *guard* (see below).
- **Local only.** No cloud services; everything runs on the machine the drive
  is plugged into. The one optional exception: with the Maps setting on (off by
  default, phase 10) the browser loads OpenStreetMap tiles; shoebox stores none.
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
| Video | Yes. Metadata via `nom-exif`; poster frame via a static `ffmpeg` next to the binary or on `PATH`; playback streams the original (HTTP range requests), no transcoding |
| Drive format | exFAT |
| Scale | ~100,000 files, 150 GB today, up to 1 TB |
| Repo | `core/` (Rust), `recognizer/` (Python), `scripts/`, `docker/`, `docs/` |
| Dev environment | Docker for Linux builds/tests; native macOS builds with rustup; GitHub Actions for both |
| CI until v1.0 | macOS only (PRs: arm64; main/tags: universal + Rosetta). Linux, incl. the real recognizer, runs only when started by hand; releases ship `shoebox-macos` plus `recognizer/` and the guide (`docs/guide/`: HTML and text, English and German, in the `guide/` folder of the archive) |

## Architecture

### On the drive

```
/Volumes/<drive>/
  <library>/                     ← user's folders; read-only for shoebox
  .shoebox/
    bin/shoebox-macos            ← older layout (the release now ships the folder below)
    library.db                   ← SQLite index (+ rotating backup copy)
    thumbs.db                    ← preview BLOBs keyed by quick hash, face crops only for undecided faces and people's pictures
    recognition.db               ← faces (boxes + embeddings) keyed by quick hash
    recognizer/                  ← optional (recognizer/install.sh)
      runtime/<os>-<arch>/       ← standalone Python (copies, no symlinks: exFAT)
      recognizer.py
      models/                    ← ONNX: YuNet, SFace, pet detector, CLIP
  Start shoebox.command          ← launcher scripts per OS
```

### The shoebox folder and the add-ons (Control Panel)

The release is one folder per system (`shoebox-macos/`): the program
`shoebox`, `Start shoebox.command`, `recognizer/` (installer, `recognizer.py`,
later its `runtime/` and `models/`) and `guide/`. Two ways to use it, both
supported by the same lookup:

- **On the drive**: the whole folder is copied to the drive's top folder
  (`/Volumes/X/shoebox-macos/`); other systems get their own folder beside it
  (`shoebox-windows/`). Add-ons then live in `shoebox-macos/recognizer/` on the
  drive and travel with it. `.shoebox/` is made by the first scan, never by hand.
- **On the computer**: the folder stays where the user put it; add-ons live in
  its `recognizer/` and serve every drive; drives are added by path.

Add-ons are installed from the Control Panel (step 1, "Add-ons"; job
`install_addons` runs `recognizer/install.sh --faces/--pets/--text`, on Windows `install.ps1`) into
`recognizer/` **next to the program** (`recognize::program_dir`). They are
independent: Faces (~40 MB), Pets (~140 MB), later Text; the Python/OpenCV
runtime (~200 MB) is shared and shown in the list. `POST /api/addons` reports
what is installed for the computer and for each ticked drive
(`recognize::installed`, by looking for files, never by starting the worker),
and the Control Panel greys out Recognize / Recognize lying down (need Faces)
and Recognize pets (needs Pets) with the reason. Lookup order
(`recognize::worker_dirs`): `recognizer/` next to the program, then the drive's
`.shoebox/recognizer/` (made by `install.sh <drive>`; the older
`.shoebox/bin/` layout lands here too); `find_worker_for` takes the first folder that has the
models the run needs and the Python for this computer. The models are the
same everywhere; the runtime is per `<os>-<arch>` (`recognizer/runtime/`), so
another kind of Mac shows "Python and OpenCV: required" and the add-ons "models found, needs Python for this computer", and installing
there adds only its runtime next to the existing one (`install.sh` skips models
that are present). The worker starts without face models when started
with `--pets` (hello lists only the pet tasks); the core then skips the face
passes.

**Windows installer (decision).** Windows runs `recognizer/install.ps1` and
`fetch-models.ps1` through `powershell.exe -NoProfile -ExecutionPolicy Bypass
-File` (`recognize::installer_command`; same job, same flags as `-Faces`/`-Pets`).
Chosen over doing download, checksum and unpacking in Rust because it adds no
crate (an HTTPS client, gzip, tar, sha2 would each enter `THIRD-PARTY-LICENSES.txt`
and the static build), PowerShell 5.1 and `tar.exe` ship with Windows 10 1803+,
and one script per OS mirrors `install.sh` line by line. The cost is that the
logic exists twice; `core/tests/recognize.rs` keeps the model checksums and the
Python version in step. Runtime: python-build-standalone `x86_64-pc-windows-msvc`
(archive checked against a pinned SHA-256) into `runtime/windows-x86_64/` with
`python.exe` at its top (no symlinks there, so a plain copy works on exFAT/NTFS);
only x86-64 (OpenCV has no ARM Windows wheel). Cancel kills the whole process tree
(`taskkill /T`). Status: reviewed, **not run on Windows**; the Rust side and the
checksum test run on Linux. Windows packaging (`shoebox-windows/` with
`shoebox.exe`, `Start shoebox.bat`, `recognizer/`) still waits for a Windows build
in CI: `scripts/build-deps.sh` (libheif and libde265 via cmake, sh) and
`scripts/build.sh` are POSIX-only, there is no `windows-latest` job, and the
release job packs macOS only. Once such a job exists, copy `*.ps1` along with
`*.sh` in the release step.

**What this means for phase 9 (text in photos)** — the text add-on follows the
same pattern, so the phase 9 PR must:

1. add `--text` to `recognizer/install.sh` and `fetch-models.sh` (checksummed
   PP-OCR detector and Latin recogniser; the spike in build step 1 decides the
   files and sizes) and show its size in the Add-ons list (`index.html`,
   `launcher.addon.text` and `.desc` in `en.json`/`de.json`, state pill);
2. extend `recognize::installed` / `Installed` with `text` (model files present)
   and `find_worker_for` with a text flag, `JobRequest.text`, the
   `install_addons` argument and `ADDON_OF['recognize_text'] = 'text'` in
   `launcher.js`, so "Recognize text" is grey where the add-on is missing;
3. let the worker start with only the text models: like pets, the hello lists
   the tasks that are installed, and `Worker::start` accepts any non-empty set;
   a text-only run must skip the face passes (`recognize()` already bails on a
   missing face model; the text pass must not call it), and `embed_drawn` only
   counts the kinds the worker can do;
4. mention it in the guide (both languages): the Add-ons paragraph and the
   sizes line in "Get started", the launcher task list and the People/Text
   section; regenerate the Control Panel screenshot;
5. keep the manifest-free rule: availability is decided from the files, so a
   model dropped in by hand (like DINOv2) counts.

### Responsibilities

- **Rust core** owns everything stateful: walking, hashing, metadata,
  SQLite (single writer), thumbnails, web UI (assets embedded with
  `rust-embed`), move/duplicates, backup verification, and the
  clustering/matching of recognition embeddings.
- **Python recognizer** is stateless: image path in, boxes + labels +
  embeddings out. No DB access, no knowledge of names. If it's missing, the
  app works without recognition features.

Worker protocol ([protocol.md](protocol.md)): a hello line, then one request
and one reply at a time. The core decodes the photo itself and sends pixels,
so the worker never opens an original:

```
→ {"id": 42, "tasks": ["faces"], "image": "<base64 JPEG, ≤ 1600 px>"}
← {"id": 42, "width": 1600, "height": 1200, "faces": [{"bbox": [x,y,w,h], "score": 0.97, "landmarks": [...], "emb": "<base64 f32×128>"}]}
```

The core has a fake worker (`shoebox-fake-recognizer`) for tests, so it can be
developed without Python.
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

### Adding photos (no import)

There is no upload or import in the app: a browser never reveals file paths,
so an upload could not keep a file's created date, and the app is meant to be
a viewer and tagger while the drive is managed in the Finder / File Explorer.
The user copies or moves a folder onto the drive there (dates are kept) and
runs Scan. The scan knows which records are new (their id is higher than any
record before it started) and, once everything is hashed, lists the new files
whose content the drive already had (`arrivals.rs`, `Stats.duplicates`, shown
in the launcher as "same content as: <older copy>"). The
launcher then offers "Delete the new copies" (`scan_cleanup`): each goes
through `duplicates::remove_copies` (trash, or for good if ticked; tags and
dates go to the older copy; a backup check learns it was removed on purpose).
The scan result also lists the files behind its other counts (`Stats.moved_files`
with the old path, `changed_files`, `missing_files`; at most 5000 each, the counts
stay exact). The launcher shows them as tabs next to "Results per file" (Moved,
Changed, Missing, Already on the drive, each with its count; a tab only appears
when its count is above 0, "Added" has none). Only copies that have an older copy are listed: two new files that only match
each other, a first scan, a quick scan and a file that moved are not, and the
offer is gone after the next scan. Those cases are for the duplicates screen.

### Dates shown for a photo

The timeline and the info panel use, in this order: the capture date in the
file (EXIF, video container); else the month in the name of the nearest event
folder (shown as the 1st of that month); else, for a file outside any event
folder, the earlier of its created and modification dates (a copy gets a new
created date but keeps the old modification date). Event folder names start with
year then month, 4 or 2 digits for the year (20YY), `-` or `.` between, and
any of the gaps listed under event folder naming below. Only the capture date is the day the photo was taken: for the
others the info panel shows an "estimated" mark next to the date (hover: "Not
the date the photo was taken") and a tooltip on the mark says which fallback was used.
The created date does not come before the folder because a copy made years
later has a created date of its own (a scan of 1998 sits in `98.08 Urlaub`
but was created the day it was copied to the drive).
Nothing is ever written to the file (see the rules at the top).

### Event folder naming (a setting)

Settings page, "Event folder names": three dropdowns (no free text, so the
result always fits what the scanner reads). Year first: 4 digits (`YYYY`) or 2
(`YY`, 20YY); `-` or `.` between year and month (`MM`); then a space, `_`,
`.`, `-` or nothing before the name. Stored per library as the setting
`event_pattern` (text such as `YY.MM_Name`; the default `YYYY-MM Name` is
stored as no setting). `/` is not offered: it would make a subfolder. The UI
shows the pattern with the letters of its language (`JJJJ-MM Name` in
German) in the rename hint and the move placeholder.

- The pattern only decides what the app **creates** (`library::event_pattern`,
  `GET/POST /api/event-pattern`). It refuses a year
  the pattern cannot write (2-digit years are 2000 to 2099) and a name that
  would not be read back (after a punctuation mark or nothing the name may
  not start with a digit).
- The scanner (`library::parse_event`) reads every form whatever the setting
  is, so older folders and other drives keep their date, and `2020-07-15 Foo`
  (a day) is no event folder. A scan refreshes the stored event fields of
  folders already indexed.
- A changed pattern does not rename existing folders (originals only change
  through explicit actions).

### Move dialog: "Keep folder tags" (built)

A photo's **own** tags (the ones the user added by hand) are never lost: they
always move with the photo, and duplicate cleanup (`duplicates::remove_copies`)
already hands the own and folder tags of a removed copy to the survivor as own
tags. Adding tags by hand is slow work, so the index must always keep as many
as possible. The Move dialog checkbox is only about the generated **folder
tags**, which are the ones that would otherwise be lost: "Keep folder tags" /
"Ordnertags behalten" (default: off) keeps the old folder's tags on the photo
as own tags; folder tags always follow the new folder either way.
`POST /api/move` takes `keep_folder_tags` (default false); see
`organize::move_files_with`, `tags::keep_as_own` and
`move_always_keeps_own_tags_and_can_keep_folder_tags` in `core/tests/tags.rs`.

### Recognition after a scan (open)

`shoebox recognize` (faces, `--rotated`, `--pets`) only runs when the user
starts it: from the launcher, or in Terminal. The photo app does not call it:
`serve` only clusters faces and embeds faces and pets drawn by hand, and the
launcher refuses to run a job while the photo app is running ("the photo app
is already running; stop it first"). So photos added by a scan
while the photo app is open stay unrecognised until the user stops the app and
runs Recognize in the launcher. To do: after a rescan
that found new files,, `serve` starts the recognizer in the background for
the new photos only (faces, the turned pass and pets, the way `shoebox
recognize` resumes), with progress in the status line (`status.finding_faces`
exists), and skips it when no recognizer is installed or a `recognize` run is
going.

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
`scan` and `verify`, `core/tests/serve.rs` for thumbnails and every
endpoint of `serve`, `core/tests/recognize.rs` for `recognize` (both
passes) and the face crops. Explicit changes (move, rename, trash) are
covered by `core/tests/organize.rs`: every file keeps content, size and
timestamps, only its path changes, nothing is replaced, and a scan and
`verify` afterwards find the index in line with the drive.

### Duplicates

Exact: same full hash. Near: perceptual hash Hamming distance ≤ 8. The UI
shows them side by side; the user keeps both, links them, or deletes one
(with confirmation).

### Recognition (phases 4–5 and 7)

- Faces: YuNet (detect) + SFace (128-d embedding) via OpenCV in the worker.
  ~150k faces expected → approximate nearest-neighbour index in Rust,
  incremental clustering, no all-pairs DBSCAN.
- Correction UI: name cluster, merge, split, "not this person". Confirmed
  faces become references; new faces auto-assign when close enough.
- People belong to at most one group (Family, Friends, …); groups are
  editable. Names, groups and decisions live in `library.db`, keyed by
  content and box so they survive moves and model changes; `recognition.db`
  stays a cache. Details in [phase5.md](phase5.md).
- Pets (phase 7, [phase7.md](phase7.md)): a COCO detector (YOLOX) finds cats
  and dogs, an image embedder (DINOv2-small preferred, PP-ResNet50 as the
  checked fallback) describes the whole pet; individual pets ("Spooky")
  are matched like people, in a space of their own with stricter thresholds.
  CLIP is for text search later and a separate model.
- Expected first run on the old Intel Mac: ~4–5 h for faces over 100k photos,
  similar for pets. Background job, resumable, progress in the UI.

### Backups (phase 6)

shoebox doesn't copy; rsync / Carbon Copy Cloner do. shoebox registers
backup targets, verifies them against its hashes (missing, different, bit
rot) and shows "last backup N days ago, M files new since".

## Phases and status

| Phase | Scope | Status |
|---|---|---|
| 0 | Toolchain + portability probe (`shoebox probe`) | **Done except the real-hardware run** (see below) |
| 1 | Scanner + SQLite schema + incremental rescan + move detection + guard integration test. CLI: `shoebox scan`, `shoebox verify` | **Done except the real-hardware run** (see below) |
| 2 | `thumbs.db` + perceptual hash, web UI (virtualised timeline grid, folder tree, tag search, video playback), LAN access with PIN. `shoebox serve` | **Done except the real-hardware run** (see below) |
| 3 | Move (with RAW pairs, case-only renames), duplicates UI, self-healing paths | **Done except the real-hardware run** (see below) |
| 4 | Worker protocol + Python recognizer (faces), worker supervision in Rust | **Done except the real-hardware run** (see below) |
| 5a | Show in Finder / Explorer, copy path | **Done except the real-hardware run** (see [phase5.md](phase5.md)) |
| 5b | Own tags (add/remove, many photos at once, search), user data backup | **Done except the real-hardware run** (see [phase5.md](phase5.md)) |
| 5b-2 | Search by several tags at once (AND, chips); people join in with 5c-3 | **Done except the real-hardware run** (see [phase5.md](phase5.md)) |
| 5c | Faces: check recognition (5c-1), people/groups/clustering (5c-2), sidebar + info panel UI (5c-3) | **5c-1 done**, checked on the real drive; **5c-2 and 5c-3 done except the real-hardware run** (see [phase5.md](phase5.md)) |
| 5d | Duplicates UI: one row per photo, multi-select, bulk delete within a folder, tag and capture-date carry-over; Move dialog "keep folder tags"; trash dialog focus | **Done except the real-hardware run** (see [phase5.md](phase5.md)) |
| 5e | Lean `thumbs.db`: face crops only for faces without a decision and for each person's picture; right-click "Use as … picture" on a person's photos | **Done except the real-hardware run** (see [phase5.md](phase5.md)) |
| 5f | "Type" check box drop-down (Photos, Videos, Live Photos) next to the search box | **Done except the real-hardware run** (see "Phase 5f details") |
| 5g | Favorites: a heart in the viewer's top bar and in the top right corner of each timeline photo; a heart button next to the type filter and "♥ Favorites" as a search suggestion | **Done except the real-hardware run** (see "Favorites" below) |
| 6 | Launcher UI (double-click start page), multiple drives, backup verification, packaging. Multi-drive can move to phase 8 if it gets much bigger than planned (see [phase6.md](phase6.md)) | **In progress**: library id, launcher (cancel, saved folders, start scripts) and the multi-drive core (hub, offline, backup roles, common timeline, cross-drive duplicates and people) built; backup verification (`shoebox backup`, launcher button, All drives page) built; add-ons (launcher step 1: install Faces / Pets into `recognizer/` next to the program, which is on the drive when shoebox was copied there; Recognize buttons grey where the add-on is missing; a phase 9 text add-on follows the same pattern); real-hardware checks and Linux/Windows packaging open |
| 7 | Pets: cats and dogs found (`shoebox recognize --pets`, launcher button "Recognize pets"), named, grouped and searched like people, also by kind ("all cats", "Katze", "Hund"); pets the detector missed can be drawn by hand; Settings → Calibration with the Face check and the new Pet check | **Built except the real-hardware run and DINOv2** (see [phase7.md](phase7.md)) |
| 8 | UI translation, German and English, JSON message files (design and steps in [phase8.md](phase8.md)) | **Built, check open**: loader, key test, launcher and the whole photo app; the real-hardware check, a native read-through of the German texts and the CI run are open |
| 9 | Text in photos (OCR): documents, screenshots, street signs, shop fronts found by the words in them. A new task of the recognizer worker, results in `recognition.db` with an FTS5 index, a search term in the UI | **Planned** (design below, no code yet) |
| 10 | Locations: GPS positions from files, map in the photo info (Leaflet, OpenStreetMap, opt-in setting), Locations page with clustered pins, named places drawn on the map, positions set by hand (design in [phase10.md](phase10.md)) | **Built, check open**: scan reads GPS, schema v10, API, UI, guide; real-hardware check and the CI run are open |
| 11 | Screenshots: a fourth entry in the Type drop-down, found from metadata and a pixel check (no new model); "Photos" then means stills that are not screenshots (design in [phase11.md](phase11.md)) | **Built with test pictures; threshold and real-hardware run open** (see [phase11.md](phase11.md)) |

### Phase 10: locations

Positions from the file's GPS data, a map in the photo info, a Locations page
with clustered pins and named rectangular places, positions set by hand for
photos without one (in `library.db` only). Maps (Leaflet, OpenStreetMap tiles
loaded by the browser) are an opt-in setting. Design, API and open checks in
[phase10.md](phase10.md).

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

### Phase 2 details

Done (see [phase2.md](phase2.md)):
- `.shoebox/thumbs.db` (BLOBs keyed by quick hash, 384 px JPEG), made by a
  multi-threaded pass in `shoebox scan` (`--no-thumbs` skips it) and on
  demand by the server. Scaled JPEG decoding, EXIF orientation, video
  posters via ffmpeg piped to stdout. Without ffmpeg (or when it fails) the
  web UI grabs a video frame itself (canvas, session only, never stored).
- Keyboard: arrow keys move the focus in the grid; closing the viewer
  focuses the cell of the last item seen. Space opens the focused item and
  closes the viewer again.
- Perceptual hash (64-bit DCT) from the same decode in `files.phash`.
- `shoebox serve` (axum, UI embedded with rust-embed): virtualised timeline
  grid grouped by month, folder tree, search over paths and tags, viewer
  with video playback (range requests), HEIC rendered to JPEG, Live Photos
  folded into their stills, info panel, download.
- Localhost only by default; `--lan` with PIN login, session cookie,
  DNS-rebinding protection, rate-limited PIN attempts.
- Integration tests `core/tests/serve.rs` (guard, thumbnail lifecycle,
  PIN, timeline rules); shared helpers moved to `core/tests/common/`.

Open:
- [ ] Thumbnail pass and iPad browsing on the real hardware (checklist in
      [phase2.md](phase2.md)).
- [ ] Confirm the GitHub Actions run is green.

### Phase 3 details

Done (see [phase3.md](phase3.md)):
- `core/src/organize.rs`: move photos with their companions (same name stem
  in the folder: RAW, Live Photo video, XMP/AAE sidecars), all or nothing;
  rename/move folders including case-only renames (via a temporary name);
  trash in `.shoebox/trash/` with restore and empty. Only `rename`, never
  replacing (`RENAME_NOREPLACE` / `RENAME_EXCL`), names compared NFC and
  case-insensitively, files must match the index.
- `core/src/arrivals.rs` (added after phase 3, replacing the browser upload
  `import.rs`): see "Adding photos" above.
- `core/src/duplicates.rs`: exact (full hash) and near (`phash` ≤ 8 bits,
  all pairs on all cores) groups; decisions per pair (`distinct`,
  `linked`) in schema v2.
- Self-healing paths: `serve` runs the scan's index step in the background
  when a file is not where the index says.
- UI: selection with move/trash, folder
  rename, duplicates and trash pages. Added later: Shift-click selects a
  range, "Select all" on a month heading selects the month (the iPad has no
  Shift). Every non-GET request needs an
  `X-Shoebox` header (CSRF protection for localhost without PIN).

Open:
- [ ] Moves and renames on the exFAT drive from the old Intel
      MacBook and the iPad (checklist in [phase3.md](phase3.md)).
- [ ] Confirm the GitHub Actions run is green.

### Phase 4 details

Done (see [phase4.md](phase4.md)):
- Worker protocol v1 in [protocol.md](protocol.md): hello with protocol,
  tasks, model ids and embedding size; the core sends an upright JPEG copy
  (≤ 1600 px, decoded under the guard), never a path.
- `recognizer/recognizer.py`: YuNet + SFace via OpenCV, L2-normalised 128-d
  embeddings, errors per picture without dying, stdout reserved for the
  protocol. `fetch-models.sh` (checksummed), `install.sh` (standalone Python,
  OpenCV and models into `.shoebox/recognizer/`), `test_recognizer.py`.
- `core/src/recognize.rs`: finding the worker, supervision (start and reply
  timeouts, restart after crash or hang, one retry per photo, give up after
  5 crashes in a row), `shoebox recognize` (resumable, newest first, copies
  once, model changes redone, `--limit`, `--retry-failed`), pruning.
- `.shoebox/recognition.db` (attached as `recog`): `jobs`, `looked`, `faces`.
- `serve`: face progress in `/api/info` and the status line, faces per
  photo in `/api/files/{id}`, boxes in the viewer.
- `shoebox-fake-recognizer` and `core/tests/recognize.rs` (guard,
  supervision, limits, model change, pruning, API, the real worker when
  `SHOEBOX_RECOGNIZER` is set; CI runs it on Linux).

Open:
- [ ] `install.sh` and a run on the old Intel MacBook against the exFAT
      drive (checklist in [phase4.md](phase4.md)).
- [ ] Confirm the GitHub Actions run is green.

### Phase 5 details

Planned in [phase5.md](phase5.md): split into 5a (reveal in Finder /
Explorer), 5b (own tags) and 5c (faces, three PRs), separate PRs merged in
that order, with the conflict hot spots (info panel in `app.js`, route list,
schema versions v3 for 5b and v4 for 5c-2) listed there.

Open:
- [x] 5a: show in Finder / Explorer, copy path ([phase5.md](phase5.md)); the
      real-hardware check is in its list.
- [x] 5b: own tags, trash keeps them, user data backup
      ([phase5.md](phase5.md)); the real-hardware check is in its list.
- [x] 5c-1: `shoebox faces stats`, face check page (crops in `thumbs.db`
      v3, nearest neighbours), `shoebox recognize --rotated`
      (`recognition.db` v2); the real-hardware check is in its list.
- [x] 5b-2: search by several tags at once (feedback after 5b: "Spielplatz"
      and "Winter" together); terms as chips, all must match
      ([phase5.md](phase5.md)); the real-hardware check is in its list.
- [x] 5c-2: people, groups (one per person), face decisions keyed by
      content and box (`library.db` v4), "not a face", clusters and
      suggestions as a cache (`recognition.db` v3) from nearest neighbours,
      the API, user data backup; "Not a face" on the face check page and the
      5a refinement. The real-hardware check (clustering time, naming, a
      "maybe" list, the leg and the hands) is in its list.
- [x] 5c-3: Faces in the sidebar (groups, people, Unnamed), the overview,
      a person's faces (suggested, maybe, not them), unnamed cards with a
      split, groups, corrections, people in the info panel, drawn faces
      (protocol 2 `embed`, `recognition.db` v4), people as search terms
      (several together, AND); a generation per cluster
      ([phase5.md](phase5.md)). The real-hardware check, with the 5c-2
      checks folded in and all in the UI, is in its list.

### Phase 6 details

Planned in [phase6.md](phase6.md): launcher UI, multiple drives, backup
verification and packaging. Order: library id in all API routes first, then
the launcher, then backups and packaging.

Done:
- Library id in all API routes (`/api/lib/{lib}/…`, `/api/libraries`).
- Multiple drives: `serve` with several libraries (offline-tolerant), backup
  vs. separate drives, a common timeline, the duplicates screen across
  drives, people by name, backup verification
  ([phase6.md](phase6.md)).
- Launcher first slice: `shoebox` without arguments opens an embedded page
  with Scan / Verify / Recognize / Face stats, progress and per-file results,
  "Start photo app"; `--json` on the commands ([phase6.md](phase6.md)).

Open:
- [ ] Launcher UI: double-click on the binary (no arguments) starts a
      launcher mode, independent of `serve`, that opens an embedded web page
      (rust-embed, no native GUI) in the browser; the library need not be
      running. Fields for drive and photo folder paths (suggesting detected
      drives); buttons for Scan, Verify, Recognize and Face Stats; a separate
      "Start photo app" button starts `serve` only then. Results shown
      visually: progress bars and a per-file result list (succeeded /
      failed). The CLI commands return structured results (JSON); CLI and
      UI share the same logic. Safety rules stay: localhost only,
      `X-Shoebox` header on non-GET requests, guard tests cover all new
      paths. macOS: Gatekeeper blocks a double-clicked binary, so
      `Start shoebox.command` stays as fallback; check the best behaviour
      for Windows and Linux.
- [ ] Multiple drives: each drive keeps its own `.shoebox` folder
      (`library.db`, `thumbs.db`, `recognition.db`); the app opens several
      libraries at once with one shared experience, and the UI accepts two
      or more paths. All API routes carry a library id (file ids are per
      database and would collide); built first in phase 6. People match by
      name across drives (same name = same person); groups, names and
      decisions stay in each drive's `library.db`. Duplicates across drives
      are found by hash comparison (e.g. `ATTACH`), but never against a
      backup drive: backup and separate drive must be told apart (marked by
      the user or detected by overlap of hashes). An unplugged drive shows
      as "offline" while the rest keeps working. Fallback: if this part
      turns out much bigger than planned, only it moves to a later phase 8;
      the launcher and backups stay in phase 6.
- [ ] Backup verification, launchers, packaging (the former phase 7, see
      "Backups" above).

### Phase 9 details (text in photos)

Planned, nothing built (updated 2026-10-10 after merging main: Control Panel add-ons, phase 11 screenshots). Goal: type "Rechnung", "Hauptstraße" or a licence
plate and get the photos that contain those words: a photographed document,
a screenshot, a street sign, a menu, a shop front. Only the words that are
really in the picture count; what the picture shows (a "dog", a "beach") is a
different feature (CLIP, a later phase, see Recognition above) and stays out
of this one.

#### Approach in one paragraph

OCR is one more task of the existing recognizer worker. The core sends the
same upright ≤ 1600 px copy it already sends for faces and pets (decoded
under the guard, the worker never sees an original); the worker returns text
lines with their boxes and confidences. The core stores them in
`recognition.db` (a cache, like faces) with an FTS5 index, and a search term
queries that index. No tags are written, no new service, no new dependency in
the core. This is the same shape as phases 4 and 7, so almost everything
(supervision, resumable passes, launcher button, calibration page, fake
worker, guard tests) is reused.

#### Decisions

| Topic | Decision | Why / alternatives rejected |
|---|---|---|
| Where | A task `text` of `recognizer.py` (protocol 3, additive), loaded only with `--text`, like `--pets` | Reuses the supervised worker and the "optional, app works without it" rule. A second binary or an OCR crate in the core would add a dependency and break the split "ML in the optional worker, state in the core" |
| Engine | **PP-OCR (PaddleOCR) detection + recognition ONNX models**, run by onnxruntime if installed, else OpenCV's own runner (`cv2.dnn`, which also has `TextDetectionModel_DB` / `TextRecognitionModel`), exactly the runtime rule of the pets | Runs on the old Intel Mac (macOS 12, OpenCV 4.10) and on Linux with the same code, good on scene text *and* documents, small (about 15–25 MB of models), no extra Python packages. Rejected: **Tesseract** (extra native lib and language data per OS, weak on street signs and photos), **Apple Vision** (macOS only, no Linux/Windows, two code paths), **EasyOCR / PaddlePaddle** (PyTorch or Paddle runtime, hundreds of MB, will not run on the old Mac), **a cloud API** (violates "local only") |
| Languages | Latin script with German and English first: a recognizer model whose dictionary has ä ö ü ß and accents (PaddleOCR's `latin` model), not the English-only OpenCV-zoo CRNN (36 characters, no umlauts). Model id stored with each result; adding a script later means a second model id and a redo, never a mix | **To verify in step 1:** the exact model file, its licence (Apache-2.0 expected), the ONNX export, and that OpenCV 4.10 loads it. If it does not, fall back to onnxruntime only for this task and say so in `install.sh` |
| Which photos | Images only (not RAW, no video in v1; a video's poster frame could be added later). Skip a photo cheaply: the detector runs first at 960 px, and the recogniser only runs on the boxes it found. A photo with no box stores "looked, no text" and is never redone | Most family photos have no text; detection alone is far cheaper than recognition, so the first pass over 100k photos is dominated by detection |
| Orientation | v1: upright text only, plus the sideways/upside-down case through the existing `--rotated` idea as an option later (`text --rotated`, reusing the same rotation plumbing as faces). EXIF orientation is already applied by the core | Keeps the first pass fast; a document photographed 90° off is a known gap, listed under Open |
| Storage | `recognition.db` v6: `text_looked(hash, model, ts, n_lines, error)` mirrors `looked`; `text_lines(id, hash, x, y, w, h, score, text, text_norm)`; external-content FTS5 table `text_fts(text_norm)` over `text_lines`. Keyed by quick hash like faces, so it survives moves and renames. It is a cache: losing it costs a re-run, never user data | No user decisions exist for text (nothing to correct, unlike faces), so nothing goes into `library.db` and no schema bump there |
| Normalisation | `text_norm` = NFC, case-folded, diacritics folded (`straße` → `strasse`, `Müller` → `muller`), punctuation to spaces. The query is normalised the same way. The raw text is kept for display and copying | Search must find "strasse" for "Straße" and "muller" for "Müller", and must not care about NFC/NFD (a standing rule for paths, applied here too) |
| Matching | FTS5 with the **`trigram`** tokenizer (SQLite ≥ 3.34, check the bundled version) so any substring of three or more characters matches ("rechn" finds "Rechnung", a half-read licence plate still matches) and OCR typos hurt less. Queries shorter than 3 characters fall back to `LIKE` on `text_norm`. Ranking: more matched words, higher OCR confidence, bigger box first | Word tokenizers need stemming and per-language rules; trigram is language-free and tolerant. No fuzzy matching beyond that in v1 |
| Quality filter | Lines below a confidence threshold (set from the calibration page, like the pet thresholds) and single-character lines are not indexed. Stored but hidden is not an option: noise in the index is what makes text search useless | OCR on textured scenes (foliage, brick) produces garbage lines; precision matters more than recall here |
| Search term | Two clearly separate kinds of search (see "Search: names versus text" below). Plain words keep meaning file name, folder, tag, person and pet, exactly as today; **text in the picture** is its own chip (`text:winter`, or the suggestion row "In the text of photos"), ANDed with everything else ("Rechnung" in text + folder "2023") | A plain word matching OCR would drown tag results in photos that merely contain the word. Same mechanism as the pet term (`pet=cat`), so term parsing, chips and AND logic are reused; the API gets `text=` |
| Results | Normal timeline grid. Each cell hit by text shows the matching line as a small caption under the thumbnail; in the viewer the matching boxes are outlined (same overlay as face boxes, own colour) and the info panel has a "Text in this photo" block listing the lines with a copy button. Selecting text itself on the image is not built | Reuses the viewer overlay and the info panel hot spots listed in phase 5 |
| Multi-drive | Each drive keeps its own `recognition.db`; a text search fans out over all online libraries and merges by the common timeline, like tags | Consistent with phase 6. An offline drive's photos are simply missing from the results |
| Launcher / CLI | `shoebox recognize --text` (resumable, in the order above, `--limit`, `--retry-failed`, a model change redoes it) and a **"Recognize text" button** in the Control Panel next to Recognize / Recognize pets, with the same progress bar and per-file results, a Cancel that keeps what was read, and the same grey-out with a reason when the Text add-on is missing. The add-on, `--text` in the installers and the job plumbing follow the checklist under "What this means for phase 9" above. `shoebox text stats` prints photos read, with text, lines and the model | Same shape as `--pets`. Resumable and partial results are usable at once: the search works on whatever has been read so far and the status line says "Text read for 12,400 of 50,000 photos" |
| Privacy | Photos of documents contain names, addresses and numbers. The index is a plain SQLite file on the user's own drive, never leaves the machine, and is covered by the "local only" principle; the settings page gets a "Delete all recognised text" button | Stated explicitly because text search makes sensitive content easy to find for anyone who can open the web UI (LAN PIN applies) |
| Not in scope | Handwriting, translation, tables/layout analysis, PDF files, searching text in video, "find similar documents", semantic search ("photos of receipts" without a word in them). Document *classification* is a CLIP job and belongs to that later phase | Keep the first slice small and shippable |

#### Search: names versus text

Today one search box does one job: every word must be found in the path (folder
and file name), in one of the photo's tags, or in the name of someone
confirmed on it (`browse.rs`, `Query.text`, `q=` in the API). That stays
exactly as it is, and is named in the UI as **names and tags**. Text in the
picture is a second, separate kind of term:

| | Names and tags (today) | Text in photos (new) |
|---|---|---|
| Looks at | folder names, file name, tags, people, pets | words an OCR pass read in the picture |
| Typed | plain words: `winter` | `text:winter`, or the last suggestion row "In the text of photos: winter" |
| Chip | plain chip | chip with a "T" icon and the label "Text: winter", own colour |
| API | `q=winter` | `text=winter` (repeatable, AND) |
| Computed from | `files`, `tags`, `people` | `recognition.db` (`text_fts`) |
| Before the Text add-on is installed or read | works | the suggestion row shows greyed with "Read the text of your photos first (Control Panel)" |

The two combine with AND like every other term (and with folder, people, type
and favorites). The suggestion list shows the groups in this order: names,
folders and tags (today's rows), people and pets, then the one text row, so
nobody gets OCR hits by accident. A hit by text shows the matching line under
the thumbnail; a hit by name shows nothing extra, as today.

**Typed shortcuts and the file-name search.** Two prefixes switch the box to
one kind of search and keep the suggestion list short (no tags, people or pets
mixed in):

| Typed | Suggestion list shows | Chip | API |
|---|---|---|---|
| `name:IMG_62` | one group "File names": the row "File names containing “IMG_62”" with its photo count | grey **NAME** badge: `NAME IMG_62 ✕` | `name=IMG_62` |
| `text:Rechnung` | one group "In the text of photos": the row "Photos with “Rechnung” in the text" | accent **T** badge: `T Rechnung ✕` | `text=Rechnung` |
| plain `Rechn` | today's groups (tags, folders, faces, pets) and, last, "In the text of photos" | as today | `q=` |

- `name:` matches the **file name only** (the last path segment, NFC,
  case- and diacritic-insensitive substring), not folders, tags or people. It
  is for "I know roughly what the file is called" (`name:IMG_6620`,
  `name:scan`), where today a plain word also drags in every tag and person.
  Folders stay reachable as folder suggestions or by plain words.
- Prefixes are case-insensitive, work in both UI languages and are the same in
  English and German (keyboard shortcuts, like `text:`/`name:` in a mail
  client), but every label around them is translated (`tr()`, both JSON files).
- Several chips of any kind combine with AND, including several text chips
  (`text:Rechnung` + `text:Stadtwerke`) and several name chips; the same term
  twice is one chip. A text chip next to a tag, folder, person, pet, place or
  favorites chip narrows that result like any other chip. So yes: `text:Rechnung`
  becomes a chip in the multi-chip search.
- Enter on a typed prefix adds the chip; Escape and ✕ remove it. A prefix with
  nothing after it shows a one-line hint instead of a list.

Mock-ups (static HTML rendered with the app's own stylesheet; the real UI
will differ in detail):

- plain word, with the new last group and the shortcut hint:
  [docs/mocks/search-text-dropdown.png](mocks/search-text-dropdown.png)
- `text:` typed, one group only:
  [docs/mocks/search-text-prefix.png](mocks/search-text-prefix.png)
- `name:` typed, one group only:
  [docs/mocks/search-name-prefix.png](mocks/search-name-prefix.png)
- three chips in one search (tag, text, name) with the matching line under
  each hit: [docs/mocks/search-text-chips.png](mocks/search-text-chips.png)

**Guide.** The guide describes what the app does today, so the text below is
**not** in `docs/guide/` yet: it goes into both guide files in the same pull
request that builds the search (`CLAUDE.md`: the guide is part of every
change), with the "at a glance" legend, the Search section, the launcher task
list, the Add-ons paragraph and the FAQ checked as listed there, and the
`shoebox-*.txt` files regenerated.

> EN, section "Search": *Plain words search file names, folders, tags, people
> and pets. Two shortcuts search one thing only. Type* `name:` *and part of a
> file name (*`name:IMG_62`*) to search file names only, without tags and
> people in the list. Type* `text:` *and a word (*`text:Rechnung`*) to find
> photos that contain that word in the picture itself: documents, screenshots,
> street signs. This needs the Text add-on and a run of* Recognize text *in the
> Control Panel. Both shortcuts become chips (a grey* NAME *or an orange* T*) and
> combine with every other chip, for example a folder and* `text:Rechnung`*.*
>
> DE, Abschnitt „Suche“: *Einfache Wörter durchsuchen Dateinamen, Ordner, Tags,
> Personen und Haustiere. Zwei Kürzel suchen nur eines: Mit* `name:` *und einem
> Teil des Dateinamens (*`name:IMG_62`*) suchst du nur in Dateinamen, ohne dass
> Tags und Personen in der Liste auftauchen. Mit* `text:` *und einem Wort
> (*`text:Rechnung`*) findest du Fotos, auf denen dieses Wort selbst zu lesen ist:
> Dokumente, Bildschirmfotos, Straßenschilder. Dafür braucht es das
> Text-Add-on und einen Lauf von* Text erkennen *in der Schaltzentrale. Beide
> Kürzel werden zu Chips (ein grauer* NAME *oder ein oranges* T*) und lassen sich
> mit jedem anderen Chip kombinieren, zum Beispiel mit einem Ordner und*
> `text:Rechnung`*.*

(The exact German button name is taken from `de.json` when the key exists; the
text above is a draft.)

How it runs: the text term becomes a set of photo ids, the way the people
term does (`files_of_people`: matching quick hashes, mapped to file ids by
the same hash-to-ids lookup), and is intersected with the other sets. It is
evaluated only when a text chip is present; typing in the box never queries
the text index (the suggestion row needs no lookup).

#### What it costs: space, search speed, reading time

These are **estimates for planning, not measurements**; step 1 (the spike)
replaces them with numbers from the real drive, and they are written back here.

*Assumed library:* 100 GB of photos is about 50,000 files (the plan's "150 GB,
100,000 files" scale gives 1.5 MB per file; phone photos and videos mix).
`.shoebox` is about 1 % today (under 1 GB for 80 GB, measured on a real
library, mostly thumbnails, see the guide), so roughly 1.2 GB for this drive.
Estimated share of photos with readable text: 10–20 % (screenshots, documents,
signs, menus, tickets); 15 % is used below: 7,500 photos, about 15 lines each.

| Item | Calculation | Size |
|---|---|---|
| `text_looked`, one row per photo read (with or without text) | 50,000 rows × ~40 bytes | ~2 MB |
| `text_lines`, one row per line: box, score, raw text, normalised text | 112,000 lines × ~110 bytes | ~12 MB |
| FTS5 trigram index over the normalised text | ~3.4 MB of text; a trigram index is about 3–4× its text | ~12 MB |
| **Total in `recognition.db`, typical** | | **~25 MB** (range 20–60 MB) |
| Extra for a text-heavy library (half the photos are scanned documents, 40 lines each) | 25,000 photos × 40 lines × ~40 bytes of text, ×4 for the index and rows | up to ~400 MB |
| The Text add-on's models (on the computer or in `recognizer/` next to the program, **not** in `recognition.db`) | detector + Latin recogniser | ~15–25 MB, one time |

So for 100 GB of photos with a normal share of text the drive grows by about
**25–60 MB (2–5 % on top of the ~1.2 GB `.shoebox`, 0.03–0.06 % of the
photos)**; the worst realistic case (a drive full of scanned documents) is
about 0.4 GB. Two things keep it small: the FTS table indexes `text_norm`
only (an external-content table, the text is not stored twice), and nothing
is stored for a photo without text except its one `text_looked` row. There
are no crops and no thumbnails for text: the Text check page cuts its crops
from the original on demand, as the pet check does, and never writes them.

*Search speed.* A text search is one FTS5 query on roughly 100,000 lines,
a few milliseconds for a trigram match; queries under 3 characters fall back
to `LIKE` over `text_norm`, about 10–30 ms for 100,000 rows. The rest is the
hash-to-file-id lookup shared with the people term (loaded once, a few tens
of ms for 50,000 files, cached by `data_version`). Expected: **well under
100 ms**, the same order as a people search, and nothing changes for ordinary
searches, because the text index is not touched unless a text chip is
present. Across several drives the query runs per drive in parallel and the
results are merged by the common timeline.

*Reading time (the real cost).* OCR is heavier than faces. Detection runs on
every photo, recognition only on photos where the detector found text.
Guesses: old Intel MacBook (OpenCV runner) about 0.5 s detection per photo
plus ~1 s more for the 15 % with text, so 50,000 photos take **~8–10 hours**;
a modern Mac or PC with onnxruntime about 0.1 s, so **~1.5–2 hours**. The
faces pass is the comparison: the plan's 4–5 h per 100,000 photos on the old
Mac, i.e. 2–2.5 h for this drive, so expect text to take 3–4× as long as
faces there. It is a background job, resumable, and the search works on what
has been read so far.

**Order of the pass (decided, applies to every run, not only a slow one).**
Text is read where text most likely is, so the first hours already fill the
search with useful hits:

1. screenshots first (the phase 11 score: the user's decision, else
   `screenshots.rs`, the same test as the Type filter), highest score first;
2. then photos without camera data (no `camera`, no GPS, no exposure data):
   scans, exports, forwarded pictures;
3. then everything else, newest first (as faces and pets).

The order is a single `ORDER BY` in the pass's selection, plus a rank column
computed from data already in `files`; it never changes what is read, only
when. Resuming keeps the order (unread photos are picked in the same rank), so
a cancelled run continues with the best candidates.

#### Build order (each step is its own commit, mergeable alone)

1. **Spike on real data, no core changes.** Goal: pick the models and the
   thresholds from numbers, and replace the estimates under "What it costs".
   It runs on **your computers with your photos** (the cloud session this
   plan was written in cannot see the drive); what the repository ships for it
   is a script, and what comes back is a small report.
   - *Photos to provide:* about 60 pictures in one folder, copied from the
     drive (copies, never the originals): ~30 with text (several photographed
     documents and letters incl. one sideways, ~8 screenshots, ~8 street
     signs, shop fronts and menus, a few with German umlauts and ß, one
     handwritten note as a known "should not work") and ~30 without (people,
     landscapes, foliage and brick walls, which tempt false text). An
     `expected.txt` next to them lists, per file, one or two words that are
     really readable; that is all the ground truth needed. Fewer is fine
     (20+20 already shows the picture); more files are not needed.
   - *The script:* `recognizer/text_spike.py <folder> [--runner onnx|opencv]`
     (new, throw-away, not part of the app) loads PP-OCR detector and Latin
     recogniser, runs each picture, and prints one line per file (time for
     detection, time for recognition, number of lines, the text) and a
     summary: seconds per photo, share of pictures with text, hits and misses
     against `expected.txt`, lines found in the no-text pictures (false
     positives) at several confidence thresholds, and the size of the models
     and of the rows it would store. It reads files read-only (the guard
     rules apply: it decodes a copy, writes only its report next to the
     script).
   - *Where it runs:* on the old Intel MacBook (the case that decides whether
     the first run is bearable) and once on a modern machine; the same folder
     both times. Output is pasted back as a text file; no photo has to be
     sent anywhere. If you would rather not run anything, the 60 photos
     alone are enough for a first look in a session that has them.
   - *Decides:* PP-OCR version, whether OpenCV 4.10 on macOS 12 runs the
     models without onnxruntime, default confidence threshold, whether a
     rotated pass is needed in v1, and the real seconds per photo. If the old
     Mac needs more than about 1.5 s per photo on average, say so here and
     decide: accept a long first run (as for faces; the order of the pass
     already puts the likely text first) or skip photos the cheap detector
     scores low.
2. **Protocol 3** ([protocol.md](protocol.md)): hello lists `text` with its
   model id; request task `text`; reply key `text: [{bbox, score, text}]`.
   Additive: a protocol 2 worker is refused with the usual "update the
   recognizer" message, as in earlier bumps. The fake worker returns
   deterministic lines (derived from the file hash), so the core is testable
   without Python.
3. **Core storage and pass** (`core/src/text.rs`): `recognition.db` v6,
   normalisation (one function, used for both index and query, with unit
   tests for ß, umlauts, NFD input), `shoebox recognize --text`, pruning of
   deleted photos like faces, `text stats`.
4. **Search**: the `text` term in `browse.rs` (a set of ids from the FTS
   query, ranking, AND with the other terms, multi-library merge), `text=` in
   the API, the matching line as a snippet in the result. Plain `q=` is not
   changed.
5. **Add-on and Control Panel**: `--text` in `install.sh` / `install.ps1` /
   `fetch-models.sh`, `Installed.text`, the "Recognize text" button with
   progress, Cancel and the grey-out reason, the Add-ons list entry, `text
   stats`; the checklist under "What this means for phase 9" above.
6. **UI**: suggestion row and `text:` chip, caption in grid cells, boxes in
   the viewer, "Text in this photo" in the info panel, launcher button,
   status line. Settings → Calibration gets a **Text check**: the lines with
   the lowest confidence next to their crop, a threshold slider, so the
   quality filter is set by looking, not guessing (same idea as the Face and
   Pet checks).
7. **Docs and checklist**: `docs/phase9.md` (as built, how it was checked,
   real-hardware list), this table row set to done-except-hardware.

#### Tests

- Guard: `core/tests/recognize.rs` gets a text pass with the fake worker;
  sizes, mtimes, created and full hashes of every original unchanged
  (nothing new reads originals: the same decode as faces).
- Normalisation and trigram query behaviour (ß/ss, umlauts, NFC vs NFD, short
  queries, punctuation), including that a query is normalised the same way as
  the index.
- Resumability, `--retry-failed`, a model change redoes only text, faces and
  pets are untouched (and vice versa: the faces and pets passes must not
  delete text rows; every new query over `recog` says which kind it means,
  see the pitfall about `recog.faces`).
- Search: `text:` AND tag AND person, multi-library merge, offline drive.
- The real worker, when `SHOEBOX_RECOGNIZER` is set: a generated image with
  known words (rendered by the fixture script) is found by its words.
  Fixtures: `make-fixtures.sh` gains a rendered "document" and "street sign"
  image (ImageMagick or ffmpeg drawtext, whichever is already in the image).

#### Open questions (answer in step 1, not before)

- [ ] Exact OCR models: PP-OCR version (v4 or v5), size, licence, and whether
      OpenCV 4.10 on macOS 12 can run them without onnxruntime.
- [ ] Throughput on the old Intel Mac and on a modern machine; whether the
      detector-first pass makes the first run over ~100k photos acceptable
      (target: a night, like faces).
- [ ] Is a rotated pass needed in v1? Look at how many real documents are
      photographed sideways.
- [ ] Which languages besides German and English matter on this drive.
- [ ] Real-hardware check: search terms people actually use ("Rechnung",
      "Fahrkarte", a street name, a licence plate), precision of the default
      threshold, and whether the trigram index size on the drive is
      reasonable (estimate: well under 100 MB for 100k photos).

### Phase 5d details (duplicates UI and tag carry-over)

Built, one PR, one commit per step (as built: [phase5.md](phase5.md)). Feedback after 5c.

UI
- Trash dialog: focus the "Move to trash" button when it opens, so Enter confirms.
- Move dialog: checkbox "Keep folder tags". Own tags always move along; checked, the old folder's tags also stay as own tags (default: off).
- Duplicates page: one row per photo instead of one card per copy. Left: one thumbnail and the file name. Right: one compact card per copy with the metadata (resolution, size in MB, folder, tags, capture date). The thumbnail is shown once per group, not once per copy.
- Each copy card has a checkbox "delete this copy". At least one card per group must stay unchecked; the last unchecked one is disabled, so the original can never be deleted.
- Multi-select: select copies across groups and trash them in one action.

Bulk action (same folder)
- Button at the top of the page: remove all exact duplicates (same full hash) that lie in the same folder, without review. The copy with the highest resolution stays (tie: keep the oldest path). Near duplicates (phash) are never touched.
- Copies in different folders always need the user's decision, because the folder carries meaning.

Lower-quality versions (feedback after the first build)
- Three kinds of groups, with a drop-down (check boxes) to choose which are shown: Identical photos; Same photo, different resolution; Similar photos (different shots that look alike: a series, repeated clicks, a burst). Buttons "Clear Same Folder Copies" and "Clear Lower Quality Copies" on the right. Of every photo all files but the best are ticked, and the best is the one with the original name, not "IMG (2)" or "IMG - Copy".
- One row per photo: the same photo at different quality (an original and the smaller copy that came back from a messenger, with another name and resolution) is one row with one thumbnail, one card per file. Photos that merely look alike (other shots of a series) are rows of their own.

Feedback after the first real run
- Different camera numbers (IMG_6620 vs IMG_6621) are never "same photo, different resolution": they are different shots and land in Similar photos, nothing ticked.
- iPhone edits (IMG_E6616 next to IMG_6616) get their own group kind "Original and edited": one card per file labelled Original/Edited, nothing ticked, the user picks (or "Keep both"). Matched by camera number in the same folder with a loose hash limit (`EDIT_BITS`), as a blur moves the hash.
- Similar and edited groups: clicking a picture opens a compare dialog with all photos large, side by side; "Keep this one" ticks all the others for deletion.
- Copies that are surely the same photo as a better file are ticked already. Sure means: identical content, or a picture hash at most 4 bits apart (6 when one of the two lost its capture date), the same shape (a turned copy counts), and no capture time that disagrees; the better file has more pixels, or as many and the capture date the other lost.
- Button at the top: remove all lower-quality versions without review. The better file stays, with the folders, tags and capture dates of the removed ones.

Metadata and tags must not get lost
- Folder tags of a deleted copy are added to the surviving original as own, removable tags (folder tags themselves stay non-removable). Example: "Weihnachten/braten.jpg" survives, the copy in "Kochen" is trashed, so the original gets the removable tag "Kochen".
- Capture date: if the original has none or differs from a copy, take over the existing/oldest date automatically. Stored as an override in `library.db`, never written into the file (originals stay untouched, see principles). Only real conflicts that cannot be merged cleanly ask the user.
- Own tags of the deleted copy are merged into the original as well.

Tests
- Guard stays green (`organize.rs`: only paths change, nothing is modified).
- New: tag carry-over, capture-date merge, "at least one stays" rule, same-folder bulk action only on exact duplicates.

Open
- [ ] Check the new duplicates page on the real drive and the iPad (list in [phase5.md](phase5.md)).
- [ ] Confirm the GitHub Actions run is green.

### Phase 5f details (filter by file type)

Feedback: a type used to be found by typing an extension (".mp4") into the
search box, which matches it as a word in the path: you had to know the
extension, it found only that one, and it also hit folders or tags that
merely contain the text.

Decided: a **check box drop-down "Type"** next to the search box (option A),
nothing else. No suggestions in the search box, no sidebar entries, no
separate extensions (MP4 vs MOV); typing ".mp4" keeps working as before.

- Entries: **Photos** (all stills, a Live Photo's still included),
  **Videos** (stand-alone videos only: the clip of a Live Photo is folded into
  its still and is never listed), **Live Photos** (the stills that have a
  clip). Ticked types are OR; the filter is AND with folder, tags, people and
  text. Nothing ticked means everything.
- **RAW gets no entry** (RAW stays hidden in the UI, see Decisions).
- API: `type=photo|video|live` (repeatable) on the timeline, the tag and
  people suggestions and the common timeline `/api/all/timeline`; an unknown
  value is a 400. The filter lives in the URL hash (`type=video`), so it
  survives reload and back; picking a folder keeps it, "All photos" and
  "Clear all" drop it. Read-only over the index, so no guard change.
- Tests: `timeline_order_filters_and_search` in `core/tests/serve.rs` (types,
  OR, AND with a folder, Live and Videos with the fixtures, 400).

Open:
- [ ] Check the drop-down on the real drive and the iPad (it opens on tap,
      closes on a tap outside, the button shows what is ticked).
- [ ] Confirm the GitHub Actions run is green.

## Build notes and pitfalls (learned in phases 0–4)

- **Pets are rows of `recog.faces` with a species**, and everything that
  deletes or compares faces must say which kind: the faces pass only touches
  `species IS NULL`, the pets pass only the others, clustering and
  suggestions are per model (`pets::Space`), and a decision matches only
  a face of its own kind (`face_decisions.species`). A new query over
  `recog.faces` that forgets this mixes embeddings of different lengths.
- **Embedding scales differ per model.** PP-ResNet50 features give 0.50–0.68
  for different pets (SFace: well under 0.35), so thresholds belong to
  the model (`Space::thresholds`), never to a constant shared with faces.

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
- **Perceptual hashes of synthetic images are flaky.** Smooth gradients or
  blocky test patterns leave most DCT coefficients near the median, so
  noise flips bits. Tests use rings (photo-like structure). Exact copies
  take over their twin's hash instead of re-hashing the JPEG thumbnail.
- **`DynamicImage::thumbnail` enlarges** small images; `thumbs::shrink`
  only ever shrinks.
- **`db::open` marks running jobs interrupted**, which would break a
  running scan if the server used it; `serve` uses `db::open_shared`.
- **`PRAGMA data_version` ignores the connection's own commits.** Caches in
  `serve` are keyed by it plus a counter of the server's own changes.
- **Phases 3+ write originals' paths.** Use `organize.rs` helpers
  (`rename_noreplace`, `Names` for case/NFC collisions, `ensure_folder`);
  never `fs::rename` directly, never copy.
- **Two decoder threads deliver out of order.** `recognize` (like the
  thumbnail pass) handles pictures in the order decoding finishes, so tests
  must not depend on the order (successes can come between crashes).
- **The worker restarts lazily**, on the next picture after a crash; the
  `restarts` count depends on what comes after the last crash.
- **python-build-standalone on exFAT:** its archives contain symlinks, so
  `install.sh` builds the runtime in a temporary folder and copies it with
  `cp -RL`, keeping only `bin/python3`. Use the `install_only_stripped`
  archives: the plain ones carry ~250 MB of debug info, and GNU `strip`
  breaks their binaries.
- **File names lie.** Some exports keep `.HEIC` on a JPEG (43 such files on
  the real drive). Images are decoded and their metadata read by their
  first bytes (`media::content_kind`), not their extension; the stored
  `kind` stays the extension's. thumbs.db v2 retries failed thumbnails, and
  a scan re-reads metadata that failed before.
- **OpenCV wheels and old macOS.** OpenCV 4.11+ has Intel macOS wheels for
  macOS 13+ only; without a wheel pip compiles OpenCV from source (hours,
  usually fails). `install.sh` passes `--only-binary :all:`, so macOS 12
  gets 4.10.0.84, the newest release with a `macosx_12_0_x86_64` wheel.
- **Python prints to stdout.** `recognizer.py` keeps the real stdout for the
  protocol and points `sys.stdout` at stderr; the core skips stray lines.
- **Migrations must tolerate a re-run.** Tests simulate an older database
  by lowering `user_version` on a current one, so a step that creates a
  table uses `CREATE TABLE IF NOT EXISTS` (thumbs.db v3).
- **Turned copies and face sizes.** The rotated pass stores boxes upright;
  a face it found lies sideways, so its width (for the 30 px threshold) is
  the box's height (`faces.rs`, `size_px`).
- **Face ids are not stable.** `recog.faces.id` is a plain rowid: SQLite
  can give a new face the id of a deleted one (the largest), and a model
  change or `--retry-failed` replaces a photo's faces. Nothing the user does
  refers to a face id (decisions are keyed by content and box), caches
  that do (`recog.neighbours`) are dropped with the face, and cluster
  similarities are computed again from the embeddings.
- **SIMD without fast-math.** A plain `iter().map(a * b).sum()` cannot be
  vectorised (float addition is not reordered); `ann::dot` keeps eight
  sums side by side, which the compiler turns into SIMD.
- **Every change bumps `index_version`**, people and decisions included, and
  the status poll then reloads the open view. Pages that change their
  cards in place (Unnamed, a person's faces) are left alone by `reloadAll`,
  or the place and typed names are lost.
- **Cluster numbers change after every decision.** Clusters are a cache
  recomputed from scratch, so actions on a card carry the cluster's own
  `generation` (a hash of its faces), not its number or a list-wide one.
- **Git push** uses SSH via the 1Password agent with the "GitHub" key pinned
  in this repo's `core.sshCommand` (the keychain's HTTPS login belongs to a
  different account, `svenfritschpeers`).

## Rotate (lightbox)

Rotate button (`r` left, Shift+R or Option-click right) in the detail view,
like the Finder's Quick Look. Two mechanisms, chosen by what the file really
is (`organize::turn`, going by content, not name):

- **JPEG: in the file.** `organize::rotate` patches the EXIF Orientation tag
  in place (`orientation.rs`): two bytes, lossless, same size, creation date
  kept; the modification date is the file system's, as in the Finder. Before:
  the file matches the index (size, mtime, full hash if known). After: its
  full hash must equal the old bytes with those two changed, else the old
  bytes are put back. The quick hash changes, so the index record is updated
  and the capture-date override, face decisions (boxes turned with the
  picture) and a person's picture follow to the new key; thumbnails and
  detected faces are made again (next view, next `shoebox recognize`).
  Refused: a JPEG without an Orientation tag (adding one shifts every byte
  after it). The small EXIF thumbnail inside the file is not turned.
- **HEIC and PNG: in shoebox only.** The turn is stored in `library.db`
  (`view_turns`, keyed by quick hash like `taken_overrides`, in
  `userdata.json` v4) and applied when shoebox serves the picture
  (`/thumb`, `/view`: decode, turn, encode). The file is not opened for
  writing. Finder, Explorer, the download button and other apps show it as it
  was. The `?v=` of the picture addresses carries the turn
  (`db::version_of`: 7 hash characters and the turn), so browsers fetch the
  new picture.

**Why HEIC is not turned in the file** (measured on a real iPhone HEIC with
macOS Preview, 4032×3024, 1.91 MB, after one rotation):
- Preview writes a whole new file: 99.9% of the bytes differ, the `ftyp`
  and `meta` boxes are rebuilt, and the size drops 18% (1.56 MB). The pixel
  size is reported swapped (3024×4032), so the turn is baked into re-encoded
  pixels: lossy. The Apple HDR gain map brand (`tmap`) is gone from the
  header, so HDR information is probably dropped too.
- A lossless change would add an `irot` box to the container. Rotation of a
  HEIC lives there, not in EXIF (HEIF readers, libheif and Apple's apps
  ignore the EXIF Orientation of a HEIC). Landscape iPhone photos usually
  have no `irot` box, so adding one grows `meta`, which shifts the absolute
  offsets in `iloc` (every item's position in `mdat`) and means writing a
  new file and replacing the original: against the rename-only rule. The
  gain map is a second image with its own transform that would have to be
  kept consistent. Decided against; no plan to do it.
- Live Photos: the still is turned in the view, the paired video is not.

**Pitfalls of the view-only turn:**
- Face boxes are in the file's orientation; the lightbox turns them for
  display (`turnedBox`), "Add face" is disabled while a photo is turned, and
  face crops (people pages, face check) are cut from the file as it is, so
  they show the unturned photo.
- Duplicate, people and trash pages build their addresses without the turn
  (`take(8)` of the quick hash in `duplicates.rs`, `people.rs`, `faces.rs`);
  the server still turns the picture, but a browser that cached the old one
  keeps it until reload.
- Identical copies (same quick hash) share the turn; a copy that is edited
  elsewhere (new content) starts unturned. The turn is not carried when a
  JPEG is turned in the file, and not removed when the last copy goes.
- Real-hardware check: turn a copy of a JPEG and look at it in Finder and
  Explorer; turn a HEIC and a PNG and check Finder shows them as before.

## Favorites

A favorite is an **own tag called `favorite`** (`tags::FAVORITE`, source
`user`), so it lives in the index, survives moves, rescans and the trash, and
is in `userdata.json` like any own tag. The UI shows it as a heart and hides
the tag itself (info panel chips, tag suggestions); the sidebar's Tags list
has one "♥ Favorites" row instead.

- Heart: top bar of the viewer (also key `f`) and the top right corner of each
  timeline cell (shown on hover, always for a favorite; on touch screens
  always). `POST /api/favorites {ids, on}` calls `tags::set_favorite`. Not over
  all drives (ids are per drive); the filter works there.
- Search, three ways to the same thing: the heart button next to **Type**
  (`fav=1` in the URL hash and the API, a chip "♥ Favorites", AND with the rest);
  the suggestion "♥ Favorites" while typing the start of *favorite*, *favorit*
  or the UI language's word; and plain text, where `tags::is_favorite_word`
  accepts the English and German words (favorite(s), favorit(en)) whatever
  language the UI is in, like the pet words. The tag is stored in English
  only, so "favorit" is not a second tag.
- Typed as a tag name (info panel, bulk "Add tag…", the API), `favorite`, `favorites`,
  `favorit`, `favoriten` (and `favourite(s)`) all mean the heart: `tags::check_name`
  maps them to `favorite`, so there is no second tag.
- The timeline response carries `favs` (ids of the shown items with a heart).
- Only the user's own tag counts; a folder named "favorite" does not make its
  photos favorites. Read-only over originals, so no guard change.
- Tests: `favorites_are_the_own_tag_favorite_and_found_by_heart_or_word` in
  `core/tests/tags.rs`.
- Real-hardware check: heart a few photos on the iPad (the corner heart is
  tappable, the button and suggestion filter) and look at the heart in the
  dark theme.

## Trash is opt-in

Moving photos to the trash is off by default, because shoebox's key goal is to
leave originals alone. The library setting `allow_trash` (Settings, "Allow
move to trash"; `/api/allow-trash`) turns it on. Only then does the photo view
show the trash icon (top bar, between rotate and download) and the timeline
selection bar offer "Move to trash". The sidebar's Trash entry is hidden while
the setting is off and the trash is empty; it stays while something is in the
trash. UI gating only: the trash API and the duplicates screens are unchanged.
Real-hardware check: with the setting off there is no trash icon or button;
tick it and both appear.

## Before giving shoebox to others

The release archive (`shoebox-macos.tar.gz`) carries `LICENSE.txt` (all rights
reserved, private use; the repository is public only while GitHub Actions
minutes are needed and goes private when development is done) and
`THIRD-PARTY-LICENSES.txt` (generated by `scripts/third-party-licenses.py`,
checked by the release job). Open:

- [ ] **LGPL: recipients must be able to replace libheif and libde265.**
      Both are LGPL-3.0 and linked **statically** into the binary. The LGPL
      does not force shoebox open, but its section 4(d) wants whoever receives
      the program to be able to use a modified version of those libraries.
      With closed source that works in one of two ways:
      1. *(preferred)* ship them as separate files (dylibs) in the archive
         next to the binary, loaded from there (`@executable_path`), with the
         LGPL text and the upstream source URLs. Changes `scripts/build-deps.sh`
         (shared instead of static), `core/build.rs` and the archive layout;
         copies, no symlinks (exFAT, as for the recognizer's Python).
      2. keep static linking and provide the program's object files and a link
         script on request, so the user can link again with their own build.
         Until step 1 is done, `THIRD-PARTY-LICENSES.txt` promises exactly
         this on request.
      Alternative that avoids the libraries on macOS: decode HEIC with the
      system (ImageIO); a larger change (behaviour, no Linux/Windows).
- [ ] Check with a lawyer before shoebox goes to more than friends or is sold
      (this is not legal advice).
- [ ] When the repository goes private: the guide and README no longer link to
      GitHub (they do not any more); releases need the repository's own
      Actions minutes.
- [ ] First release: the job needs internet to fetch the crates' licence texts
      for `--check`; look at the first run.

## Next step

Run the phase 0–4 hardware checklists on the old Intel MacBook and the iPad
(all iPad checks are collected in [ipad-checklist.md](ipad-checklist.md))
(phase 4: `recognizer/install.sh` and a `shoebox recognize` run on the
drive). 5a (reveal), 5b (own tags) and 5c-1 (face check, `--rotated`) are
built; on the drive run `shoebox faces stats`, look through the face check
page and time `shoebox recognize --rotated` (list in [phase5.md](phase5.md)).
5b-2 (search by several tags) is built too. 5c-2 (people, groups,
decisions, clustering) is built; on the drive time the first clustering,
name a few people and look at the suggestions and the "maybe" list, and
mark the known false finds "not a face" (list in [phase5.md](phase5.md)).
5c-3 (the UI) is built: install the protocol 2 recognizer, then go through
the combined 5c-2/5c-3 list in [phase5.md](phase5.md), all in the UI and
from the iPad. 5d (duplicates UI and tag carry-over) is built and comes before
phase 6 (launcher UI, multiple drives, backups, packaging; pets follow in phase 7);
check it on the drive and the iPad (list in
[phase5.md](phase5.md)). 5e (lean `thumbs.db`) is built: update, start `serve`
once (it removes the old crops) and go through its list in [phase5.md](phase5.md).

Pets (phase 7) are built on top of the launcher branch: install or update the
recognizer (`recognizer/install.sh`; `fetch-models.sh` now also fetches
YOLOX and PP-ResNet50), run `shoebox recognize --pets` (or "Recognize pets"
in the launcher), then look at Settings → Calibration → Pet check and name
a few pets under Unnamed → Pets. DINOv2 is still to be dropped in and
calibrated (list in [phase7.md](phase7.md)).

Text in photos (phase 9) is planned, not built: start with the spike in step 1
of "Phase 9 details" (OCR task in the worker, tried by hand on real photos),
before any core change.
