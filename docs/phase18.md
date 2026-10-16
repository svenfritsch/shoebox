# Phase 18: identical photos once in the common timeline

**Planned, nothing built.** Phase 16 is not used; phase 17 is the Windows release (main).
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
| The two copies disagree on the date (one has an estimated date) | The shown copy's date decides; the info panel says where the other copy is |
| One copy has the heart, the tag or the face decision | Filters are applied per drive first, then identical results are merged; the shown copy is one that matches the filter. The heart and tags shown are those of the shown copy |
| A backup drive | Not part of the common timeline already, so nothing changes |
| The user wants to see both (to compare or to find a stray) | A switch turns the merging off |

## 2. Solution

A switch in the All drives timeline: **Show identical photos once**, **on by
default** (it is the fix; off shows every copy as today).

- **Identical** means: same **file name** (last path component, NFC, case-folded,
  never the folder) **and** the same **full hash**, both known. Nothing else
  (not a picture hash, not size alone, not name alone).
- **Which copy is shown**: the one on the drive that comes first in the drive
  list; among copies on the same drive, the first by path. Always the same copy,
  so the timeline does not jump between reloads.
- **Where the others are**: the info panel of a shown photo gets a row **Also
  on** with one line per other copy (`SSD-LENA · Urlaub Kroatien`), each a
  button that opens that photo on its own drive (like the existing button that
  opens the photo's drive).
- **Status line**: "4,310 photos · 502 identical copies hidden" (the second part
  only while the switch is on and something is hidden).
- **Counts**: the search suggestions (phase 15a) count each identical photo once
  while the switch is on, so a count still equals what the result shows.
- **Not merged**: the single-drive timelines (a folder there must show all its
  files) and the backup comparison view (`compare=different`, `flat`), which
  exists to show both copies. See open question 1 for single drives.
- **Read only**: nothing is written anywhere; no schema change.

### 2.1 How

`GET /api/all/timeline` gets `once=1` (the browser sends it from the switch,
`shoebox-once`, default on). After the per-drive queries and before sorting, the
server looks up `(name_fold, full_hash)` for the rows it has (one query per drive
over `files`, kept in a map by file id), keeps the first row of every
`(name_fold, full_hash)` key in drive order, and answers `hidden` (how many it
dropped). Rows without a hash are kept. The `live` link of a kept row is its
own. A new `GET /api/all/files/{gid}/copies` gives the "Also on" lines (the other
files with the same name and hash on the online non-backup drives).

`/api/all/tags` and `/api/all/people` take the same `once` and count distinct
`(name_fold, full_hash)` keys per name instead of adding up per drive.

### 2.2 UI

- A toggle in the All drives header next to the type filter, an icon of two
  overlapping photos, tooltip `once.tip`. State in `localStorage`
  (`shoebox-once`), on by default.
- Texts (`once.*`, EN and DE in `core/i18n/`): `label` "Identical photos once",
  `tip` "A photo with the same name and the same content on more than one drive is
  shown once. Photos with different names count as different.", `hidden`
  "{count} identical copies hidden", `also` "Also on".
- Info panel row `once.also`, as above.

## 3. Guide

(EN and DE, per the `guide` skill.) All drives timeline: the switch, what
"identical" means (name and content), where the other copies are, the status
line, and that nothing is deleted or changed. Remove the guide's remark that
identical photos show twice, if there is one. `python3 docs/guide/check.py`.

## 4. Tests

`core/tests/multi.rs` (two temp drives, as in the existing tests):
- same name and content in different folders on two drives: one row, `hidden` = 1;
  the shown copy is the one on the first drive;
- same content, other name: two rows; same name, other content: two rows;
- a missing full hash: two rows;
- `once` off or missing: both rows, as today;
- a tag or heart only on the second drive's copy with the tag filter: the photo
  shows once; counts for the tag equal the result's length;
- `compare=different` and a backup are not merged;
- `/api/all/files/{gid}/copies` lists the other copies and nothing from a backup;
- case and Unicode in names (`IMG_1.JPG` vs `img_1.jpg`, decomposed umlauts).

`core/tests/i18n.rs` for the keys. Real data: the couple's two drives.

## 5. Open questions (with my recommendation)

1. **Single-drive timelines too?** The same name and content can lie in two
   folders of one drive (`Weihnachten/IMG_1.jpg` and `Kochen/IMG_1.jpg`).
   *Recommend no*: in a folder every file must show, and the Duplicates screen is
   the tool for that. Only All drives merges.
2. **On by default?** *Recommend on*, with the switch remembered.
3. **Which copy is shown.** First drive in the list (as planned), or the one with
   most user data (tags, hearts, people)? *Recommend the first drive*: simple and
   stable; the "Also on" row is there for the rest.
4. **Same content, different name** (use case C): stays two photos. *Recommend
   keep it that way*; a later "also merge renamed copies" can follow if wanted.
