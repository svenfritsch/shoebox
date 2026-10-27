# shoebox: plan

Portable photo library manager that lives on the external drive next to the
photos. This is the overview: what shoebox is, how it is put together, and
where each feature stands. Keep it short; read it first.

## Where things are written down

| File | What | Keep it up to date when |
|---|---|---|
| `CLAUDE.md` | the rules every change follows (safety, tests, guide, UI) | the user changes a rule |
| `docs/plan.md` | this overview and the status table | a feature is added or its state changes |
| [docs/decisions.md](decisions.md) | why things are as they are, what was rejected | a decision is made or changed |
| [docs/open.md](open.md) | everything still to do, check or decide | something is found, done or checked |
| [docs/pitfalls.md](pitfalls.md) | bugs whose cause the code does not show | such a bug is fixed |
| [docs/protocol.md](protocol.md) | the recognizer's JSON-lines protocol | the protocol changes |
| `docs/design/<topic>.md` | the plan of something big that is not built yet | while it is built; folded into the files above and deleted when it is merged |
| `docs/guide/` | the user guide (EN and DE), the `guide` skill | anything a person sees or does changes |

There are no per-phase documents any more: a finished plan is history, and
`git log` keeps it. Write down only what a later change needs (a decision and
its reason, an open item, a pitfall), in the file above where it belongs, and
delete what is done.

## Goal and principles

- **No copies, no changes to originals.** shoebox only indexes files by path
  and hash. Originals are opened read-only; the only operations that touch
  them are explicit user actions (move, rename, trash, remove a duplicate, and
  turning a JPEG, which changes the two bytes of its EXIF Orientation tag in
  place). The rules are in `CLAUDE.md`.
- **Timestamps are sacred.** EXIF `DateTimeOriginal` and the file's created
  date never change. Moving within the drive uses `rename`, which keeps all
  timestamps. Every code path that reads originals is covered by the *guard*:
  size, modified and created time and full hash before and after, any
  difference fails (`shoebox probe` at runtime; `scan.rs`, `serve.rs`,
  `recognize.rs`, `organize.rs`, `export.rs` in `core/tests/`).
- **Local only.** No cloud services. The one optional exception: with the Maps
  setting on, the browser loads OpenStreetMap tiles; shoebox stores none.
- **Portable, few dependencies.** One program per system, next to the photos or
  on the computer. Scale: ~100,000 files, 150 GB today, up to 1 TB, on exFAT.
- **Web UI**, so an iPad on the same network can browse the library.

## Architecture

### On the drive

```
/Volumes/<drive>/
  <library>/                     ← user's folders; read-only for shoebox
  .shoebox/
    library.db                   ← SQLite index (+ rotating backup copy)
    thumbs.db                    ← preview BLOBs keyed by quick hash, face crops only for undecided faces and people's pictures
    recognition.db               ← faces, pets, text lines (boxes, embeddings) keyed by quick hash
    trash/                       ← photos moved to the trash (only with "Allow move to trash")
  shoebox-macos/                 ← the release folder, if shoebox lives on the drive (below)
```

### The release folder

One folder per system, with only three things on top, so a person without a
technical background sees what to double-click and nothing else:

```
shoebox-macos/
  Start shoebox.command          ← the only thing to double-click (.sh / .bat on Linux / Windows)
  guide/                         ← shoebox-en.html, shoebox-de.html, their .txt, assets/
  program/                       ← everything else; nobody needs to open it
    shoebox                      ← shoebox.exe on Windows
    libheif.1.dylib              ← next to the program (@executable_path), LGPL: replaceable
    libde265.0.dylib
    recognizer/                  ← install.sh, fetch-models.sh, recognizer.py, README.md
      runtime/<os>-<arch>/       ← after an add-on install: standalone Python (copies, no symlinks: exFAT)
      models/                    ← after an add-on install: ONNX models
    sources/                     ← libheif and libde265 source archives (LGPL)
    LICENSE.txt
    THIRD-PARTY-LICENSES.txt
    ffmpeg                       ← optional, put there by the user
  libraries/                     ← phase 16: indexes kept next to the program (user data, never inside program/)
```

The launcher looks for `program/shoebox` only, checks the two libraries
beside it, and on macOS takes the quarantine mark off the files in `program/`
and `program/recognizer/` before it starts the program: a program started
directly from a browser download is blocked by Gatekeeper when it loads
`libheif.1.dylib` ("cannot check it for malicious software"; seen with
v0.7.4). Everything the program looks up "next to itself" (`recognizer/`,
`ffmpeg`, the libraries) is inside `program/`. User data never is: the
phase 16 `libraries/` folder sits beside `program/`, so `program/` can be
replaced by a newer one.

`scripts/package.sh` makes this folder for every system: the tag release
(`build.yml`) for macOS, `release-by-hand.yml` for Windows (`shoebox-windows.zip`)
and Linux (`shoebox-linux.tar.gz`), both added to an existing release (phase 17).

### Responsibilities

- **Rust core** (`core/`) owns everything stateful: walking, hashing,
  metadata, SQLite, thumbnails, the web UI (`core/web/`, embedded), the
  launcher (Control Panel, `core/launcher-web/`), move/duplicates, backup
  checks, export, and the clustering and matching of recognition embeddings.
- **Python recognizer** (`recognizer/`) is stateless and optional: pixels in,
  boxes, embeddings and text lines out ([protocol.md](protocol.md)). No
  database access, no names. Without it the app works without recognition.
  Add-ons (Faces, Pets, Text, a shared Python runtime) are installed from the
  Control Panel into `program/recognizer/` (`install.sh`, `install.ps1` on
  Windows) and found only there (`recognize::worker_dirs`). The core has a fake
  worker (`shoebox-fake-recognizer`) for tests.

