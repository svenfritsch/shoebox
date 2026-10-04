# Phase 4: recognizer and worker supervision

`shoebox recognize <root>` finds the faces in every photo and stores a box
and a 128-number embedding per face in `.shoebox/recognition.db`. Naming
people, clustering and the correction UI are phase 5; this phase delivers the
data they work on and the machinery around the worker.

The worker is the optional Python `recognizer/` (OpenCV: YuNet to detect,
SFace to embed), started and supervised by the Rust core. The protocol is in
[protocol.md](protocol.md).

## Installing the recognizer on the drive

```sh
recognizer/install.sh /Volumes/Fotos     # the library root, scanned before
```

This puts a standalone Python (python-build-standalone 3.12), OpenCV, numpy,
`recognizer.py` and the models into `/Volumes/Fotos/.shoebox/recognizer/`
(~330 MB on Linux), with the runtime under `runtime/<os>-<arch>/`. Nothing is
installed on the computer. Run it once **on each kind of computer** that will
run recognition (Intel Mac, Apple Silicon Mac, Linux): pip picks the OpenCV
and numpy builds that this computer's system can load. Symlinks are resolved
while copying (exFAT has none).

For development, any Python with OpenCV works:

```sh
pip install opencv-python-headless numpy
recognizer/fetch-models.sh                 # into recognizer/models/
shoebox recognize <root> --recognizer recognizer/recognizer.py
```

## Running

```sh
shoebox recognize /Volumes/Fotos
shoebox recognize /Volumes/Fotos --limit 500     # a first taste
shoebox recognize /Volumes/Fotos --retry-failed  # try failed photos again
```

- JPEG, PNG and HEIC, one per content (copies share their faces), newest
  first. RAW files (covered by their JPEG/HEIC twin) and videos are skipped.
- **Resumable.** Results are committed every few seconds; an interrupted run
  continues where it stopped. Ctrl-C ends a run at once and marks it
  interrupted, so the next one can start right away (the worker runs in its
  own process group and is stopped by shoebox). A run killed otherwise
  counts as running for 2 minutes. Photos looked at with the same model are not
  looked at again; results of another model are redone (embeddings of
  different models cannot be compared).
- **Originals are only read.** shoebox decodes each photo itself (HEIC
  included, EXIF orientation applied) inside `fingerprint::read_unchanged`
  and sends the worker an upright JPEG of at most 1600 px. The worker never
  opens an original; a photo that changed since the last scan is skipped.
- Runs alongside `shoebox serve`. Unlike a scan it does not block moves,
  imports or the trash, and its progress lives in `recognition.db`, so the
  web UI does not reload while it runs. The status line shows
  "finding faces N%"; the viewer's info panel shows the number of faces and
  can draw their boxes (*Show*).
- Faces of files that are gone (not in the index, not in the trash) are
  dropped at the end of each run. `recognition.db.bak` is written after it.

## Supervision

| Situation | What happens |
|---|---|
| Worker not installed | `recognize` says so; everything else works as before |
| No hello within 2 min, exits at start, other protocol | Run stops with the reason |
| Worker answers with an error | Recorded as failed for that photo; next photo |
| Worker exits or does not answer within 2 min | Killed, restarted, photo tried once more; a second failure is recorded (`recognizer crashed: …`) |
| 5 crashes in a row without a reply in between | Run stops; results so far are kept |
| Lines on stdout that are not replies | Logged and skipped |
| shoebox cannot decode a photo | Recorded as failed; the worker never sees it |

Failed photos are not tried again unless `--retry-failed` is given.

## Fake worker

`shoebox-fake-recognizer` (`core/src/bin/`) speaks the protocol without
Python: one face in the middle of every picture, an embedding from its mean
colour, and misbehaviour on cue (solid red: crash, green: error, blue: hang,
magenta: garbage, black: no faces). The integration tests
(`core/tests/recognize.rs`) use it for the guard, the supervision table
above, limits, model changes, pruning and the web API. With
`SHOEBOX_RECOGNIZER` pointing at `recognizer/recognizer.py`,
`real_recognizer_runs_under_the_guard` also runs the real worker over the
fixtures (CI does, on Linux, with a photo of a face).

