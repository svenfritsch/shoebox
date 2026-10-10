# Phase 6: launcher UI, multiple drives, backups, packaging

Status: planned. Overview and checklist in [plan.md](plan.md).

Phase 7 is pets (formerly phase 6). Fallback: if the multiple-drive part gets
much bigger than planned, only that part moves to a phase 8; launcher and
backups stay here.

## 1. Library id in all API routes (first)

File ids are assigned per database, so ids from two drives collide. Every
route carries a library id (`/api/lib/{lib}/files/{id}` and so on); the UI
keeps the id with every file reference. Built before anything else in this
phase, with one library, so the single-drive behaviour stays testable.

Built: all routes except `/api/session`, `/api/login` and `/api/libraries`
live under `/api/lib/{lib}/…` (the other docs still write the short form).
`serve::scope_request` rewrites the path before routing and marks the request,
so the handlers are unchanged for now; `guard` answers 404 to a library route
without the id, an unknown id matches nothing. The id is eight hex digits of
the BLAKE3 hash of the NFC folder name (`serve::library_id`). `/api/libraries`
lists `{id, name, online}`; the UI builds `LIBAPI` from it. Test helpers add the
prefix themselves (`/raw/…` sends a path as written). Next step: an `App` per
library and the id choosing between them.

## 2. Launcher UI

Goal: people who are not technical can use shoebox without a terminal.

- Double-click on the binary (no arguments) starts launcher mode, independent
  of `serve`, and opens the page in the browser. No library has to be running.
- Embedded web page (rust-embed), no native GUI: same on every OS, lean binary.
- Field for drive and photo folder path (suggesting detected drives);
  buttons Scan, Verify, Recognize, Recognize pets (phase 7: the same, then cats and dogs), Face Stats. "Start photo app" starts
  `serve`, and only then.
- Results are visual: progress bars and a result list showing per file
  whether it worked or failed (scan, verify).
- CLI commands return structured results (JSON); CLI and UI use the same
  logic.
- Safety: localhost only, `X-Shoebox` header on non-GET requests, guard tests
  cover every new path.
- macOS: Gatekeeper blocks a double-clicked binary, so `Start shoebox.command`
  stays as fallback. To check: the best behaviour on Windows and Linux.

