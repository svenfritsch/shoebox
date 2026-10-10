# Phase 12: export (collect photos, copy them to a folder)

**Planned, not built.** Mock-ups of every screen are in
[`mocks/export/`](mocks/export/) (static HTML plus PNGs, made by
`mocks/export/build.py`; they reuse `core/web/app.css`, they are not the app).

Goal: someone finds pictures in shoebox, collects them over one or several
sessions, and ends up with a plain folder of copies they can take elsewhere:
onto a USB stick for a print shop, onto the computer for an InDesign album,
into a folder for a birthday video (photos and videos), or onto another
person's shoebox drive.

## 1. User journey

1. **Collect.** In **Settings → Collection** the user types a tag name
   (`print-march`). A bookmark button then appears in the photo view
   (shortcut **C**): one click tags the photo with that tag, another click
   removes it. The same tag can also be given the usual way (selection →
   "Add tag…"). Nothing new is stored for this: a collection is an ordinary
   own tag, so searching, chips and the sidebar already work.
2. **Review.** Search for the tag. The result is the collection.
3. **Export.** With a tag chip active, an **Export…** button appears next to
   the chips. In the selection bar there is an **Export…** button too. Both
   open the *same* dialog; only the folder name differs: from a tag it is
   pre-filled with the tag name, from a selection it is empty and required.
4. **Done.** A folder with numbered copies, a summary, "Show in Finder".
5. **Top up later.** Exporting the same tag into the same folder adds only
   what is new and renumbers the rest (section 4).

Per-batch use stays possible (select, Export…, name the folder), so the
journey does not force tagging.

## 2. Decisions

| Topic | Decision |
|---|---|
| Collecting | An ordinary own tag, named in **Settings → Collection**. No new table. The bookmark button is **hidden** (not greyed) while the setting is empty, like the trash icon while `allow_trash` is off |
| Wording | **Collection** in the UI and the guide; the button itself is only an icon. Its tooltip says what it does in tag terms: `Add tag “print-march” (C)` / `Remove tag “print-march” (C)`. German: **Sammlung** (not "Kollektion", which means a fashion or product line); tooltip `Tag „print-march“ hinzufügen (C)` / `… entfernen (C)`. The file-side words stay Tag / Export / Exportieren |
| Where the two settings live | Library settings next to `allow_trash` (server-side, so the export job can read the numbering setting). To confirm when building. The tag name goes through `tags::check_name`; `favorite` and its spellings are refused (they mean the heart) |
| Export = copy, never move | Originals are only read. A copy is byte-identical (EXIF and everything else stays), modified time is set to the original's, the created date where the system allows it (macOS, Windows; not Linux) |
| Naming | Settings → Export → **Number exported files** (on by default): `001_IMG_3457.jpg`. Numbers follow capture date (then path), width at least 3 digits, wider if the set needs it. The same dialog explains this and points to Settings → Export in both entry points |
| Name clashes | With numbering **on** the prefix makes every name unique, no suffix. With numbering **off** two different photos can both be `IMG_0001.jpg`: the later one becomes `IMG_0001_2.jpg`. The suffix exists for this case only |
| RAW | A checkbox "Also export the RAW files (N photos have one)", **only shown when the set has RAW pairs**. A RAW file takes the number and stem of its picture. Off by default |
| HEIC | A checkbox "Convert HEIC to JPEG (N photos)", only shown when the set has HEIC. On by default (print shops, InDesign). See section 5 |
| Videos | Allowed; copied as they are, numbered like photos. Live Photos: open (section 8) |
| Destination | Two rows: **Save in** (the parent folder, chosen with the in-app picker, remembered) and **Folder name**. Downloads is the first place in the picker. The folder is always created by the export, so the picker has no "New folder" |
| Other shoebox drives | **Not forbidden.** Drives and folders that are on a drive with a `.shoebox` folder are marked "shoebox" in the picker. Clicking Export with such a destination first asks "Save to another shoebox drive?": the copies will be found by that drive's next scan and added as new photos, without tags, people or favorites. Confirm to go on. If the destination is on the drive the photos come from, the text says the copies will show up there as duplicates of the originals |
| Re-export | Folder exists → merge, never replace, never delete. Skip a photo that is already there: same name **without the number** and same size. A photo that was converted from HEIC counts as already there when a `.jpg` with the same name without the number exists (no size test: no second conversion just to compare). All files of the set are then renumbered to the new order (section 4) |
| Progress and stop | A background job with progress ("Copying 17 of 42 …") and **Stop**. Stop finishes or removes the file in progress; files already copied stay |
| After | "Checked against the originals: all identical", counts (new / already there / converted), **Show in Finder** (`reveal.rs`) |
| Phone and iPad | Export copies onto the computer shoebox runs on, so the Export buttons and the picker are only offered to a browser on that computer (loopback). The iPad gets an explanation instead. A ZIP download for it is out of scope |

## 3. Mock-ups

| Screen | Mock-up |
|---|---|
| Selection bar with Export… | `mocks/export/01-selection-bar.png` |
| Export dialog from a selection (name required, Export disabled) | `02-export-dialog-selection.png` |
| Folder picker, macOS and Windows | `03a-folder-picker-mac.png`, `03b-folder-picker-windows.png` |
| Tag chip with the Export… button | `04-tag-active-export-button.png` |
| The same dialog from a tag (name pre-filled) | `05-export-dialog-tag.png` |
| Folder exists: "Add 12 photos" | `06-export-dialog-folder-exists.png` |
| Progress with Stop, finished | `07-export-progress.png`, `08-export-done.png` |
| Settings: Collection and Export | `09-settings.png`, `09b-settings-collection-empty.png` |
| Photo view: tooltip off / after click / tooltip on / no collection set | `10a` … `10d` |
| Warning for another shoebox drive | `11-warning-other-shoebox-drive.png` |

