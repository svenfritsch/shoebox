# Phase 15: your data between drives

**15a built** (15a-1 and 15a-2, one PR; 15b, 15c and phase 18 are still planned). One deviation: a date is carried only when the target has neither a corrected nor an estimated date for that photo. Closes two of the three "not yet done" items in
[phase6.md](phase6.md) section 3 in a smaller way than first drawn up:

- **15a** (this plan): counts in the All drives search suggestions that narrow
  with the filter, and **Carry over data**: bring the tags, hearts, people,
  dates and places of photos from one shoebox drive to the same photos on
  another.
- **15b** (own PR, later): the same data from a **file** instead of a second
  drive (section 6).
- **15c** (own PR): **Ignore folders** in the duplicates across drives, for
  folders that are on several drives on purpose (section 6b; mock-ups in
  [mocks/ignore-folders/](mocks/ignore-folders/)).
- **Not planned** (section 7): moving photos between drives inside shoebox, and
  a "keep both" decision stored per photo.

Phase 14 is the backup comparison in plan.md, so this is phase 15.

## Status: done in pull request #121 and still to do

Pull request #121 (`claude/phase15-between-drives`) holds **documents only**. No
Rust, web, i18n or guide file is changed, so the build workflow skips it (docs
only) and `python3 docs/guide/check.py` stays green. Main was merged into the
branch (it has phase 17, the Windows release; `docs/plan.md` had one conflict in
the status table, resolved: rows 15, 17, 18 in that order).

**Done in the pull request**

| What | Where |
|---|---|
| This plan (use cases, design, tests, open questions) for 15a, 15b, 15c | `docs/phase15.md` |
| Plan for phase 18 (identical photos once) | `docs/phase18.md` |
| Status rows 15 and 18 and a short details entry each; pointers in the "Not yet done" line | `docs/plan.md`, `docs/phase6.md` |
| Mock-ups of the Across drives screen: the dropped "Keep both" placement, the Ignore folders field with chips inside, the suggestions dropdown | `docs/mocks/ignore-folders/` (`build.py` makes the HTML, `shoot.mjs` the PNGs) |

**Decisions taken** (all in the plan above): moving photos is Finder's job, not
shoebox's; Carry over data has five check boxes (Tags, Hearts, People, Dates,
Places), all ticked, and "other marks" are named but not offered yet; 15b (data
from `userdata.json`) is its own PR; "Keep both" is dropped; Ignore folders hides
a photo when **any** copy is in an ignored folder, found on one drive is enough,
no "show hidden" switch; the field is a chips input with the chips inside, and
`chipsInput` is generic: suggestions, row drawing and the free-text row are passed
in; the dropdown sits under the field and follows it; phase 18 is its own phase
and merges only identical **name and hash**.

**Still to do** (nothing below is built)

