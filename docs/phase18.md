# Phase 18: identical photos once in the common timeline

**Built** (branch `claude/phase18-identical-once`). Phase 16 is not used; phase 17 is the Windows release (main).
Decided with the user before building: no switch (section 5), and a date the
user gave decides which copy is shown.
Closes the last line of "Not yet done" in [phase6.md](phase6.md) section 3:
*identical photos on two separate drives show twice in the common timeline*.
Raised in [phase15.md](phase15.md) open question 7.

## 1. Use cases

**A. The couple's common timeline.** Noah's `SSD-NOAH` and Lena's `SSD-LENA` both
hold the shared trip photos: `2025-07 Kroatien/IMG_4021.jpg` on one drive,
`Urlaub Kroatien/IMG_4021.jpg` on the other (same file, 312 photos). They open
**All drives** to watch together. *Today:* every Kroatien photo appears twice in
a row, the year counts are inflated, and swiping through the viewer shows each
picture twice.

**B. The laptop drive and the family drive.** `LAPTOP` and `FAMILIE` both hold
`2025-12 Weihnachten/` (190 photos), copied by hand. Same problem, a bit
less visible because the drives sit apart in the sort order.

**C. Same photo, new name.** Lena renamed `IMG_4021.jpg` to `Strand.jpg` on her
drive. The content is identical, the name is not. *Wanted:* both stay in the
timeline. Only a file with the **same name and the same content** counts as
"the same photo here"; anything else may be a deliberate second file and is
left to the Duplicates screen.

**D. Search in All drives.** Sofia searches `Anna` + `Winter`. If the tag is
only on Noah's copy, the photo must still show (once), and the counts under the
search box must match what the result shows.

**What can go wrong**

| Case | Handling |
|---|---|
| A file has no full hash yet (just scanned) | Cannot be called identical; shown as it is until hashing has run |
| Same name, different content (two cameras, `IMG_0001.jpg`) | Different photos, both shown |
| Same content, different name | Both shown (use case C) |
| The two copies disagree on the date (one has a corrected or estimated date) | The copy with the date the user gave is shown, so the photo sits where the user put it; the info panel says where the other copy is |
| One copy has the heart, the tag or the face decision | Filters are applied per drive first, then identical results are merged; the shown copy is one that matches the filter. The heart shows if any copy has one (All drives is read only, so there is no question which copy a click changes); tags and people in the info panel are those of the shown copy |
| A backup drive | Not part of the common timeline already, so nothing changes |
| The user wants to see both (to compare or to find a stray) | **Duplicates across drives** (Manage drives) lists every copy, renamed ones too; the "Also on" row names the others and opens them. No switch |

## 2. Solution

The All drives timeline always shows identical photos once. There is no switch
(section 5, question 2).

- **Identical** means: same **file name** (last path component, NFC, case-folded,
  never the folder) **and** the same **full hash**, both known. Nothing else
  (not a picture hash, not size alone, not name alone).
- **Which copy is shown** (`multi::shown_first`), among the copies the filter
  left:
  1. the one with a date the user gave (`taken_overrides` or `date_estimates`);
     if several have one, the one given last (largest `at`);
  2. else the one on the drive that comes first in the drive list;
  3. among copies on the same drive, the first by path.

  Always the same copy, so the timeline does not jump between reloads.
- **Heart**: the shown photo has the heart when any copy on a drive of the
  timeline has it. With the heart filter the copy that has it is shown.
- **Where the others are**: the info panel of a shown photo gets a row **Also
  on** with one line per other copy (`SSD-LENA · Urlaub Kroatien`), each a
  button that opens that folder on its own drive (like the existing button that
  opens the photo's drive).
- **Status line**: "4,310 photos on 2 drives · … · 502 identical copies hidden"
  (the last part only when something is hidden).
- **Counts**: the search suggestions (phase 15a) count each identical photo
  once, with and without a filter, so a count equals what the result shows.
- **Not merged**: the single-drive timelines (a folder there must show all its
  files) and the backup comparison view (`compare=different`, `flat`), which
  exists to show both copies.
- **Read only**: nothing is written anywhere; no schema change.

### 2.1 How

`App::copy_facts` (cached like the snapshot, by index version) maps every file
that is there and has a full hash to its `(name_fold, full_hash)` key and the
`at` of a date the user gave (`multi::copy_facts`). `GET /api/all/timeline`,
after the per-drive queries and before sorting, keeps one row per key
(`multi::shown_first`), sets the heart from any copy, and answers `hidden`
(how many it dropped). Rows without a hash are kept. The `live` link of a kept
row is its own.

`GET /api/all/copies?library=<id>&file=<id>` gives the "Also on" lines: the
other files with the same key on the drives of the timeline (online, not
backups), with drive, disk name, folder id and path.

`/api/all/tags` and `/api/all/people` count distinct photos (`multi::Photo`: the
key, or the file itself when it has none) per name over the photos the
timeline shows, instead of adding up per drive. Tags go through
`browse::tag_files_within` (`tags_within` with the files).

### 2.2 UI

- No toggle.
- Texts (EN and DE in `core/i18n/`): `info.also_on` "Also on",
  `info.also_on_hint` (tooltip of a line), `count.hidden_copies` "{n} identical
  copies hidden".
- Info panel row `info.also_on` after the Drive row, filled when
  `/api/all/copies` answers (cached per photo until the timeline reloads).

## 3. Guide

(EN and DE, per the `guide` skill.) All drives timeline: what
"identical" means (name and content), which copy is shown, where the other
copies are, the status line, and that nothing is deleted or changed. Remove the guide's remark that
identical photos show twice, if there is one. `python3 docs/guide/check.py`.

## 4. Tests

`core/tests/multi.rs`, `identical_photos_show_once_in_the_common_timeline`
(two drives and a backup):
- same name and content in different folders: one row, `hidden` counts it; the
  shown copy is the one on the first drive; the same on every reload;
- same content, other name: two rows; same name, other content: two rows;
- a missing full hash: two rows;
- case and Unicode in names (`IMG_1.JPG` vs `img_1.jpg`, decomposed umlaut);
- a tag only on the second drive's copy: the photo shows once (that copy);
  tag and person counts equal the result's length;
- a heart on the second copy shows on the first; the heart filter shows the
  second;
- a date given on the second copy makes it the shown one, at that date; a
  newer date on the first makes the first win again;
- `/api/all/copies` lists the other copies (also by case), nothing from a
  backup, nothing for a renamed copy, 404 for an unknown drive;
- no original changed.

The existing tests (`compare=different` in `backup.rs`, the suggestions in
`multi.rs`) still pass. `core/tests/i18n.rs` for the keys. Real data: the
couple's two drives.

## 5. Open questions (decided)

1. **Single-drive timelines too?** No: in a folder every file must show, and
   the Duplicates screen is the tool for that. Only All drives merges (copies
   with the same name and content in two folders of one drive are merged there
   too, the first by path shown).
2. **A switch?** No. Comparing copies is what **Duplicates across drives** is
   for (it also finds renamed copies), the "Also on" row says where the others
   are and opens them, and All drives is read only, so there is nothing to do
   on a particular copy there.
3. **Which copy is shown.** The one with a date the user gave (the last given),
   else the first drive. A corrected or estimated date says where the photo
   belongs; showing the other copy would put it back at the old date.
4. **Same content, different name** (use case C): stays two photos; a later
   "also merge renamed copies" can follow if wanted.
