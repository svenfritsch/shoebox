# Phase 16: index next to the program (server libraries)

Status: **steps 1 to 6 built** except the CLI. The Control Panel layout is decided and built: tabs (mock-ups in `docs/mocks/phase16/layout/`, T1 and T2 are the chosen ones). Overview in [plan.md](plan.md).

## Why

A library today is a drive: `<root>/.shoebox/` holds `library.db`, `thumbs.db`,
`recognition.db` and the trash, so the drive carries its own index. That does
not fit a folder on a company server or NAS that is mounted like a drive
(`\\server\share\Fotos` or a mapped letter):

- the server folder may not be ours to write to, and `.shoebox/` would be
  left on it for everybody;
- nobody knows in advance which folders hold photos, so the scan has to walk
  all of them, and the result should live on the computer that did the work.

Use case that started this: look through a server for old photos of a
colleague, collect them, export them to a folder for a collage (see the
conversation summary at the end).

## Decision

The index of a library can live in one of two places, chosen per library when
it is added:

| | On the drive (today, default) | Next to the program (new) |
|---|---|---|
| Index folder | `<root>/.shoebox/` | `<program folder>/libraries/<name>/` |
| Travels with | the drive | the computer / the program folder |
| Writes into the library folder | `.shoebox/` (and the trash on request) | **nothing** |
| Moves, trash, rotate | as today (trash off by default) | **off, not switchable**: this library is read-only for shoebox |
| Found by the launcher | `.shoebox` exists in a drive's top folder | entry in `libraries.json` |

"Program folder" is `recognize::program_dir()`, the folder that holds
`shoebox` / `shoebox.exe` and `recognizer/` (the same place the add-ons use).
The two modes of the release (on the drive, on the computer) both keep working;
a program on a drive may also have libraries next to it, but the normal use is
the program on the computer.

## Layout

```
shoebox-windows/                 (or shoebox-macos/)
  shoebox.exe
  recognizer/                    ← add-ons, as today
  libraries.json                 ← the registry
  libraries/
    Server Fotos/                ← one folder per library, the name is the key
      library.db
      thumbs.db
      recognition.db
```

`libraries.json` (written by shoebox, readable by a person; one writer at a
time, written to a temporary file and renamed):

```json
{
  "version": 1,
  "libraries": [
    {
      "name": "Server Fotos",
      "root": "\\\\server\\share\\Fotos",
      "added": "2026-10-14T09:00:00Z"
    }
  ]
}
```

- `name`: the folder name under `libraries/` and the label in the Control
  Panel. NFC, unique case-insensitively, no path characters (the same check as
  `organize` uses for folder names). Default: the last folder name of the
  root, made unique with " 2".
- `root`: where the photos are. Files are stored **relative to it**
  (`files.path`), so the user can change it ("the share is now `Z:\Fotos`")
  without a rescan: the next scan sees the same relative paths, same sizes and
  dates, and finds nothing new. A different root that does not hold the files
  just shows them as missing, as an unplugged drive would.
- Nothing else in the file. Settings stay in `library.db` (`settings`), so a
  copied `libraries/<name>/` folder is complete on its own.

## Code: one place that says where the index is

Most of the work is a single function. From the grep of `<root>/.shoebox`:

- `thumbs.db` and `recognition.db` already follow the database path
  (`recognize::path_for(&db_path)`); nothing to do.
- `db::default_path(root)` is called in `scan`, `verify`, `serve`, `faces`,
  `recognize`, `arrivals`, `backup` and `launcher`. It becomes
  `library::index_path(root)`: look `root` up in the registry; if a library
  has this root, `<program>/libraries/<name>/library.db`; otherwise
  `<root>/.shoebox/library.db` as today.
- `organize::trash_dir(root)` (`.shoebox/trash`) stays for libraries on the
  drive; for a library next to the program every action that changes originals
  (`/api/move`, `/api/trash`, rotate, restore, rename of folders, removing
  copies) answers with an error that says why, and the buttons are hidden
  (`info` carries `read_only`). `export` is unaffected (it only reads).
- `scan::run` already accepts a database outside the library (`opts.db`);
  the check "outside the library or inside its `.shoebox`" stays.
- `launcher::drives` / `list_folders` and `export` use `p.join(db::DIR).is_dir()`
  to say "this is a library": they also consult the registry.
- Backup partners (phase 6): a library next to the program can be neither a
  backup nor have one in v1 (the pairing keeps its data in each drive's
  `library.db`, which would still work, but it is not needed here).
- Case: two registry entries with the same root are refused.

## Control Panel

Step 2 "Where are your photos?" is three **tabs**, so nothing sits in a box inside
the card (decided with the mock-ups in `docs/mocks/phase16/layout/`):

- **Photo folders**: the chosen folders (chips, editable by double-click), a
  "Choose…" folder picker (the photo app's export picker: `/api/places`,
  `/api/folders`), the detected drives (a click starts a path in the box; Enter
  alone adds the drive) and, only once a library exists, the row "Libraries
  kept next to the program" with an info marker (the photo app's circle-i, native
  tooltip) and one chip per library: name and a badge with the photo app's tab
  icon and **"separate" / "separat"**, dashed and "not connected" when the root
  cannot be read, like a missing drive. With no library the row is replaced by
  one line that points to the next tab.