## Status

Every feature below is built and tested in CI. "Hardware" means the check on
the real drives (old Intel MacBook, exFAT, iPad) is still open; the items are
in [open.md](open.md).

| Feature | Since | State |
|---|---|---|
| `shoebox probe`, scan, verify, guard, move detection | phases 0-1 | done, checked |
| Thumbnails, timeline, folders, search, viewer, `--lan` with PIN | 2 | done, checked |
| Move, rename, trash, duplicates, self-healing paths | 3 | done, checked |
| Recognizer worker and supervision; faces, people, groups | 4-5 | done, checked; cluster tuning open |
| Own tags, multi-tag search, Show in Finder, Type filter, favorites | 5 | done, checked; iPad open |
| Control Panel, several drives, backup checks | 6 | done on macOS; Linux open |
| Pets | 7 | done, checked; DINOv2 open |
| English and German | 8 | done; hardware |
| Text in photos (`text:`, `name:`) | 9 | done; hardware, edge-cut rule open |
| Places and maps (opt-in) | 10 | done; hardware |
| Screenshots in the Type filter | 11 | done; thresholds from real files, bulk button open |
| Estimated dates, "Needs a date", date search | 12 | done; hardware |
| Export and Collection | 13 | done; hardware, created date open |
| Backup comparison views | 14 | done; hardware |
| Carry over data, `userdata.json`, Ignore folders | 15 | done; hardware |
| Libraries kept next to the program (read-only) | 16 | done; Windows share open |
| Release folder; Windows and Linux releases by hand | 17 | done; Windows checked, Linux open, cloud-folder check planned |
| Identical photos once in All drives | 18 | done; hardware |
| Groups in the search | 19 | done; hardware |
| Hidden from the timeline | 20 | done; hardware, people counts open |
| Year strip, rotate, decoders per build (AVFoundation, stored browser frames) | - | done; Media Foundation posters planned |

## Licences

The release archive carries `LICENSE.txt` (all rights reserved, private use;
the repository is public only while GitHub Actions minutes are needed) and
`THIRD-PARTY-LICENSES.txt` (generated by `scripts/third-party-licenses.py`,
checked by every release job). libheif and libde265 ship as separate files
with their sources in `program/sources/` (LGPL). What is still open before
shoebox goes to anyone beyond friends is in [open.md](open.md), section 6.

## Browser tests

Decided in PR #144 (October 2026); keep to it when the photo app or its API
changes. Three layers, each checking what the others cannot:

| Layer | Where | What it checks | Runs |
|---|---|---|---|
| API | `core/tests/*.rs` (e.g. `people.rs`) | every endpoint's reply for given bodies and parameters, on a real library: the server's side of each contract the page relies on (e.g. a naming reply's `person_data` equals that person's entry in `GET /people`) | `cargo test`, CI `test` |
| Page, mocked | `core/tests/browser/faces-mocked.mjs` with `fake-api.mjs` | the page's logic: which requests it makes after each action and when, what it shows from a reply (the sidebar's people list), on a fake clock. No shoebox, no build: core/web as it is on disk | ~10 s |
| Page, real server | `faces-e2e.mjs`, `keyboard.mjs` (`make-faces.py`, `make-library.py`, the fake recognizer) | what needs the real server: that the page and the server agree (the sidebar built from replies is what the server then lists), the grouping after a change, pages that need many endpoints (the viewer, a person's page, the grid) | ~20 s |

`run.sh` runs both browser layers; CI runs it in `guide.yml` (every PR that
touches `core/web`, `core/i18n`, `core/tests/browser` or the guide).

Why so: `app.js` is one script of globals that starts itself, so there are no
unit tests of single functions; splitting it into modules only for tests is
not worth it. A page against a fake API in a real browser is the unit test of
the page (fast, exact: the clock moves only when the test moves it, so "nothing
is asked for until the grouping is done" is checked over minutes of page time
at once, and each step of a grouping comes on cue). Against the real server
the same checks need real waits (each "nothing more happens" costs a second)
and depend on how fast the server groups (225 faces take milliseconds), so only
a few tests run there, enough to catch a fake that no longer matches the
server. The real server found what a fake could not: every change to people
asked for a regrouping (so the People page fetches the list once more after
it; now only decisions, a merge and a deletion do, see "Regrouping" in
[decisions.md](decisions.md)), and clusters stop at 100 faces
(`clusters::MAX_CLUSTER`, below `GROW_STEP`). `run.sh` serves with
`SHOEBOX_REGROUP=2,1500,300` (two cards or 1.5 s) so the real server's
regrouping comes within the tests' waits; the fake follows the app's own
rules (5 cards, 30 s) and keeps the counts of the last grouping in
`/api/info`, as the server's cache does.

Keeping them up to date (agents too):

- A change to what the page asks for or does after an action on the Unnamed
  page, the people list, groups or people's pictures updates
  `faces-mocked.mjs`, and, when the page asks for a new endpoint or a reply
  changes, `fake-api.mjs` (the fake answers 404 to anything it does not know
  and the test fails, so a new request does not go unnoticed).
- A change to a reply's shape in `people_api.rs` (or any endpoint the fake
  answers) updates the Rust API test of that endpoint first, then
  `fake-api.mjs` to give the same shape.
- New tests go to the mocked layer unless they need the real server (the list
  above); `faces-e2e.mjs` stays at a handful. No fixed sleeps: wait on the
  page, its requests or the server; a time window only to show that something
  does not happen.
- Prove a new test can fail: `APP_JS=<changed copy of app.js> core/tests/browser/run.sh`
  (or `node core/tests/browser/faces-mocked.mjs` alone) runs the pages on
  that file; change what the test is about and see it fail.
- UI texts come from the page's `tr`, never written out in a test.

