# Decisions

Why shoebox is built the way it is, and what was considered and rejected. One
entry per decision, with its reason, so it is not argued again without a new
reason, and can be argued again when there is one.

**These are current answers, not laws.** When a task runs into a decision that
no longer fits (the reason is gone, the cost has grown), say so and propose the
change; do not quietly work around it. When the user agrees, change the entry
here in the same pull request. The safety rules in `CLAUDE.md` are the
exception: they change only when the user asks.

Details of how a feature works are in the code and in the user guide
(`docs/guide/shoebox-en.html`); the measurements behind the performance rules
are in `git log` (the old `docs/plan.md` and `docs/phaseN.md`).

## Product

- **No import, no upload.** Photos are copied onto the drive in Finder or
  Explorer (which keeps the created date; a browser upload cannot, it never
  sees a path) and a scan finds them. After a scan, `arrivals.rs` lists the new
  files whose content was already on the drive and offers "Delete the new copies".
- **Files are handled by Finder and Explorer, not by shoebox.** No moving or
  copying of photos between drives (Export copies a selection, Carry over data
  carries tags and names). A "keep both" mark for identical copies on two drives
  was dropped for Ignore folders: a list of names stores nothing about photos.
- **Local only.** No cloud service. Maps are the one exception, opt-in: the
  browser loads OpenStreetMap tiles (vendored Leaflet, no CDN), shoebox stores
  none; a tile cache on the drive was dropped (the browser's cache does it).
  Looking up addresses would need a geocoding service: not planned.
- **Web UI** served by the binary, so an iPad on the same network can use it
  (`--lan` with a PIN, sessions do not survive a restart: accepted). Phones
  are not a target; tablet and desktop are.
- **Settings that are about how a person works are kept by the browser**
  (`localStorage`): Maps, Collection tag, export numbering, Ignore folders.
  Settings about a drive (event folder pattern, `allow_trash`, backup role) are
  in its `library.db`.
- **Trash is opt-in** (`allow_trash`, off): the app's key promise is to leave
  originals alone. Our own trash folder (`.shoebox/trash/`, a rename on the same
  drive, undoable), not the system's.
- **Hidden is tidiness, not privacy**: an own tag and a folder flag in
  `library.db`; pages for looking leave hidden photos out, pages for looking
  after the library show everything.
- **Favorites, Collection and Hidden are own tags** (`favorite`, the chosen
  name, `hidden`) shown as a heart, a bookmark, an eye: no new tables, they
  survive moves, the trash and `userdata.json` like any tag. Typed spellings in
  both languages map to the one stored English tag.
- **Groups**: a person is in at most one group; a group is made from a person
  and goes when its last person leaves (no empty groups). A group chip is OR
  inside, AND with everything else.
- **Search terms combine with AND**, always. "Any of" (OR) between chips is not
  planned (a group is the one OR). Text read from photos (`text:`) and file
  names (`name:`) are terms of their own, never mixed into plain words, so a
  word does not drown tag results in photos that merely contain it.
- **RAW files are indexed but hidden** in the UI; they move, trash and export
  with their JPEG/HEIC of the same stem. The Type filter has no RAW entry.
- **Live Photos** are folded into their still. Pairing is name *and* moment
  (`live.rs`: same folder and stem, video at most 6 s, capture dates within
  10 s): cameras reuse numbers, so the name alone paired the wrong video.

## Library and dates

- **Folders**: an event folder (`2020-07 Name`; the forms the scanner reads are
  in `library::parse_event`) dates the photos without a capture date; any other
  folder is a topic. Every path level is a tag. The event pattern setting only
  decides what the app creates; the scanner reads every form.
- **Which date a photo has**, first that exists: the user's estimate (year,
  month or day; wins even over EXIF, because a scan of a print has the scan's
  date), the capture date in the file, the event folder's month, the earlier
  of created and modified. The file's own date stays visible ("EXIF" row).
- **Folder tags cannot be removed** (they come from where the file is; move
  the photo instead). Moving keeps own tags always; "Keep folder tags" (off)
  keeps the old folder's tags as own tags. Removing a duplicate gives its
  tags, dates and position to the copy that stays.
