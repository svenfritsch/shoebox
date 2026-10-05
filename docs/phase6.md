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
  buttons Scan, Verify, Recognize, Face Stats. "Start photo app" starts
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
- Not yet done: cancel button, remembering several paths, serving more than
  one folder, the platform checks below.

Platform behaviour to check on real machines (not tried yet):
- macOS: a double-clicked binary is blocked by Gatekeeper; `Start
  shoebox.command` stays the way in (it only has to start the binary).
- Windows: a double-click opens a console window and runs the launcher;
  SmartScreen warns about unsigned downloads once.
- Linux: file managers often open an executable in an editor or ask; a
  `.desktop` file or `Start shoebox.sh` with "run in terminal" is the
  fallback.

## 3. Multiple drives

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

Backup verification as described in plan.md ("Backups"), launcher scripts per
OS and packaging.

## Checklist

Done:
- [x] Library id in all API routes (one library; several come with section 3)

Open:
- [ ] Launcher UI (section 2): first slice built, platform checks and polish open
- [ ] Multiple drives (section 3)
- [ ] Backups and packaging (section 4)