- **Add a folder or server** (always there): folder (with Choose…), where the
  index is kept (**On the drive**, the usual way, or **Separate (next to the
  program)**; the latter is greyed out with the reason when the program folder
  cannot be written to), and, for Separate only, a name and the note that
  shoebox only reads. Adding returns to Photo folders.
- **Manage libraries (n)**: only while n > 0 (it goes away with the last
  library). Per library: state ("connected · 48,212 photos", "not scanned yet",
  "not connected"), Rename (moves the index folder), Change folder (checks a
  sample of the indexed files against the new folder first; the index is
  kept), Remove from the list (the index folder stays) and Delete the index.
  Remove and delete are modal dialogs (the page is greyed out behind them);
  deleting says that tags, dates, places and names go with the index. Both also
  take the folder out of the list of chosen folders, so a later scan cannot
  write a `.shoebox` into it.

The Control Panel's own tab icon is the photo app's box with a small gear on it,
so the two windows can be told apart.

CLI: `shoebox scan <root> --index-next-to-program [--name N]`,
`shoebox libraries` (list, `--json`), `shoebox libraries remove <name>`. A
plain `shoebox scan <root>` of a path that is in the registry uses its index
without flags.

## Windows and network paths

- Canonicalising a UNC path gives `\\?\UNC\server\share\…`; the registry
  stores the root as the user gave it and compares canonical forms, so
  `Z:\Fotos` and `\\server\share\Fotos` are the same root. If two registered
  roots resolve to the same canonical path, refuse the second.
- `volume::placement` returns `None` for a share; the Control Panel shows the
  registry name instead.
- SQLite lives on the local disk here, so the locking problem of a database on
  SMB does not arise; only reads go over the network.
- Reads over the network are slower: the scan's full-hash pass is already a
  resumable background pass; add a lower default thread count for roots that
  are not a local drive (to decide with a measurement).
- Guard: unchanged. Every read of an original is still before/after checked
  (size, mtime, created, full hash). New tests assert that a scan, a serve
  session and an export of a library next to the program write **nothing** in
  the root (compare a tree snapshot before and after, including no `.shoebox`).

## Steps

1. [x] `libraries::index_db` (called by `db::default_path`), the registry (`libraries.json`, read/write,
       atomic), a test for every caller that now goes through it; behaviour
       with no registry entry is byte-for-byte as today.
2. [x] Read-only enforcement: `libraries::ensure_writable(root)` is the first line of
       every function that changes an original (`organize::{rename_folder,
       move_files_with, rotate, turn, trash_files, restore, empty_trash}`,
       `duplicates::remove_copies`) and of `POST /api/allow-trash`; `info.read_only` and
       `/api/allow-trash` (`allow` is false, `read_only` true) tell the photo app, which hides
       Move (selection bar and info panel), Rotate, the folder rename pencil and the duplicates
       delete buttons (`body.read-only`) and disables the "Allow move to trash" checkbox with a
       hint. `core/tests/libraries_readonly.rs` checks each function refuses and a real serve
       session (info, trash setting, move, trash, rotate refused, nothing written).
3. [ ] CLI (`--index-next-to-program`, `libraries`).
4. [x] Control Panel: the row with badge (our own shoebox icon + "separate" / "separat") and
       an info marker like the photo app's, the add dialog, the Manage panel (rename,
       change folder with a check, remove, delete the index); API `/api/libraries*`
       in `launcher.rs`; strings in `en.json` / `de.json`; `core/tests/launcher.rs`.
       The folder picker is the photo app's (`export::places` / `list_folders`, routes
       `/api/places` and `/api/folders` in the launcher). A click on a detected drive starts a
       path in the box, a double-click on a chip edits it. Remove and delete the index are modal dialogs.
5. [x] Tree-snapshot guard tests: scan and verify (`core/tests/libraries.rs`) and a serve
       session (`libraries_readonly.rs`); an export of such a library is still to add.
6. [x] Guide, both languages (Get started, Safety, FAQ; the Drives section waits for the conflict markers on main to be resolved) (Get started: the two places for the index;
       Safety: nothing written to the folder; Control Panel; FAQ "my photos are
       on a server"), then `python3 docs/guide/check.py`.
7. [ ] Check on a real share from a real Windows PC (needs the Windows build
       check of "Decoders per build", step 4/5, first).

## Not in scope

A multi-user server mode (accounts, a service, several people working at
once), index copies shared between computers, writing anything to the
share, faces over the network as a tuned feature (works, slow).

## Open questions

- Name of the folder and the file (`libraries/`, `libraries.json`) is decided;
  whether the registry also records the drive-letter form the user typed is
  not (it would only matter for display).
- Default thread count for network roots (measure).
- Decided: "next to the program" is offered for any folder, not only a
  server. It always means read-only for shoebox (no move, trash, rotate,
  rename); someone who wants to organise photos keeps the index on the drive.
- Licence: the first user is a friend, on one work laptop, with the owner's
  permission. `LICENSE.txt` stays as it is; the permission is a note to
  the friend, not a licence change. Anything beyond friends still waits for
  the lawyer item in plan.md.

## Origin

A friend wants to find old photos of a retiring colleague on the company
server (mounted like a drive, no cloud) and make a collage. Flow with this
phase: download the Windows folder, Control Panel → add the server path with
"index next to the program", scan (overnight), find the colleague by folder,
date, text or (with the Faces add-on) face, collect with the collection tag,
export to a folder, build the collage in another program.