Built (first slice, `core/src/launcher.rs`, page in `core/launcher-web/`):
- `shoebox` without arguments starts the launcher on `127.0.0.1:7879` (next
  free port if taken) and opens the browser. Own small axum server, no
  library needed. The page offers a path field with detected drives
  (`/Volumes`, `/media`, `/mnt`, Windows drive letters, and the drive the
  binary sits on when it is in `<drive>/.shoebox/bin/`), buttons Scan, Verify,
  Recognize, Face stats, a progress bar, a per-file result list (✓/✗, "only
  problems" filter) and a summary. "Start photo app" starts `serve` in-process
  only when pressed (`POST /api/app`).
- One field for "drive or photo folder": the folder that holds `.shoebox` and
  the photos, which is how the scanner works (a separate photo folder inside
  the drive is not supported yet).
- `core/src/report.rs`: the commands print through `say!` and report
  `Event::{Line, Progress, File}` to an installed sink (the launcher's job
  state); the CLI output is unchanged. `--json` on `scan`, `verify`,
  `recognize` and `faces stats` prints the result on standard output and the
  usual text on standard error. Launcher and CLI call the same functions.
- Safety: peer and `Host` header must be local, non-GET needs `X-Shoebox`,
  one job at a time (409 otherwise). `core/tests/launcher.rs` runs scan,
  verify, face stats and recognize through the API under the guard
  (snapshot of size, mtime, created and hash before and after).
- Several folders: the path field holds chips (Enter adds, × removes), each
  with a check box. A command runs over the ticked folders only, one after the
  other, with a result list and a summary per folder; "Start photo app" opens
  the ticked ones too. The "Backup check" button is enabled only with exactly
  two ticked folders (first = original, second = copy), and the others only
  with at least one. The list is remembered in a JSON file in the user's
  config folder (`~/Library/Application Support/shoebox/launcher.json`,
  `~/.config/shoebox/launcher.json`, `%APPDATA%\shoebox\launcher.json`, or
  `$SHOEBOX_CONFIG`): `{ "paths": [ … ], "checked": [ … ], "history": [ … ],
  "backups": { "<backup>": { "of": "<original>", "at": <unix seconds> } } }`.
  `checked` is absent while everything is ticked; `history` holds every folder
  ever added, newest first, and is offered in the drop-down again;
  `backups` is written by the launcher only (see below), never by the page.
- Cancel: the same as Ctrl-C on the command line. The command stops between
  files, keeps what it committed and the job is marked interrupted, so the next
  run continues (`report::request_cancel`, checked by scan, hashing,
  thumbnails, verify and recognize).
- While a command runs, and while the photo app runs, all inputs and buttons
  that start or change something are disabled (the folders in a disabled
  `fieldset`, the command buttons one by one); the server refuses them too
  (409). Only Cancel and "Stop photo app" stay usable.
- **Exception: recognition** (`recognize`, `recognize_pets`, `faces_stats`).
  It may run while the photo app is open, and the photo app may be started
  while it runs, so after a scan the person can browse while the
  faces are found. This is safe because `serve` already tolerates it: both
  open the databases shared (`db::open_shared`, 10 s busy timeout), `serve`
  skips its own clustering and drawn-face embedding while a recognition job
  is running (`recognize::running`), recognition does not block moves or tags
  and a file moved or changed meanwhile is skipped by the guard.
  Scan, Verify and Backup check stay locked while the app runs
  (`launcher::runs_beside_app`; test
  `recognition_runs_while_the_photo_app_is_open`).
- "Start photo app" opens every chosen folder as one library (see section 3).
- Start scripts for the double-click fallback are in `scripts/launchers/`
  (`Start shoebox.command`, `Start shoebox.sh`, `Start shoebox.bat`); the
  release tarball ships the `.command` file.
- Not yet done: the platform checks below.

Platform behaviour to check on real machines (not tried yet):
- macOS: a double-clicked binary is blocked by Gatekeeper; `Start
  shoebox.command` stays the way in (it only has to start the binary).
- Windows: a double-click opens a console window and runs the launcher;
  SmartScreen warns about unsigned downloads once.
- Linux: file managers often open an executable in an editor or ask; a
  `.desktop` file or `Start shoebox.sh` with "run in terminal" is the
  fallback.

## 3. Multiple drives

Built:
- `shoebox serve <root> [more roots…]` and the launcher open one `App` per
  drive behind a hub (`serve.rs`: `Hub`, `Slot`). `/api/lib/{id}/…` goes to
  that drive; `/api/libraries` lists `{id, name, online, reason}`. A drive that
  is not there answers 503 `{"offline": true}` for its own routes only; a
  drive that disappears while the server runs closes (checked at most every
  2 s), one that returns opens again by itself. Background work uses
  `db::open_existing`, so an unplugged drive's `.shoebox` folder is never
  created again on the computer's own disk.
- `multi.rs` (read-only, `ATTACH`): `/api/all/people` (same NFC, case-folded
  name = same person; names, groups and decisions stay per drive),
  `/api/all/duplicates` (full hashes over all drives that are not backups),
  `/api/all/drives` (roles and suggestions), `POST /api/all/role`.
- Backup or separate: the role is stored per drive in `library.db`
  (`settings`, schema v5). Undecided drives are *suggested* as backups when
  at least 90 % of their contents are on another drive and they are not the
  bigger one; two drives that hold the same are both suspected until one is
  marked. Backups and suspects are left out of the duplicate list, with the
  reason shown.
- UI: drive list in the sidebar (online/offline/backup), an "All drives"
  page (roles with confirm buttons, duplicates across drives, people across
  drives), an offline page for the selected drive that opens it again when it
  comes back. Tests: `core/tests/multi.rs`, `core/tests/serve.rs`.
- Manage drives page: every drive is a card (same height, a status strip on
  top, the role menu in the name row, the disk name and folder from
  `volume.rs`, a footer with "Scanned" and "New files" / "Last backup"). A
  backup follows its original, has a blue outline and shows a progress bar
  plus the lists "not on the backup yet" / "only on the backup", each file with
  a "show in Finder" button (`POST /api/all/reveal`: drive and path, only a
  file in that drive's index, only from this computer). Below: tabs for people
  and duplicates across drives. The disk name comes from the path
  (`/Volumes/<name>`, `/media/<user>/<name>`, `/run/media/…`, `/mnt/<name>`) and,
  on Windows, from the volume label (`GetVolumeInformationW`; not tested in CI,
  which does not build Windows yet).
- Common timeline ("All drives" in the drive list, `GET /api/all/timeline`):
  one list, newest first, over the drives that are online and not backups
  (nor suspected ones). Filters name things instead of numbering them
  (`tag=Winter`, `person=Anna`, `q=…`), because ids differ per drive; a drive
  that lacks one of the names has no match. An item's id is
  `drive * 2^40 + id` and the drive is its position in the `libs` the server
  sends along; `/api/all/tags` and `/api/all/people` feed the search
  suggestions. It is for looking: no selecting, importing or trash; hearts show
  only on favorites (disabled buttons, no outline heart to click); the info
  panel shows the drive, the favorite, tags and people read-only (no
  add/remove, no face editing) and a button that opens the photo's own
  drive for changing them and moving. Searching for a person who exists on two drives
  gives the photos of both in one timeline (`core/tests/multi.rs`).
- Duplicates screen: with several drives it has two tabs, "On this drive"
  (the existing groups and decisions) and "Across drives" (same content on
  different drives, side by side, with "Move to trash" per copy and a note
  for drives left out as backups). In "All drives" only the second exists.
- Not yet done: moving files between drives, a decision "keep both" for
  copies on different drives, the 90 % threshold on real data, counts in the
  search suggestions that narrow with the filter (they are per name, over all
  drives). Identical photos on two separate drives show twice in the common
  timeline.

- Every drive keeps its own `.shoebox` folder with `library.db`, `thumbs.db`
  and `recognition.db`. The app opens several libraries at once with one
  shared experience; the UI takes two or more paths.
- People are matched by name: the same name on different drives is the same
  person. Groups, names and decisions stay in the drive's own `library.db`.
- Duplicates across drives are found by comparing hashes: SQLite `ATTACH`
  opens the other drive's `library.db` in the same connection, so one query
  compares the full hashes without reading any photo.
- Backup drive or separate drive: a drive that is a backup must never produce
  duplicate suggestions. A drive counts as a backup when the user marks it so,
  or when nearly all its hashes (threshold to be chosen, e.g. 90 %) already
  exist on another drive; the UI then asks once to confirm. Backups go through
  the backup verification (section 4) instead; duplicates are only proposed
  within a drive and between drives that count as separate.
- Offline drives: no request may fail the application; the drive's routes
  answer "offline" and the UI greys it out until it is plugged in again.
- An unplugged drive shows as "offline"; the rest keeps working.

## 4. Backups and packaging

Backup verification, built (`core/src/backup.rs`, `multi::backup_report`):
- A backup drive is an ordinary drive with its own `.shoebox` (scanned after
  every backup run by rsync / Carbon Copy Cloner), marked "backup" (see
  section 3). The drive it copies is the one the user named
  (`POST /api/all/role` with `of`), else the one that holds most of its
  contents.
- `shoebox backup <original> <backup> [--deep] [--json] [--limit N]`, the
  launcher button "Backup check" (first ticked folder = original, second =
  backup)
  and `GET /api/all/backups` give the same report from the two indexes, by
  full hash, without reading any photo: files new since the last backup (not
  on the backup), files at the same path with other content, content only the
  backup has (gone or changed on the original since; informational), files of
  the original without a full hash yet, and "last backup N days ago" (the
  backup index's last scan and the day files last arrived on it). Exit
  status 2 if something is missing or different.
- A clean check (`ok`: nothing missing, different or unhashed) marks the backup
  folder in the launcher: its chip shows "backup of <drive>" (the mark is in
  the launcher's config, keyed by the folders as written; any later check that
  is not clean takes it away). Result rows of the backup check have a "show
  both" button that opens the file on the original and on the backup in
  Finder / Explorer (`POST /api/reveal`: only folders of the job on screen,
  only paths inside them).
- Copies removed on the duplicates screen. Deleting there moves a copy into
  `.shoebox/trash/<batch>/` on its drive (one batch per photo with its RAW,
  Live Photo and sidecar files) and the `trash` table keeps what is needed to
  put it back; emptying the trash deletes those files and their batch folders
  (the `.shoebox/trash` folder goes with the last one) and the table rows. So
  that the backup can still be told, `duplicates::remove_copies` also writes
  a row to `removed_copies` (library schema v9: the removed copy's path, size
  and content hash, plus the hash of the copy that stays; no file content),
  which emptying the trash does not touch. A plain "Move to trash" of a photo
  is not recorded: it may be the only copy.
  The backup check lists, as "removed here, still on the backup", the
  backup's files that match such a row by path and content and are safe to
  remove: the kept content is still on the original, the original has no such
  file at that path again, and the backup keeps another file with the kept
  content (`multi::removable_on_backup`). They do not make the check fail: a
  backup that holds more is still complete.
  `backup::cleanup` (launcher: "Delete duplicates from backup as well", job
  `backup_cleanup`) removes them from the backup one at a time, asking again
  before each (`multi::still_removable`, so the last copy of a content on the
  backup can never go), through `organize::trash_files` on the backup's own
  index: they land in the backup's `.shoebox/trash`, or are deleted for good
  with "Delete for good". A file that no longer matches the backup's index is
  skipped with a reason. Nothing on the original changes. Tests:
  `core/tests/backup.rs`, `core/tests/launcher.rs`.
- `--deep` (launcher: "also re-read the backup drive") then runs `verify` on
  the backup drive: bit rot shows as DAMAGED although size and date match.
- UI: the "All drives" page shows a status box per backup drive with the
  lists; the photo app never writes to a backup (only the cleanup below does, on request).
- Tests: `core/tests/backup.rs`, `core/tests/launcher.rs`.

Packaging: the release archive contains the macOS binary, `recognizer/` and
`Start shoebox.command`. Linux and Windows archives wait for the CI that
builds them (plan.md: CI until v1.0).

## Checklist

Done:
- [x] Library id in all API routes (one library; several come with section 3)

Open:
- [ ] Launcher UI (section 2): first slice built, platform checks and polish open
- [~] Multiple drives (section 3): hub, offline, roles, cross-drive duplicates and people built; common timeline built; moving between drives open
- [x] Backup verification (section 4); packaging of Linux and Windows builds waits for their CI
