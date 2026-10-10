# Phase 1: scanner and index

`shoebox scan` builds and maintains the index in `.shoebox/library.db`;
`shoebox verify` checks the drive against it. Neither writes anything outside
`.shoebox/`, and both are covered by the guard integration test
(`core/tests/scan.rs`).

## Usage

```sh
shoebox scan "/Volumes/<drive>"            # index, then compute missing full hashes
shoebox scan "/Volumes/<drive>" --quick    # index only; a later scan catches up on hashes
shoebox scan "/Volumes/<drive>" --forget-missing   # also drop records of deleted files
shoebox verify "/Volumes/<drive>"          # re-hash everything, report damage
shoebox verify "/Volumes/<drive>" --limit 5000     # least recently verified first
shoebox verify "/Volumes/<drive>" --quick  # size and date only, no reading
```

The root is the folder that holds the photo folders; the database goes into
`<root>/.shoebox/` (override with `--db`, which must be outside the library
or inside its `.shoebox/`). Stored paths are relative to the root, so the
drive can be mounted anywhere. `verify` exits with status 2 if anything is
missing, changed, damaged, or the database fails its integrity check.

Both commands can be interrupted at any time: work is committed every 500
files or 5 seconds, and the next run continues (full hashes are computed for
whatever still lacks one; verify starts with the least recently verified
files).

## What a scan does

1. **Walk** the library with `stat` only. Skips the macOS/Windows bookkeeping
   names from `classify.rs` and non-media files. Names are kept as found (for
   opening) and NFC-normalised (for comparing).
2. **Moves.** A path the index does not know, whose size matches a record
   whose path has vanished, is quick-hashed. Same quick hash plus same name
   and mtime (a plain `mv` keeps both) is a move. Otherwise the full hash
   must match the stored one; if the record has no full hash yet, the quick
   hash decides. The record keeps its id, hashes and (later) faces and tags;
   only path, folder and folder tags change. This covers renamed folders,
   case-only renames and copy-then-delete.
3. **Everything else.**
   - Same size and mtime: skipped without opening the file.
   - Same size, mtime off by a whole number of quarter hours up to 14 h
     (exFAT written in another time zone): quick hash decides; if equal, only
     the stored mtime is updated.
   - Otherwise: quick hash and metadata are read again and the full hash is
     dropped for recomputation.
   - New: quick hash, metadata, folder tags.
   - Same NFC name in a different Unicode form: only the stored raw path is
     updated.
4. **Missing.** Records whose file is gone are marked `missing_since` and
   kept, so the file is recognised if it turns up elsewhere. `--forget-missing`
   deletes them.
5. **Full hashes** for every present file without one.

Each file's stamp (size, mtime, created) is taken before and after it is
read; a file that changes in between is skipped and reported, and picked up
by the next scan. After the index pass and after hashing, the database is
copied to `library.db.bak` (via a temp file and rename).

## Scan and verify compared

| | scan | verify |
|---|---|---|
| Question | what changed on the drive since last time? | is what the index remembers still true, byte for byte? |
| Reads | `stat` of every file; content only of new or changed files | every file in full (`--quick`: only size and date) |
| Cost | seconds to minutes | as long as copying the whole library |
| Changes the index | yes: adds, moves, marks missing | only `verified_at` |
| Result | one-off: the Moved, Changed, Missing tabs list what *this* run did | the same every time until something changes |

A move is reported by the scan once. The record is repointed to the new path
(same id), so the next scan finds nothing to do and verify has nothing to say:
it checks the record at its new path. Verify only sees a gone path when the scan
did not follow it, namely when no record could take it over, e.g. the
destination already had its own record of identical content (a duplicate was
deleted or merged). The old record then stays, marked `missing_since`, until
`--forget-missing`, and verify meets it on every run. If another present file
has the same full hash, verify reports it as `relocated` (not a failure; the
content is safe); only content with no other copy is `missing` and fails.
Damage (content differs although size and date match) is only ever found by
verify, because a scan trusts a matching size and date.

## Index

Schema in `core/src/db.rs` (`PRAGMA user_version` = 1):

- `folders`: every folder, with `event_year`/`event_month`/`event_name` for
  `YYYY-MM Name` folders.
- `files`: path (raw + NFC), kind, size, mtime/created (ns), quick and full
  hash, capture date (`taken` local wall-clock time, `taken_offset` when the
  file has one; videos are UTC), dimensions, duration, camera, metadata error,
  `missing_since`, `verified_at`. `phash` stays empty until phase 2.
- `tags` / `file_tags`: one tag per folder level (`source = 'folder'`); event
  folders contribute their name without the date (`Urlaub Griechenland`).
- `jobs`: one row per scan, hash and verify run with state, progress and a
  JSON summary. Rows left `running` by a killed process become `interrupted`.

SQLite runs with a rollback journal (no WAL on an external exFAT drive) and
`synchronous=FULL`.

## Decisions taken in this phase

- **Perceptual hash moved to phase 2.** It needs a full decode of every image;
  phase 2 decodes once for the thumbnail and derives the hash from it.
- **Move confirmation shortcut.** Same size + quick hash + name + mtime is
  accepted without a full hash, so renaming a folder of videos does not
  re-read gigabytes. Any difference in name or mtime requires the full hash.
- **Missing records are kept** until `--forget-missing` (phase 3's delete
  action will remove records directly).
- **JPEG/PNG dimensions** come from the image header when EXIF has none.

## Still to check on real hardware

- [ ] `shoebox scan` + `shoebox verify` on the exFAT drive from the old Intel
      MacBook: time for the first scan and for the full-hash pass, and a
      rescan that reports everything unchanged.
- [ ] Plug the drive into a Mac in another time zone (or change the Mac's
      time zone) and rescan: files should count as "time-zone shift", not
      "changed".
