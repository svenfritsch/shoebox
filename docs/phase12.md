# Phase 12: estimated capture date

A date of the user's own for a photo: a WhatsApp picture, a scan of an old
print, a photo taken of a photo. It decides where the photo sits on the
timeline and what a date search finds. It lives in `library.db` only; the
file, its EXIF date and its created date are never touched.

Feedback that shaped this: a scan or a photo of a print has an EXIF capture
date of the day it was made (2026) while the picture is from 1987. So the
date must be settable on **every** photo, and it must win.

## Decisions

| Topic | Decision |
|---|---|
| Precision | **Year, month or day.** The year is required, the month is optional, the day only after a month. The date keeps its precision, so the screen says "1987", not "January 1987". No ranges ("1985 to 1987"; a year already is one), no seasons or "circa" (the "~" marks every guess), no free-text note (later). |
| Storage | Table `date_estimates` (schema v13): `key` (= `files.quick_hash`, like `taken_overrides`, `view_turns`, `geo_overrides`), `year`, `month`, `day`, `at`. Copies with the same content share it. Never written into a photo. |
| Which date wins | **The user's date wins on every photo**, even over a capture date in the file. Order, first that exists: (1) the user's estimate, (2) the capture date in the file (EXIF, a video's container; also a date a duplicate clean-up carried over, `taken_overrides`), (3) the month of the nearest event folder (shown as the 1st), (4) the earlier of the file's created and modification dates. The file's date is never lost: the info panel shows it in its own row "EXIF" and "Remove estimate" brings it back. |
| Why not "the file wins" | A scan or a photo of a print has a 2026 EXIF date; it could never be placed. A second "Correct date" feature would be two buttons and two rules for one result. |
| Sorting and groups | The less precise date sorts as the start of its period: `1987-06-14T00:00:00` (day), `1987-06-00` (month), `1987-00-00` (year). Within a month the exact photos come first, then month-only. A **year-only** photo gets its own group "1987" at the bottom of that year (the timeline's month 0 heading, which already prints the year only). A time of day and a UTC offset are not shown for a photo with an estimate (it has none). |
| "Needs a date" | A sidebar entry (`nodate=1`) with a count: photos with **no capture date in the file** (date source folder, created or modified) **and no estimate**. It is the worklist for WhatsApp and scan folders. |
| Duplicate clean-up | It compares real dates only (file plus carried over); the estimate is carried separately: the surviving copy takes it only if it has none. A JPEG turned in the file gets a new quick hash, so `organize::rotate` moves the estimate to the new key (like `taken_overrides`). Trash keeps it (content key). A file edited elsewhere has a new content key and starts without an estimate, like a turn does. |
| Backup of user data | `userdata.json` version 6 adds `date_estimates` (with the files they belong to). |
| Scope | One library at a time (ids are per drive), as with favorites and positions. The common timeline shows the dates and the "~" but has no pencil. |

## Wording

| Where | English | German |
|---|---|---|
| Badge (today, for the fallbacks) | estimated | geschätzt |
| Badge (the user's date) | manual | manuell |
| Info row, only if the file has a capture date and an estimate is set | EXIF | EXIF |
| Pencil (title) | Set date / Change | Datum setzen / Ändern |
| "~" on a timeline cell (title) | Estimated date manually / from folder name / using file created date / using file modified date | Geschätztes Datum manuell / aus Ordnername / nach Erstelldatum der Datei / nach Änderungsdatum der Datei |
| Search group, chip | Date | Datum |

The EXIF row label is the shortest option, so the panel's label column does not
grow ("Original date" was longer than "Camera"). For a video the date comes from
the container; the row still reads "EXIF". The German texts are drafts for a
native read-through, like the other keys.

## UI

- **Info panel.** The Date row keeps one line: the date, the badge ("estimated"
  as today; "manual" for the user's date), and a pencil button at the right end
  (`title` "Set date", or "Change" when an estimate exists). Under it, only when
  the file has a capture date and an estimate is set: the row "EXIF" with the
  file's date and time (and UTC offset). The pencil is on every photo.
- **Dialog "Set estimated date" / "Change estimated date".** Year (4 digits; 2
  digits read as 19xx, or 20xx when 19xx would lie in the future), Month and Day
  as drop-downs ("Not sure", then the months; the days 1 to the length of that
  month, leap years included; the day is disabled until a month is chosen). A
  preview says what is shown and where the photo lands. A date in the future is
  refused. Quick button "Folder: June 2025" when the date now comes from the
  event folder. "Remove estimate" only when there is one (for a selection: when
  at least one has one); it says what comes back. Enter saves, Esc cancels. Hint:
  "Saved in shoebox only. The file is not changed."
- **Timeline.** A small "~" in the corner of every cell whose date is not from
  the file or was set by the user, with a `title` that says which (see Wording).
  Year-only photos form the group "1987" below January.
- **Selection.** "Set date…" in the selection bar for any selection; the dialog
  tells how many of them have a capture date in the file ("6 of the 38 photos
  have a date in their file. Your date will be used for them on the timeline.").
  After saving, a toast with Undo restores the previous estimates.
- **Sidebar.** "Needs a date" with its count, next to "All photos".

## Search

Follows the shortcut design of phase 9 (`name:`, `text:`): a prefix switches the
box to one kind of result.

- `date:` (and `datum:`, same thing, case-insensitive, in both languages) clears
  the suggestion list to one group headed **Date** (**Datum**). Rows read "June
  1987" with the number of photos; no "Date:" in front, the heading says it.
- Without a prefix the Date group is the **last** group, and only when what was
  typed is clearly a date (a 4-digit year, a month with a year, a numeric date).
  A month name alone never gets a date row there ("June" keeps finding June tags
  and people called June). `date:june` works and lists every June of the
  library, newest first (one row per year that has photos in a June).
- A picked row becomes a chip with a calendar icon (`title` "Date: June 1987"),
  ANDed with the other chips. API: `date=1987`, `date=1987-06`,
  `date=1987-06-14` (repeatable, AND), `nodate=1`.
- Accepted input: `1987`; `june 1987`, `juni 1987`, `6/1987`, `06.1987`,
  `1987-06`; `14.6.1987`, `14 june 1987`, `1987-06-14` (numbers with `/` or `.`
  are day, month, year). Month names in English and German, 3 letters are enough
  (`jun`, `mär`, `dez`).
- **A photo matches when its date lies completely inside what was typed.** A
  year-only photo matches "1987" but not "June 1987"; a month-only photo (or one
  that sits in an event folder, shown as the 1st) matches the year and the
  month but not a day. The search uses the date the timeline shows.
- The counts come from the timeline held in memory (`browse::Snapshot`), so
  typing never queries a database.

## API (all under `/api/lib/{lib}/`)

- `GET /files/{id}`: gains `estimate {year, month, day}` or `null`, and `exif
  {taken, taken_offset}` (the file's own capture date, or `null`); `date_source`
  can now be `estimate`.
- `POST /files/dates` `{ids, year, month?, day?}`, `{ids, clear: true}` or
  `{restore: [{id, estimate}]}`: set, remove, or put back (Undo). Works on every
  photo; answers `{changed, with_exif, previous: [{id, estimate}]}`. 400 for an
  impossible date or one in the future.
- `POST /files/dates/check` `{ids}`: `{total, with_exif, with_estimate}` for the
  dialog.
- `GET /timeline`: gains `est` (`[[id, "m"|"f"|"c"|"u"]]`: manual, folder name,
  created, modified) for the "~", `date=`, `nodate=`; year-only photos have the
  day value `YYYY0000`.
- `GET /dates?q=…&prefix=1`: the Date suggestions (rows `{year, month, day,
  count}`) within the current filter; `GET /info`: gains `needs_date` (the
  count).

## Guide

Both guide files (English and German, kept in step) get a section "Dates and
searching by date": which date a photo is placed by and in which order (see
Decisions), what "estimated" and "manual" mean, how to give a photo or a whole
selection a date, "Needs a date", the "~" mark, and the search (`date:`, the
Date group, what matches). "Your photos are safe" says a date set in shoebox
stays in shoebox. The info-panel screenshot is regenerated.

## Tests

`core/tests/dates.rs`: set, change, clear (day, month, year); impossible and
future dates refused; the order (estimate over file over folder over created);
removing the estimate brings the file's date back; sort order of day, month and
year in one month and the day value `YYYY0000`; `est` markers; `needs_date`;
`date=` filters (a year-only photo matches the year, not the month; the folder
fallback is month-precise); the Date suggestions (prefix, no month alone without
prefix, `datum:`); `restore` (Undo); carried by a rotated JPEG and by a
duplicate clean-up; `userdata.json` v6; the guard (size, mtime, created and full
hash of every original unchanged, `X-Shoebox` header needed for changes).

## Status

| Slice | Scope | State |
|---|---|---|
| 1 | Table, date order, API, info panel (pencil, dialog, EXIF row), "~", year-only group, carry-over, userdata, guide | see plan.md |
| 2 | "Set date…" for a selection, Undo, "Needs a date" | see plan.md |
| 3 | Date search: group, `date:`/`datum:`, chips | see plan.md |

## Not done / ideas

- [ ] Real-hardware check (iPad: the drop-downs, the pencil target size, the dialog on a phone-sized screen).
- [ ] A hint with the dates of the neighbours in the same folder in the dialog.
- [ ] A free-text note with the date ("from the back of the print: Ostern"), searchable.
- [ ] Dates of the common timeline cannot be changed there (ids are per drive), as for tags and favorites.
