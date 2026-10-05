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

## 3. Multiple drives

- Every drive keeps its own `.shoebox` folder with `library.db`, `thumbs.db`
  and `recognition.db`. The app opens several libraries at once with one
  shared experience; the UI takes two or more paths.
- People are matched by name: the same name on different drives is the same
  person. Groups, names and decisions stay in the drive's own `library.db`.
- Duplicates across drives are found by comparing hashes (e.g. `ATTACH`).
- An unplugged drive shows as "offline"; the rest keeps working.

## 4. Backups and packaging

Backup verification as described in plan.md ("Backups"), launcher scripts per
OS and packaging.

## Checklist

Done:

Open:
- [ ] Library id in all API routes
- [ ] Launcher UI (section 2)
- [ ] Multiple drives (section 3)
- [ ] Backups and packaging (section 4)