- **User data lives in `library.db`, keyed by content** (quick hash, plus the
  box for faces), never by row id or path, so it survives moves, rescans and
  model changes; `recognition.db` and `thumbs.db` are caches that can be
  deleted. `userdata.json` is the readable backup of the user data.
- **Turning a picture**: a JPEG with an Orientation tag gets those two bytes
  changed in place (lossless, hash checked after). Everything else (HEIC, PNG,
  a JPEG without the tag) is turned in shoebox only (`view_turns`). HEIC in the
  file was rejected: rotation lives in an `irot` box, adding one shifts every
  offset in `iloc`, so the file would have to be rewritten and replaced (and
  the HDR gain map kept consistent); macOS Preview re-encodes it lossily.

## Architecture

- **Rust core, one binary per system**: scanner, SQLite, web server, UI
  (embedded with `rust-embed`). Few dependencies; each crate enters
  `THIRD-PARTY-LICENSES.txt` and the static build.
- **The index lives on the drive** (`.shoebox/`), so a drive carries its own
  library. **A library kept next to the program** (`libraries/<name>/`,
  registered in `libraries/libraries.json`) is for folders shoebox must not
  write into (a server share): it is read-only for shoebox, always, not a
  switch. With an unreadable registry everything stops (fail closed).
- **Three databases**: `library.db` (index and user data), `thumbs.db`
  (previews as BLOBs: one file per thumbnail would waste ~5x on exFAT's 128 KB
  clusters), `recognition.db` (faces, embeddings, text; its writes do not move
  the index version the UI watches). Thumbnails are never written into EXIF.
- **SQLite on exFAT**: rollback journal, `synchronous=FULL`, a copy
  `library.db.bak` after each scan and after changes. WAL stays off (its `-shm`
  file on external exFAT drives, and copies of `.shoebox` without the `-wal`).
- **One shared connection per drive in `serve`** (`App::conn`), so a slow
  request holds up every other: the page asks only on actions, never polls
  (except `/api/info` every 5 s while a job runs), calls off what it no longer
  needs (`AbortController`, `dropPending`), the search box sends one
  `/suggest` request per pause (150 ms), and the server takes those one at a
  time in order, dropping requests called off while they wait. Heavy reads
  (the people view) run on a read-only connection of their own, invalidated by
  `PRAGMA data_version`.
- **Several drives in one server**: every API route carries the library id
  (file ids are per database); offline drives answer 503 for their own routes
  only. People match by name across drives. A drive is a backup or a separate
  drive (stored role; suggested as backup at 90 % overlap); backups are left
  out of duplicates and refuse Carry over. In All drives a photo with the same
  name and full hash on several drives shows once; All drives is read-only.
- **Recognition is an optional Python worker** (`recognizer/`): stateless, JSON
  lines over stdin/stdout ([protocol.md](protocol.md)). The core decodes the
  photo and sends pixels, so the worker never opens an original and HEIC needs
  no Python support. It runs only on request (`shoebox recognize`, launcher
  buttons), never from a drive's `.shoebox/` (a drive from somebody else could
  bring code). Add-ons are decided by the files present, no manifest.
- **Windows add-on installer in PowerShell** (`install.ps1`, mirroring
  `install.sh` line by line) rather than download and unpack code in Rust: no
  new crates, PowerShell 5.1 and `tar.exe` ship with Windows 10.
  `core/tests/recognize.rs` keeps checksums and versions of both in step.
- **Decoders**: libheif and libde265 on all three systems, as separate library
  files next to the program (LGPL: replaceable), so previews and perceptual
  hashes are the same whichever computer made them. Video posters: the
  system's framework (AVFoundation on macOS), else an `ffmpeg` the user puts
  next to the program (GPL, 80-100 MB, never bundled), else a frame the browser
  grabbed, re-encoded and stored. No transcoding: a video the browser cannot
  play gets a notice and "Show in Finder".