| # | Step | Notes |
|---|---|---|
| 1 | **15a-1 Counts that narrow** | `/api/all/tags` and `/api/all/people` take the timeline's filter; UI passes it. Section 5.1 and 9; `core/tests/multi.rs` |
| 2 | **15a-2 Carry over data** | `core/src/transfer.rs`, API (`/api/all/transfer`, `/plan`, hints), Manage drives button and dialog with the five boxes, the export's `export.transfer_hint` line, `transfer.*` keys in `en.json` and `de.json`, `core/tests/transfer.rs`, `core/tests/serve.rs`, guide (EN and DE), a line in the CLAUDE.md rules (reads another drive's index, writes only this drive's index), run `python3 scripts/third-party-licenses.py` only if dependencies change (none planned) |
| 3 | **15b Data from a file** | own PR: a second reader for `userdata.json`, "From a file…" in the dialog, a file picker (extend the export's folder picker), versions 1 to 6 of the file; guide |
| 4 | **15c Ignore folders** | own PR: `chipsInput` (generic) in `core/web/app.js`, `ignore` argument in `multi::cross_duplicates`, `hidden_groups`, `/api/all/duplicates/folders`, "Ignore this folder" on each row, `dups.ignore.*` keys, the browser setting `shoebox-dup-ignore`, tests in `core/tests/multi.rs`, guide, screenshot step |
| 5 | **Phase 18** | own PR, after 15a-1: see [phase18.md](phase18.md); it also changes the counts of step 1 |
| 6 | **Guide for every step** | both languages in step, `docs/guide/check.py`, say in each PR what changed in the guide |
| 7 | **Real-hardware check** | two real exFAT drives: a Finder copy, then Carry over; the couple's drives for 15c and 18 |

**Open questions still without an answer**: section 10, questions 1 to 6 (question
7 is decided: phase 18). Until they are answered the recommendations there are
what will be built.

## 1. Why this shape

Shoebox does not manage files: there is no import, and an original only
changes by rename, trash or a JPEG turn. Copying or moving photos between
drives is a job for Finder or Explorer, and the export already copies a
selection (also to another shoebox drive) with a verified hash. What is missing
is not the files, it is **what the user made in shoebox**, which lives in the
source drive's `library.db` and stays behind. So this phase moves data, never
photos.

## 2. Use cases

Names and numbers are made up but typical.

**A. The travel SSD is emptied in Finder.** Mia comes back from Norway with
`TRAVEL` (exFAT) holding `2026-08 Norwegen/` (1,840 photos, 60 videos, 210 RAW,
95 Live Photos) and `2026-08 Norwegen Drohne/`. On the trip she gave tags
("Fjord", "Papa"), hearted 40 photos, named people and fixed the date of 14
scanned slides. At home she copies both folders to `FAMILIE` in Finder and
scans `FAMILIE`. *Today:* the photos are there, every tag, heart, person and
date is not, and `TRAVEL` is about to be wiped.

**B. A full drive, one year moves to a second drive.** `FAMILIE` is 98 % full.
Tom drags `2014/` to `2016/` (11,300 photos, 90 GB) to `FAMILIE-2` in Finder
and scans it. Some 2015 folders were copied there once before and already got
some tags on `FAMILIE-2`. *Today:* the same loss as in A, and he must not
overwrite what he did on `FAMILIE-2`.

**C. A selection across folders goes to another shoebox drive.** Eva picks 80
photos from a dozen folders on `LAPTOP` for her grandparents' drive `OMA` and
uses **Export** (it also writes to a shoebox drive after a question, see
phase 13). The copies are numbered and flat. *Today:* on `OMA` they are new
photos without her tags, hearts and people.

**D. Search counts in All drives that do not match.** Sofia is in All drives
(`LAPTOP`, `FAMILIE`), picks the person Anna and sees 640 photos. The tag
suggestions still say "Winter (5,210)", "Ski (1,400)", counted over every photo
on both drives. She picks "Ski" and gets 38. On a single drive the suggestions
narrow with the filter and hide tags that would show nothing; in All drives
they do not.

**Things that go wrong**

| Case | How the plan handles it |
|---|---|
| The source drive is already wiped or not plugged in | Not possible in 15a (it needs the source's `library.db`); that is what 15b is for. The dialog and guide say: carry over *before* wiping |
| Source photos were moved or deleted in Finder | The source index still knows them as missing (`missing_since`) until its records are forgotten; they are used |
| Different file names or folders on the two drives (Finder move, export numbering) | Photos are matched by **content** (full hash), not by name or folder |
| A file was edited (different content) | Not the same photo; nothing is carried |
| Target already has tags, dates or decisions of its own | They win. Carry over only adds |
| Two identical copies on the target | The data goes onto both |
| A person has another spelling on the target | Matched by name as in All drives (NFC, case-folded); a missing person is created without group and picture |
| A drive is unplugged half way | One transaction on the target's `library.db`: all or nothing. Run it again |
| A backup drive is involved | Refused as source and as target (also a suspected backup, until the role is decided) |
| RAW, Live Photo video, sidecars | They are separate files with their own hash; whatever has data carries. Sidecars are not indexed and have no data |
| Different file system | Irrelevant: no file is read or written |

## 3. What is carried (five check boxes, all ticked by default)

The dialog offers **opt out**, not opt in: the default saves data. A box
shows its count and is greyed at 0.

| Box | Source data | How it lands on the target |
|---|---|---|
| **Tags** | own tags (`file_tags.source = 'user'`, except the heart); the collection tag from phase 13 travels too (it is an ordinary tag and can be removed) | Added as own tags to every target file with the same content. Folder tags are never carried: they come from where the file lies |
| **Hearts** | the own tag `favorite` | same, as the heart |
| **People** | `face_decisions` (confirmed, not this person, stranger, not a face, hand-drawn boxes), with the person by name | Decisions copied by content and box. A person is looked up by name, created if missing. A target decision with an overlapping box (IoU ≥ 0.5, the existing rule) is left alone. Groups, pictures and hidden flags are not carried. The target's recognizer finds the faces later, the decisions then take hold as they do after a model change |
| **Dates** | corrected dates (`taken_overrides`) and estimated dates (`date_estimates`) | Row copied if the target has none for that content |
| **Places** | positions given by hand (`geo_overrides`) | same. Named places (rectangles on the Locations page) are about the drive, not a photo, and are not carried |

**Other marks** (a HEIC/PNG turn in the viewer `view_turns`, "is a screenshot"
`shot_marks`, hidden text lines `text_hidden`) are the same kind of
content-keyed rows. They are **not offered yet**: the code is built so a sixth
box can be added by listing one more table, and the plan names them so a later
change adds them on purpose. Not carried at all: thumbnails and recognition
results (the target builds them), duplicate decisions inside the source drive
(`dup_decisions` refer to its file ids), the source's trash.

Matching: a source file and a target file are the same photo when their full
hashes are equal; if either has none yet, when size and quick hash are equal.
The source's missing files count. Photos in the target that are themselves
missing do not receive data.

## 4. How it works

`core/src/transfer.rs`, one module, two halves so 15b can reuse the second:

1. **Read** a `TransferSet` (per box: items keyed by hashes) from a source. 15a
   has one reader: the other drive's `library.db`, opened read-only with
   `ATTACH` as `multi.rs` already does. Only the five boxes' tables are read.
2. **Apply** a `TransferSet` to the target for the ticked boxes: matching, then
   inserts in **one transaction** on the target's `library.db` (`INSERT OR
   IGNORE` for content-keyed rows; tags added if not there). Idempotent: a
   second run reports 0 and writes nothing.

Writes go only to the target's `library.db`, which the app writes anyway.
`library.db.bak` is refreshed by the usual debounce (a way back). No photo is
read or written, so no guard is needed; a test checks that the source drive's
files and index are byte-identical afterwards.

API (the drives are `{id}`s as in `/api/all/*`):

- `GET /api/all/transfer` the **hints**: for each ordered pair of online library
  drives (neither a backup nor suspected) with matching photos: how many photos
  on the target would get something and per box how many items. Hash join over
  the attached indexes, no file access.
- `POST /api/all/transfer/plan {from, to}` the counts per box (the dialog).
- `POST /api/all/transfer {from, to, boxes}` applies; answers the counts done.
  503 `offline` for a drive of the pair that is not there; 400 for a backup.
- After applying: the target's caches that show tags and people are refreshed
  like after any tag edit, and the clusters/suggestions job is asked for.

## 5. UI

- **Manage drives**: a drive card (the target) gets a button **Carry over data…**.
  Where hints exist, the card shows a line: "TRAVEL has data for 212 photos
  here. [Carry over data…]". The button opens the dialog with a **From** drop-down
  (the other usable drives; preselected when the hint names one).
- **Dialog** (`transfer.*` in `core/i18n/en.json` and `de.json`):
  - `transfer.title` "Carry over data" / "Daten übernehmen"
  - `transfer.explain` "Photos that are on both drives get what you made on {from}: {boxes}. Nothing on {from} changes, no photo is touched, and what {to} already has stays."
  - boxes `transfer.tags` "Tags (212 photos)", `transfer.hearts` "Hearts (40)", `transfer.people` "People (58 faces)", `transfer.dates` "Dates (14)", `transfer.places` "Places (9)", all ticked
  - `transfer.none` "Nothing to carry over: {to} has everything already, or no photo is on both drives."
  - `transfer.before_wipe` "Do this before you erase {from}: afterwards its data is gone." (shown when the source looks emptied: more than half of its files missing)
  - `transfer.done` "Done: {n} photos got tags, hearts, people, dates or places." with `transfer.nothing_new`
  - `transfer.backup` "{name} is a backup. Backups are not changed by the photo app." and `transfer.undecided` "Decide first whether {name} is a backup."
- **Export** (phase 13): when the destination is a shoebox drive, the finished
  dialog adds one line, `export.transfer_hint` "Scan {drive}, then use Carry over
  data on Manage drives to bring tags, hearts, people, dates and places."
  No checkbox in the export dialog.
- **All drives search**: no new text. The suggestions re-ask when a chip changes.

### 5.1 Counts that narrow (use case D)

`/api/all/tags` and `/api/all/people` take the same filter as
`/api/all/timeline` (`tag`, `person`, `q`, type, favorite, dates …, by name).
For each drive that takes part the server resolves the names to that drive's
ids (a drive lacking a name contributes nothing, as in the timeline), takes the
matching photo ids and counts with the same `tags_within` and people counting the
single-drive route uses. Counts are added by folded name; tags and people
that would leave nothing are dropped. The timeline shows identical photos twice,
and the counts add up the same way, so a count always equals what the result
then shows (phase 18 changes both together).

## 6. 15b: the same data from a file (own PR)

For when the source drive is gone, erased or lost its index.
`.shoebox/userdata.json` already holds the five boxes (and the other marks) with
each file's path and hashes and is written automatically; nothing reads it back
yet.

- A second reader for `transfer.rs`: `TransferSet` from a `userdata.json` (the file on
  the source drive, or a copy saved elsewhere), same dialog with **From a
  file…** in the drop-down, same apply, same tests with a file instead of a drive.
- It also gives the file a purpose it lacks today: restoring tags and decisions
  after a lost `library.db` (a rescan makes the files, the file brings the data).
- Needs: reading `userdata.json` versions 1 to 6, a file picker (the export's
  folder picker, extended to pick a file), and guide text.

Only the reader and the picker are new; nothing in 15a is redone.

## 6b. 15c: Ignore folders in the duplicates across drives (own PR)

**Use case E. A couple with two drives and shared vacation folders.** Noah has
`SSD-NOAH` with his folders and a few shared ones, Lena has `SSD-LENA` with
hers and the same shared ones: `2025-07 Kroatien/` (312 photos) and `2025-12
Weihnachten/` (190). They look at the common timeline together. *Today:* the
Across drives list shows 502 "duplicates" that are on purpose and hides the
real ones (the festival photo Noah left in `Downloads` that Lena also has in
`Handy 2025`).

**The solution: a list of folder names to ignore.** On the Across drives tab a
field "Ignore folders" takes folder names as chips that sit **inside** the
field (see "The chips input" below). On each copy row the folder name also gets
a small "Ignore this folder" action, so one click is enough. The mock-up is
[mocks/ignore-folders/2-ignore-folders.png](mocks/ignore-folders/2-ignore-folders.png).

- **Matching**: a copy is in an ignored folder when **any folder in its path**
  has that name (the shared folder or any folder inside it), compared NFC and
  case-folded, exact (no wildcards). It is by name, not by path, and the name
  only has to be found on **one** of the drives: the other drive's folder may
  be called something else.
- **Which photos disappear**: a photo with **at least one copy** in an ignored
  folder is left out of the list, with all its copies. Example: Noah's folder is
  `2025-07 Kroatien`, Lena keeps the same photos in `Urlaub Kroatien`. Ignoring
  `2025-07 Kroatien` hides every photo inside it, including Lena's copies in
  `Urlaub Kroatien`, because they are the same photos. So the list can only
  show a photo when none of its copies is in an ignored folder, and "Mark all
  copies on <drive>" can never tick a hidden copy.
- **The price of that rule**: a stray copy of a shared photo (Lena also dropped
  one into `Downloads`) is hidden too, as long as one copy lies in an ignored
  folder. That is the rule that was asked for, and the line under the field says
  how many photos are hidden, so it is not silent. There is no "show hidden"
  switch: taking the chip away (✕) shows them again and putting it back costs one
  click on the folder name in a row or one typed word.
- **Counts**: the hidden number is also used on the All drives page
  ("Duplicates across drives").
- **Nothing changes in any file or index**. It is a filter on the list. No hash
  is stored in a library, nothing is shared between drives, no schema bump.
- **Where the list lives**: the browser (`shoebox-dup-ignore`), like the Maps and
  Collection settings, and it travels with the request
  (`/api/all/duplicates?ignore=…`, repeated). The server filters
  (`multi::cross_duplicates`, a new `ignore` argument) and answers
  `hidden_groups`. Open question 5 asks whether it should live in the drives.
- **Scope**: the **Across drives** list only. "On this drive", the same-folder
  and lower-quality bulk buttons and "Delete the new copies" after a scan are
  not touched (open question 6).
- **Texts** (`dups.ignore.*`, EN and DE): `label` "Ignore folders",
  `placeholder` "Folder name, then Enter", `note` "Photos with a copy in a
  folder with one of these names are left out of this list, whatever the other
  copies are called. Nothing is deleted or changed. {count} hidden.",
  `remove` "Remove {name}" (the ✕ of a chip, for screen readers), `add` "Ignore
  this folder".
- **Tests** (`core/tests/multi.rs`): a group with one copy in an ignored folder is hidden with all its copies and counted,
  also when the other drive's folder has a different name; a group with no copy
  in an ignored folder stays; an ignored ancestor folder matches; NFC and case;
  nothing changes with an empty list; "On this drive" is unchanged.
  `core/tests/i18n.rs` for the keys.
- **Guide**: the Duplicates section (Across drives) and the All drives page.

**The chips input.** One bordered field that looks like a text input. The chips
sit inside it, left to right and wrapping onto more lines as they grow (the field
gets taller); the text cursor is always right after the last chip, and the
placeholder shows only while the field is empty. The pattern is the usual "chips
input" or "tag input" (Gmail's recipient field and Material Design's input chips
are well-known examples); we draw it ourselves with the app's own `.chip` style,
no library.

- **Type, Enter**: the text becomes a chip (trimmed, NFC, empty ignored, a name
  already there in another case ignored) and the cursor stays in the field for
  the next word. A comma does the same. Leaving the field with text in it also
  makes the chip, so typed text is never lost.
- **Remove**: ✕ on a chip, or Backspace in the empty field takes the last chip
  away.
- **Paste**: text with commas or line breaks becomes several chips.
- **Click** anywhere in the field puts the cursor in it; focus draws the ring
  around the whole field, not around the small input.
- **Suggestions** drop down below the field: see "Suggestions" right after this
  list.
- **Reusable component**: `chipsInput(options)` in `core/web/app.js`. It knows
  nothing about folders or duplicates: the chips, the keyboard and the dropdown
  are generic, and everything specific is **passed in** (see "The component's
  interface" below). 15c is its first user; the Settings lists of folder names
  (`settings.copy_folders`, now chips on top of a text box, no suggestions) can
  move to it later without changing it; not part of this change.
- **Checks**: by hand (long names, many chips wrapping, narrow window, iPad
  keyboard with `enterkeyhint="done"`) and in the guide's screenshot run; the
  server only receives the finished names, so no server test changes.

**The component's interface.** The dropdown is bound to the field from outside,
like an input binding: the caller says where the suggestions come from and how a
row looks; the component handles focus, keys, the list and the chips.

```js
var field = chipsInput({
  values: [],                    // chips to start with (strings)
  placeholder: tr('…'),          // shown only while there is no chip and no text
  label: tr('…'),                // aria-label of the field
  removeLabel: function (v) {},  // aria-label of a chip's ✕, e.g. tr('dups.ignore.remove', {name: v})
  normalize: function (text) {}, // text -> chip value or null; default: trim, NFC, empty -> null
  same: function (a, b) {},      // when two values are one; default: tag_fold-style compare
  suggest: function (text, taken) {},  // optional. Returns rows or a Promise of rows:
                                 //   [{ value, label, detail, ... }]; `taken` = values already chips
  renderRow: function (row, text) {},  // optional. A node for one row; default: label with
                                 //   the typed part bold, detail grey on the right
  freeRow: function (text) {},   // optional. Text of the last row ("Add “xyz”"); null = only
                                 //   suggested values can become chips
  delay: 120, maxRows: 8,        // wait between keys, rows shown
  onChange: function (values) {} // after every add or remove
});
parent.appendChild(field.el);    // field.values(), .set(list), .add(v), .focus(), .destroy()
```

- Without `suggest` there is no dropdown at all (the Settings lists).
- The component hides rows whose value is already a chip (`taken`, using `same`),
  throws away answers that arrive out of date, and calls `suggest` with the
  empty text when the field gets the focus.
- For 15c the caller's `suggest` calls `/api/all/duplicates/folders` and its
  `renderRow` adds the photo count and the drives; `freeRow` makes `Add “xyz”`.
  Another screen could pass a list from memory, or tags, and look the same.
- Placement is the component's job and is the same for every user: the list is
  attached to the **bottom edge of the field**, as wide as the field, with a few
  pixels between. It follows the field when chips wrap onto more lines and the
  field grows (it is positioned from the field, not at a fixed height). It overlays
  the page without pushing anything down, sits above sticky bars, and opens **above**
  the field when there is no room below in the window. (The first mock-up put it
  at the same height as the field; that was a mistake in the picture, which now
  sets it from the field's bottom edge and shows a field with two lines of chips.)

**Suggestions.** The field helps the user find the right folder name instead of
remembering it. Mock-up:
[mocks/ignore-folders/3-ignore-suggestions.png](mocks/ignore-folders/3-ignore-suggestions.png).

- **Where it comes from**: this section describes what the **15c caller** passes
  to `chipsInput`; the component itself is generic.
- **When**: the list opens when the field gets the focus (with the most useful
  names) and narrows while typing. Esc closes it; it also closes when the field
  loses focus.
- **What is offered**: names of folders that **occur in the paths of the photos on
  the Across drives list**: the folder a copy lies in and every folder above it
  (`2025-07 Kroatien`, `Noah`, `Gemeinsam`). Names already ignored (chips) are
  not offered. Free typing stays possible: a name that is not in the list can be
  added (the row at the bottom says `Add "xyz"`), for example before the photos
  are there.
- **What a row says**: the name, how many photos on the list it would hide
  (with the rule above: photos with at least one copy in a folder of that name),
  and on which drives the name occurs (`SSD-NOAH, SSD-LENA`). So the user sees the
  effect before picking: `2025-07 Kroatien · 312 photos · SSD-NOAH`, and
  `Urlaub Kroatien · 312 photos · SSD-LENA` are two suggestions for the same
  photos, and picking either is enough.
- **Order**: names that occur on more than one drive first (the likely shared
  ones), then by photos, then alphabetical. When text is typed: names that
  **start** with it first, then names that contain it; compared NFC,
  case-folded and without accents (`tag_fold`, as the tag suggestions do).
  At most 8 rows, the rest by typing more.
- **Keyboard**: Down and Up move the highlight, Enter picks the highlighted row
  (or, with none highlighted, makes a chip from the typed text), Esc closes. The
  mouse or a tap picks too, and the list stays open for the next name with the
  picked one gone from it. The highlighted row is announced to screen readers
  (`role="listbox"`, `aria-activedescendant`).
- **Route**: `GET /api/all/duplicates/folders?q=&ignore=&limit=8` answers
  `[{name, photos, drives}]`, built from the same cross-drive scan as the list,
  over **all** groups, not only the first 200 shown, and with the current ignore
  names applied (so counts are what is still visible). It is a read like
  `/api/all/tags?q=`, nothing is written. The field asks while typing (waits a
  moment between keys, drops answers that are out of date).
- **Texts** (`dups.ignore.*`): `suggest_photos` "{count} photos", `suggest_on`
  "on {drives}", `suggest_add` "Add “{name}”", `suggest_none` "No folder of that
  name in the list" (shown above the add row when nothing matches).
- **Tests** (`core/tests/multi.rs`): names from the leaf folder and from its
  ancestors; counts follow the any-copy rule and ignore names already chosen;
  counts cover groups beyond the 200 first; multi-drive names first; prefix
  before contains; NFC, case and accents; `limit`; an offline or backup drive
  adds nothing (as the list itself).

Not solved by it, and raised in open question 7: the **common timeline** still
shows a shared photo twice, once per drive, which is what the couple actually
sees every day. The ignore list only quiets the duplicates screen.

## 7. Not planned, and why

- **Moving photos between drives** inside shoebox (copy, verify, trash the
  source). It goes against the rule that file handling stays in Finder or
  Explorer, and needs a new copy path, journal and rule change in CLAUDE.md.
  Export covers copying a selection; Carry over covers the data.
- **"Keep both"** on the Across drives tab: a button to say "these identical
  copies on two drives are intentional" (per photo and for all shown), stored
  by hash in both drives' `library.db`. Dropped in favour of 15c: a name list
  is plainer, stores nothing about photos, and one folder name covers
  hundreds of photos. Its mock-up is kept in
  [mocks/ignore-folders/1-keep-both-placement.png](mocks/ignore-folders/1-keep-both-placement.png).
- An export checkbox "transfer shoebox data": replaced by Carry over, which also
  covers files moved in Finder.

## 8. Guide (EN and DE, kept in step)

- Multiple drives / Manage drives: new section **Carry over data** (when to use
  it, the five boxes, "before you erase the source", what is not carried, that
  nothing is deleted or overwritten).
- Export: the new hint line, and that data is carried separately.
- All drives search: the counts narrow.
- Safety rules: carry over reads another drive's index and writes only this
  drive's index.
- 15c: the Across drives list, "Ignore folders" (and that nothing is changed).
- Screenshot step for the dialog (CI makes the pictures);
  `python3 docs/guide/check.py`; say in the PR what changed in the guide.

## 9. Steps and tests

1. **Counts** (small): filtered `/api/all/tags` and `/api/all/people`, UI passes
   the filter. `core/tests/multi.rs`: for a filter the summed counts equal the
   timeline's length; a drive lacking a name adds nothing; empty tags drop out.
2. **Carry over** (15a): `transfer.rs`, API, dialog, hints, export line, guide.
   `core/tests/transfer.rs` (two temp drives, helpers as in `multi.rs`):
   - match by full hash across different names and folders; quick hash + size
     when a hash is missing; missing source files used; missing target files not;
   - each box alone: unticked boxes change nothing; folder tags never carried;
     the heart separate from tags; the collection tag carried;
   - nothing overwritten (target tags, dates, places, decisions win; an
     overlapping box keeps the target's decision); idempotent second run;
   - people by name (NFC, case) created when missing, no group, no cover;
   - two identical target copies both get the data;
   - one transaction: an injected failure leaves the target unchanged;
   - source `library.db` and photo files byte-identical afterwards (read-only);
   - backup and suspected backup refused both ways; offline drive answers 503
     (`core/tests/serve.rs`);
   - a schema test lists the tables read and fails when a new content-keyed
     table is neither carried nor listed as "not carried";
   - `core/tests/i18n.rs` checks the `transfer.*` keys.
3. **15b**: own PR (section 6).
4. **15c**: own PR (section 6b).
5. Real hardware: two real exFAT drives, a Finder copy, then Carry over.

## 10. Open questions (with my recommendation)

1. **Backup as source.** Refuse (as planned)? A backup mirrors its original, so
   the data is available there. *Recommend refuse*; carry from the original.
2. **Create missing people on the target.** *Recommend yes* (a person named on
   a trip would be lost otherwise); no group, no picture.
3. **Hints on Manage drives.** Show the "has data for N photos" line
   automatically, or only the button? *Recommend the line*: it costs one hash
   join and tells people the step exists after a Finder copy.
4. **Photos in the source's trash** (their tags are kept in `trash.user_tags`).
   *Recommend skip* in 15a.
5. **Where the ignore list lives (15c).** The browser (as planned), or the
   `library.db` of the drives so it follows the drives to another computer?
   *Recommend the browser* for now: no schema bump, same as Maps and Collection;
   the iPad would show the list again until the names are entered there.
6. **Scope of Ignore folders.** Only Across drives (as planned), or also "On
   this drive" and "Delete the new copies" after a scan? *Recommend only Across
   drives*: a copy twice on one drive is rarely on purpose, and the clean-up
   buttons delete files, so they should not get a second rule yet.
7. **Shared photos twice in the common timeline.** Decided: its own phase,
   [phase18.md](phase18.md) (same file name and same hash, a switch in All
   drives). Ignore folders does not help there. When it is built, the counts of
   15a count identical photos once (phase 18 section 2.1).
