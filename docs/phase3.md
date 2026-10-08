# Phase 3: move, duplicates, self-healing paths

`shoebox serve` can now change the library, on explicit request only:
move photos (with their RAW,
Live Photo and sidecar files), rename or move folders, move photos to a
trash and back, and settle duplicates. It also follows files that were moved
in the Finder while it runs. The integration tests are in
`core/tests/organize.rs`; they drive everything through the web API, like
the UI does.

## Rules for changes (`core/src/organize.rs`)

- **Only `rename`.** On one drive it keeps content, size and every timestamp
  (modified and created). Nothing is copied, rewritten or replaced. After
  each move the file's stamp is compared with the one before.
- **Never overwrite.** `renameat2(RENAME_NOREPLACE)` on Linux,
  `renamex_np(RENAME_EXCL)` on macOS; where the filesystem does not support
  it, an existence check just before. On top of that, names are compared
  NFC-normalised and ignoring case (as exFAT and APFS do), so `img_1.jpg`
  blocks `IMG_1.JPG`.
- **Only what the index knows.** A file moves only while its size and mtime
  match the index; anything else needs a scan first.
- **All or nothing per photo.** A photo moves (or goes to the trash) with
  every indexed file of the same name stem in its folder (RAW, Live Photo
  video, other formats) and its XMP/AAE sidecars. All names are checked
  before the first file moves.
- **Disk first, then the index**, one transaction per photo. If shoebox stops
  in between, the next scan recognises the move.
- **One change at a time, never during a scan.** Changes share one lock with
  the search for moved files, and are refused (HTTP 409) while a scan of
  another process reports progress.
- **Names users type** are trimmed and NFC-normalised. Refused: empty, longer
  than 255 bytes, control characters, `/ \ : * ? " < > |`, a leading or
  trailing dot, and the names the scanner skips (`Thumbs.db`, …).
- After changes, `serve` writes `library.db.bak` when it stops, like a scan.

## Moving and renaming

- **Move:** select photos in the grid (☑︎ in the top bar) or use the info
  panel in the viewer, then *Move…* and type or pick a folder path. Missing
  folders are created; a level that exists in another spelling is reused.
  Photos whose names are taken stay where they are and are listed.
- **Folder rename / move:** the ✎ on the folder filter chip. The new path may
  put it under another parent. Case-only renames (`familie` → `Familie`) go
  through a temporary name, as case-insensitive filesystems may ignore them
  otherwise. Event fields (`YYYY-MM`), tags and paths of everything below
  are updated; ids stay, so tags, decisions and later faces stay with the
  files.
- Index rows that still sit at the new path (records of missing files) are
  dropped; if the index still lists *present* files there, the rename is
  refused until a scan has run.

## Trash

*Move to trash* moves a photo and its companions into
`.shoebox/trash/<batch>/` on the same drive (never the system trash: it
would copy across volumes on some systems, and its timestamps are not
ours). The records go, with their tags and duplicate decisions; the `trash`
table keeps the original path, size, mtime and hashes. *Put back* renames
the batch back (refused if one of the names is taken by now) and indexes the
files again (new ids; the full hash is reused if size and mtime are
unchanged). *Delete for good* / *Empty trash* deletes the files, after a
confirmation. Thumbnails of trashed files are kept until then.

## Import (removed)

The browser upload (⤒ button, drag and drop, `POST /api/import`) was removed:
see "Adding photos" in [plan.md](plan.md). Copy the photos onto the drive in
the file manager and scan; the scan reports copies of files the drive had
(`core/src/arrivals.rs`, `core/tests/arrivals.rs`).

## Duplicates (`core/src/duplicates.rs`)

*Duplicates* in the sidebar. Among what the timeline shows (no RAW, no Live
Photo videos, nothing missing):

- **Identical copies:** same full hash. Shown with a letter per content.
- **Similar photos:** perceptual hashes at most 8 bits apart and different
  content (resized, re-encoded, edited copies, bursts). Files are grouped by
  the pairs between them, so a group may be a chain of similar shots.
- All pairs of distinct hash values are compared, on all cores (~1.4 s for
  100,000 distinct hashes on 4 cores in Docker; POPCNT is used where the CPU
  has it). The result is cached until the index changes.
- Per group: *Different photos* (`distinct`), *Keep all*
  (`linked`; one label for every kind), or *Move to trash* per file. Decisions are
  stored per pair of file ids in `dup_decisions` (survive moves); decided
  pairs are not shown again. Linked versions appear in the viewer's info
  panel.

## Self-healing paths

When a request finds that a file is not where the index says (moved in the
Finder while `serve` runs), the server runs the scan's index step
(`scan::index_library`: walk, move detection by hashes, missing marks) on a
connection of its own, in the background. Requests that arrive meanwhile
are covered by the same search; searches are at least 30 s apart and skipped
while another process scans. Stored thumbnails keep showing in the meantime;
the UI reloads when the index has changed. A thumbnail that fails because
its file vanished is not stored as failed.

## API additions

All changes are `POST` with JSON and need the `X-Shoebox: 1` header (every
non-GET request does, including the PIN login): a web page on another
origin cannot send it without a CORS preflight, which the server never
grants.

| Endpoint | |
|---|---|
| `POST /api/move` `{ids, folder}` | Move photos with companions |
| `POST /api/folders/{id}/rename` `{path}` | Rename or move a folder |
| `GET /api/duplicates` | Undecided groups |
| `POST /api/duplicates/decide` `{ids, decision}` | `distinct`, `linked`, or `null` to forget |
| `GET /api/trash`, `POST /api/trash` `{ids}` | List / move to the trash |
| `GET /api/trash/{id}/thumb` | Stored thumbnail of a trashed file |
| `POST /api/trash/{batch}/restore` | Put back |
| `POST /api/trash/empty` `{batch?}` | Delete for good |
| `POST /api/rescan` | Look for moved files now |

`/api/info` also reports `trash` (photos in the trash); `index_version` is
now a string that also changes with the server's own changes.

## Schema v2

`dup_decisions(a, b, decision, decided_at)` (a < b, file ids) and
`trash(batch, path, path_nfc, stored, kind, size, mtime_ns, quick_hash,
full_hash, deleted_at)`. Older databases are migrated on open.

## Decisions taken in this phase

- **Our own trash folder** instead of the system trash, so "delete" is a
  rename on the same drive and can be undone from the UI.
- **Companions by name stem** (same folder, case-insensitive), not only
  RAW and Live Photo: an `IMG_1.JPG` with an `IMG_1.HEIC` moves as one.
- **Duplicates compare all pairs** instead of an index structure: simple,
  exact, and fast enough at 100,000 photos with caching.
- **No CLI for moves yet**; the UI and API cover it.

## Still to check on real hardware

- [ ] Moves and case-only folder renames on the exFAT drive from the old
      Intel MacBook: timestamps (modified and created) unchanged, whether
      `renamex_np(RENAME_EXCL)` works on exFAT or the fallback is used.
- [ ] Duplicates page on the full library: time of the first load.
- [ ] Self-healing: move a folder in the Finder while browsing.