The Python side has its own tests:

```sh
SHOEBOX_TEST_FACE=face.jpg python3 -m unittest -v recognizer/test_recognizer.py
```

## recognition.db (schema v1, attached as `recog`)

- `jobs`: like `library.db`'s, for `faces` runs.
- `looked(key, task, model, width, height, found, error, done_at)`: one row
  per content (`files.quick_hash`) and task.
- `faces(id, key, model, x, y, w, h, score, landmarks, emb)`: box and the
  five landmarks as fractions of the upright photo, YuNet's score, and the
  SFace embedding (128 little-endian f32, L2-normalised: cosine similarity
  is the dot product).

## API additions

- `/api/info` has `faces: {done, total, faces, running}`.
- `/api/files/{id}` has `faces`: `null` until the photo has been looked at,
  else `[{x, y, w, h, score, landmarks}]`.

## Decisions taken in this phase

- **shoebox decodes, the worker gets pixels** (base64 JPEG in the request),
  not paths: HEIC works without HEIC support in Python, the guard stays in
  Rust, and the worker cannot touch originals even by mistake.
- **A separate `recognition.db`**, keyed by quick hash like `thumbs.db`:
  keeps ~80 MB of embeddings (150,000 faces) out of the index and its
  backups, and its writes do not change the index version the UI watches.
- **One worker, decoding on two threads alongside.** OpenCV uses all cores
  inside the worker already. About 70 ms per photo on 4 cores in Docker.
- **A run on request** (`shoebox recognize`), not part of `scan` or started
  by `serve`; a button in the UI can come with phase 5.
- **Detection threshold 0.85, faces under 24 px dropped** (in the 1600 px
  copy): fewer false faces for clustering; the score is stored for stricter
  filtering later.

## Still to check on real hardware

- [x] `recognizer/install.sh` on the old Intel MacBook (macOS 12.7.6,
      x86_64), installed next to the app with `--recognizer`: 292.6 MB.
      pip found no wheel of the newest OpenCV for macOS 12 (4.11+ needs
      macOS 13 on Intel) and started compiling it; with `--only-binary`
      it takes OpenCV 4.10.0.84 and numpy 2.5.3, which work. Two bugs
      found and fixed on the way: `$PLATFORM…` broke bash 3.2's parser,
      and the compile took hours.
- [x] Time per photo on the old Intel MacBook: 2.1–2.5 photos/s at first,
      5.7/s on the second run (warm cache): 7196 photos in about 20 min.
- [x] Results over the family folder (8002 files, 7196 photos looked at):
      9564 faces (1.3 per photo), 48 failed. Face widths in the ≤1600 px
      copy: 296 under 30 px, 1828 of 30–60 px, 2969 of 60–120 px, 4471
      larger. Scores all ≥ 0.85 (the detector's own cut-off is 0.9), so
      the score does not separate good from bad detections.
- [x] Spot-check boxes in the viewer: boxes that are found sit correctly,
      HEIC and EXIF-rotated photos included. **Faces lying sideways (people
      lying down, ~60–90° roll) are missed**: YuNet finds faces up to about
      30–45° of roll. Handled in phase 5 (`--rotated`, manual faces).
- [x] The 48 failures: 43 JPEGs named `.HEIC` in `2010er/` ("No 'ftyp'
      box") and 5 `.jpg` files that are not JPEG. Images are decoded by
      content since the fix that followed; `shoebox recognize --retry-failed`
      redoes them.
- [x] Interrupt a run (Ctrl-C) and start it again: it continues, but only
      after 2 minutes ("another `shoebox recognize` is running") and with a
      Python traceback. Fixed right after: Ctrl-C now ends the run cleanly.
- [ ] `shoebox verify` after the run (was running at the time of writing).