## 4. How an export runs (`core/src/export.rs`)

1. **Plan** (no writing): the files of the tag or selection from the index,
   ordered by capture date then path; add RAW partners if ticked; drop what
   the type filter excludes; compute totals; check free space at the
   destination and that file names are valid there (exFAT, FAT32 and Windows
   forbid `: ? * " < > | \`, trailing dots and spaces, names over 255
   characters; invalid characters become `_`). Nothing is created yet.
2. **Look at the destination folder** if it exists: list its files, strip a
   leading `^\d{3,}_` from each name, and match by name and size against the
   plan (converted HEIC: by name and extension `.jpg`/`.jpeg`). The dialog
   shows "N new, M already there" from this, before anything is copied.
3. **Copy** each new file: guard snapshot of the original (size, mtime,
   created, full hash) → stream into `<name>.part` while hashing → compare the
   copy's hash with the original's → rename to the final name → set mtime (and
   created date where possible) → guard check of the original again. Any
   difference fails the export for that file and is reported, never hidden.
4. **Renumber** the files already there so the whole folder follows the new
   order. Only files that match `^\d{3,}_` followed by a name in the set are
   renamed (via temporary names, so two files can swap numbers); foreign files
   in the folder are never touched. Files that left the tag stay where they
   are, with their old numbers; export only adds and renames.
5. **Report** counts and any failures; offer "Show in Finder".

Every read of an original is under the guard (CLAUDE.md rule). Export never
writes to an original and never touches the index of the source drive.

## 5. HEIC to JPEG

- Decode with the libheif/libde265 build that exists for thumbnails; encode
  with the JPEG encoder the thumbnails already use if its quality settings
  allow a high-quality export (to check), otherwise a small encoder crate
  (then `scripts/third-party-licenses.py` and `THIRD-PARTY-LICENSES.txt`).
- **EXIF is not carried over by itself.** The converted file gets the EXIF
  block of the original (capture date, camera, GPS) with the orientation
  reset to match the pixels, and the ICC profile. Without this the capture
  date would be lost. The file's mtime is set to the capture date as for any
  copy.
- A rotation the user set in shoebox only (stored in `library.db`, not in the
  file) is applied to the converted pixels. For a copied file it cannot be:
  the copy opens unrotated outside shoebox (say so in the guide).
- The name becomes `.jpg`; the rest of the rules above treat it as the same
  photo as the HEIC.

## 6. Folder picker and the server side

- `GET /api/fs/places`: Downloads, Desktop, Documents, Pictures and the home
  folder (macOS `~/…`; Windows the system's known folders, which users may
  have moved, with `%USERPROFILE%\…` as fallback; Linux
  `~/.config/user-dirs.dirs`, then `~/…`; a place that does not exist is
  left out), plus the drives from `launcher::detect_drives` (macOS
  `/Volumes`, Linux `/media/<user>`, `/run/media/<user>`, `/mnt`, Windows
  drive letters). Each drive says whether it has a `.shoebox` folder and
  its free space.
- `GET /api/fs/list?path=…`: sub-folders only (no files; hidden folders
  left out), NFC-normalised, quoted everywhere. Because this shows the
  computer's folders, **both fs endpoints and the export job answer only to
  the local machine** (loopback), not to the PIN-protected LAN clients.
- A destination is "on a shoebox drive" when `.shoebox` exists in the volume
  root or in any parent folder of it.
- Platforms: the code is plain `std::fs` plus the three place lookups.
  Linux and macOS are testable in CI and Docker; the Windows lookups follow
  the plan's rule for Windows (reviewed, not run) until a Windows build
  exists.

## 7. Build order (each step its own commit)

1. `export.rs`: plan, name rules, copy under the guard, merge and renumber,
   unit tests with fixtures (spaces, NFD names, RAW pairs, re-export with a
   new earliest photo).
2. Settings (collection tag name, numbering), `GET/POST` for them; i18n keys.
3. API: `POST /api/export` (plan + start), job progress and stop, fs
   endpoints, the shoebox-drive check.
4. UI: dialog (one code path for both entry points), picker, warning, the
   Export… buttons, progress and result; the settings sections.
5. Photo view: the bookmark button, tooltip, `C` shortcut, info-panel refresh
   after a toggle; hidden while the setting is empty.
6. HEIC conversion with EXIF and orientation handling.
7. Guide (EN and DE, `.txt`), screenshots from the real app, CLAUDE.md rules
   (Export reads originals under the guard, writes only new files outside
   the library; originals untouched), this file as built, status row.

## 8. Open

- [ ] **Live Photos:** copy the still only, or the still and its video part?
- [ ] Where the two settings are stored (library vs. browser), see section 2.
- [ ] Whether the JPEG encoder of the thumbnails is good enough for export.
- [ ] Created date of the copies on macOS and Windows (set it; Linux cannot).
- [ ] "Export…" for a search result that is not a tag (a plain search with
      chips): same dialog, name required? Probably the selection route
      (Select all, Export…) is enough.
- [ ] A remembered default for "Save in", per computer.
- [ ] ZIP download for the iPad: not planned.
- [ ] German strings (Sammlung, Exportieren …, Ordnername, Speichern in).
- [ ] Real-hardware check: a USB stick formatted exFAT, FAT32 and NTFS; a
      folder with spaces and decomposed Unicode names.