- **Release folder**: three things on top (`Start shoebox.*`, `guide/`,
  `program/`), `libraries/` beside `program/` so `program/` can be replaced.
  The tag release is macOS only; Windows and Linux are built by hand
  (`release-by-hand.yml`) and added to a release.
- **CI until v1.0**: macOS `test` and `build`, skipped for docs-only changes;
  the guide's screenshots and the browser tests run on Linux in `guide.yml`.
  Screenshots and `.txt` copies of the guide are made by CI, not committed.

## Recognition

- **Faces**: YuNet (detect) and SFace (128-d) via OpenCV. Detection score 0.85,
  faces under 24 px dropped, under 30 px listed but not clustered (all faces of
  30-40 px on the real drive were real people). A face is suggested for a person
  at cosine 0.55 (the highest wrong match seen was 0.54). Clusters join only at
  an average similarity of 0.55 (`clusters::COHESION_SIM`) and stop at 100
  faces; they are a cache recomputed from scratch, so card actions carry the
  cluster's `generation`, never its number.
- **Regrouping when it pays off**, not after every name: after 5 cards or 30 s
  without a decision (and at once after new faces, a merge, an undo); only the
  suggestions that can have changed are searched again. Keeping embeddings in
  the server between runs (~200 MB) was decided against: nobody waits for it.
- **Pets**: YOLOX finds cats and dogs (the whole pet, not its face), an image
  embedder describes them (PP-ResNet50; DINOv2 if its file is present). Pets
  are rows of `recog.faces` with a species and ordinary "people"; every query
  over faces says which kind, embeddings of different models are never
  compared, thresholds belong to the model (`pets::Space`).
- **Text in photos**: PP-OCR via `rapidocr-onnxruntime` in the worker (Apache-2.0,
  models in the wheel); not Tesseract, Apple Vision, EasyOCR or a cloud API.
  Only Latin letters, digits, umlauts and ß are kept (the model also reads
  Chinese from icons); `β` and a capital B inside a word are read as ß. Index:
  FTS5 trigram over a folded form (substrings match, glued words still match,
  language-free). Read order: screenshots, then photos without camera data,
  then newest first. Lines a user hides are user data in `library.db`.
- **Screenshots** are a score from metadata and a pixel check of the thumbnail,
  no model; the user's mark wins. "Photos" in the Type filter means stills that
  are not screenshots.

## Export and carry over

- **Export copies, never moves**, into a new folder outside the library, every
  read under the guard; byte-identical copies with the original's modified
  time; numbered by capture date (`001_IMG_3457.jpg`) unless switched off; RAW
  and Live Photo videos only when ticked; HEIC converted to JPEG by default.
  An existing export folder is merged, never replaced. Only for a browser on
  this computer (it writes to this computer's disk).
- **Carry over data** reads another drive's `library.db` (or a `userdata.json`)
  and adds to this drive's: matched by content, the target's data always wins,
  one transaction, run again is a no-op, backups refused, missing people
  created (no group).

## Front end

- **One page, no build step, no framework**: `core/web/` is served as it is on
  disk; `app.js` is a script of globals that starts itself, with
  self-contained parts in their own scripts (`dates.js`, `geo.js`,
  `export.js`, `tip.js`). Vendored libraries only (Leaflet).
- **One set of common controls** in `app.css` (sizes, radius, focus ring,
  accent) used by every screen; bigger controls for `pointer: coarse`.
  Details in the `frontend` skill.
- **Icons are our own line drawings** (`lineIcon`), visibly different from any
  icon set; no emojis in the pages.
- **Two languages** (English, German with informal "du"), flat JSON message
  files, `tr()` / `trn()` / `data-i18n`, English as fallback. Messages made by
  the Rust side (API errors, job labels, CLI) are not translated yet.
- **Browser tests in three layers**: Rust API tests for every endpoint; the page
  against a fake API on a fake clock (`faces-mocked.mjs`, the unit test of the
  page); a handful against the real server for what the fake cannot show. See
  "Browser tests" in [plan.md](plan.md).
