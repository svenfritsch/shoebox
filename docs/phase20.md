# Phase 20: Hidden from the timeline

**Built, real-hardware check open** (branch `claude/phase19-hidden-plan`; the number 19 went to Groups on main). Photos you want kept out of the timeline (nude
pictures, documents, surprises) are marked *hidden*. They vanish from the photo
grid in every form (timeline, a folder, a person, a date, a type, a tag, a
search) and come back only when you ask for them: the Hidden chip, the word
*hidden*, or a click on a hidden folder in the sidebar. Like Favorites (see
"Favorites" in [plan.md](plan.md)), but it removes instead of marks.

It is about the **timeline**, the part you may show to family and friends. It
is **tidiness, not privacy**, and shoebox does not try to be more: the files
stay in plain view on the drive, and keeping them private is a matter for the
drive (a password, encryption). Libraries kept next to the program (phase 16) are
read-only and for shared drives, so nothing private is expected there. The guide
says in one sentence that hiding only keeps photos out of the timeline.

Mock-ups (static, on the app's own CSS; `docs/mocks/hidden/build.py` and
`shoot.mjs` make them, English and German): `01-search`, `02-folder-menu`,
`03-hidden-folder-menu`, `04-chips`, `05-viewer-hide`, `06-viewer-hidden`,
`07-tag-box-hidden`, `08-tag-box-favorite`, `09-selection-bar` (`-en` / `-de` `.png`).

## 1. What the user sees

| Where | What |
|---|---|
| Viewer top bar | New icon button right after the heart. The icon is the action: the eye with a slash (the one of the face "Ignore", `IGNORE_ICON`) hides, the same eye without the slash shows the photo again. Key `h`. Tip "Hide from timeline (h)" / "Show in timeline (h)". Lit in the accent colour when the photo is hidden. In All drives it is shown and disabled, like the heart |
| Selection bar | Icon button (no text): the eye with a slash and the tip "Hide from timeline"; the plain eye and "Show in timeline" when every selected photo is hidden already |
| Sidebar folder | **Right-click** on a folder row opens a menu with one entry: "Hide folder from timeline" / "Show folder in timeline" (new: today only photos have a context menu). No extra button in the row; there is no room for one |
| Folder chip (the opened folder, where the pencil is) | An eye button left of the pencil: the same action and the same two icons (slash = hide, plain = show again, lit when the folder is hidden), and the way on the iPad, where there is no right-click. Spacing between eye, pencil and ✕ is tighter on desktop (3 px padding per button, 2 px gap) and a little wider where the pointer is coarse (`@media (pointer: coarse)`: 6 px, 4 px); mock `04-chips` and `04b-chips-tablet`. It stays in a read-only library (the flag is in the index, not in the folder), the pencil does not |
| Hidden folder in the sidebar | Stays in the tree, **dimmed** with a small slashed eye before its count, and stays clickable. Its parents' counts do not include it |
| Click a hidden folder (or one below it) | Its photos are shown |
| Hidden chip | Like the Favorites chip: the slashed eye, "Hidden" / "Ausgeblendet", ✕. Reached by a search suggestion, by typing a word for it, or by the sidebar's one "Hidden" row under Tags. AND with the other chips: *Hidden + Anna* finds Anna's hidden photos. (No extra button next to Type and the heart; say if you want one.) |
| Info panel | No tag chip. Only for a photo hidden by its **folder** a row "Hidden: via folder Privat", because the viewer button cannot undo that (it shows lit and disabled with the hint) |
| Typing a word for it in the tag box | The photo is hidden, no tag appears, and the app's normal toast says so (section 3) |
| Move dialog | A line when the target folder is hidden (photos get hidden) or the photos come from a hidden folder (they show again) |

Rule in one line: *a hidden photo is in the grid only if the Hidden chip is on,
or if the selected folder is itself hidden (its whole subtree is then shown).* A
photo tagged hidden in a normal folder is shown by the chip only, not by clicking
its folder.

### What is **not** affected (administrative pages show everything)

Duplicates (all tabs, "Delete the new copies"), the face and pet checks and the
Unnamed lists, Calibration, Stats, Trash, Manage drives and the backup check,
Settings. Counts that sit next to a grid filter (sidebar folder, tag and person
counts, suggestion counts, the year strip) follow the grid, so a count always
matches what a click shows.

## 2. Data

- **Hidden photo = own tag `hidden`** (`tags::HIDDEN`, source `user`), like
  `tags::FAVORITE`: stored in English only, reserved. `tags::check_name` maps
  `hidden`, `versteckt`, `verborgen` and `ausgeblendet` (the word the German UI
  shows) to it, so there is no second tag. The UI never lists it as a tag. A folder
  called "hidden" does not hide its photos (only the own tag counts). It travels in
  `userdata.json` and by "Carry over data" like any own tag, and survives moves,
  rescans and the trash.
- **Hidden folder = a row in a new table** `hidden_folders(path_nfc TEXT PRIMARY KEY,
  added_at INTEGER)` in `library.db` (schema v18), keyed by NFC path so it survives
  rescans. A folder is hidden if it or an ancestor has a row. Listed in
  `transfer::TRANSFERRED_TABLES`: carried by path under the Hidden box (below). Nothing is written into a file or a folder.
- Moving a photo into a hidden folder hides it; out of it shows it again.
- **Backups.** The flags live in each drive's own `library.db`. They are not copied
  to a backup automatically; "Carry over data" can bring them over **if the user ticks
  it** (below). A backup is not part of the timeline of All drives, so nothing hides
  there. A drive on its own timeline (including a backup opened by itself) works like
  any drive: the viewer shows the heart and the hide button there and both work, as
  the heart does today (only All drives shows them disabled).
- **Carry over data** (phase 15a/b): a sixth check box **"Hidden ({n})"**, **not ticked by
  default** (the others are). It brings the own tag `hidden` by content, and the hidden
  folder flags by path for folders that exist on the target. The "Tags" box does not
  carry `hidden` (as it does not carry `favorite`; `transfer.rs` tells them apart with
  `means_favorite` today and gets `means_hidden` next to it). `userdata.json` includes
  both and obeys the same box. `transfer::Boxes` gets `hidden: bool` (default false).

## 3. The notice after typing a reserved word in the tag box

Typing `favorite`, `favorit(en)` (today) or `hidden`, `versteckt`, `verborgen`,
`ausgeblendet` (new) in the info panel's tag box, or in the selection bar's
"Add tag…" dialog, gives the heart or hides the photo and creates no tag. Today nothing
says so. New: the app's normal `toast()` (the dark-on-light pill under the top bar, as
"82 faces named Tesselina"; four seconds), one line. Mocks `07-tag-box-hidden`,
`08-tag-box-favorite`.

| Option | Favorite, English | Favorite, German | Hidden, English | Hidden, German |
|---|---|---|---|---|
| **A (mocked)** | Favorite set. Favorites show as a heart, not as a tag. | Favorit gesetzt. Favoriten erscheinen als Herz, nicht als Tag. | Hidden from the timeline. Marked through the eye button, not a tag. | Aus der Zeitleiste ausgeblendet. Erkennbar durch das Auge, kein Tag. |
| B | That is the heart: the photo is a favorite now, no tag is added. | Das ist das Herz: Das Foto ist jetzt ein Favorit, es wird kein Tag angelegt. | Hidden from the timeline, no tag is added. The eye button does the same. | Aus der Zeitleiste ausgeblendet, es wird kein Tag angelegt. Das macht auch der Auge-Button. |
| C (shortest) | Favorite set (no tag). | Favorit gesetzt (kein Tag). | Hidden (no tag). | Ausgeblendet (kein Tag). |

## 4. Where the rule lives

Everything list-like in the grid goes through `browse::Snapshot::query`
(`core/src/browse.rs`; timeline, counts, tag suggestions, year strip, search).

1. `Snapshot::load` loads the hidden folder paths and the ids of files with the
   `hidden` tag, and marks each `Item` `hidden: bool`.
2. `browse::Query` gets `hidden: bool` (the chip; `hidden=1` in the URL hash and API,
   like `fav=1`). `Snapshot::query` drops hidden items unless `q.hidden` or the item
   lies in the subtree of `q.folder` and that folder is hidden. With `q.hidden` it
   keeps only hidden items.
3. The typed word goes through `is_hidden_word` next to `is_favorite_word` and becomes
   `hidden: true`, so typing it does not also search the file names.
4. Grid-side paths that bypass `Snapshot::query` need the rule too (each a test):
   `browse::all_tags` (the unfiltered SQL), the sidebar's people and pet counts and a
   person's photo list, Places and the pins' clusters, All drives (`multi.rs`,
   `serve::forward`), the backup comparison filters, the viewer's next/previous.
   Administrative pages (section 1) are left alone on purpose. A grep for direct
   `FROM files` in `serve.rs`, `people.rs`, `geo.rs`, `multi.rs` completes the list.
5. **Export** (phase 13). The Export button next to a tag chip is not shown while the Hidden chip is on;
   hidden photos are exported from a selection, like any other. Nothing is filtered: an export is what is shown.

## 5. Decisions and what is not done

1. **Same photo on two drives, hidden on one** (All drives): each drive's own flags count; the
   photo shows once through the copy that is not hidden (`multi::shown_first` among what is left).
   Hidden on both: gone, and once for the chip.
2. `ausgeblendet` is accepted as a typed word next to `versteckt` and `verborgen` (it is the word
   the UI shows).
3. The bulk "Add tag…" dialog gets the toast after OK, no live note (a toast stays on one line).
4. A backup opened by itself works like any drive (heart and hide button work); no special case.
5. **Not done:** the people counts in the sidebar and the faces of hidden photos on the People
   pages still count hidden photos (`people.rs` counts by content, in a cache the Unnamed page
   shares); a person's photo grid does leave them out. Decide whether the counts should follow
   the grid like the folder and tag counts do.
6. **Not tested in the page:** the Hidden box of Carry over data in the dialog (the server side
   is tested in `core/tests/transfer.rs`) (the fake API has no transfer route).

## 6. Translation strings (new keys, `core/i18n/en.json` and `de.json`)

| Key | English | German | Where |
|---|---|---|---|
| `hidden.label` | Hidden | Ausgeblendet | chip, suggestion head and item, sidebar row |
| `hidden.hide_hint` | Hide from timeline (h) | Aus der Zeitleiste ausblenden (h) | viewer button tip |
| `hidden.show_hint` | Show in timeline (h) | In der Zeitleiste anzeigen (h) | viewer button tip, photo is hidden |
| `hidden.readonly` | Hidden (change it on the photo's own drive) | Ausgeblendet (auf dem Laufwerk des Fotos ändern) | viewer button in All drives |
| `hidden.by_folder_hint` | Hidden by its folder. Show the folder again, or move the photo out. | Durch den Ordner ausgeblendet. Den Ordner wieder anzeigen oder das Foto herausschieben. | viewer button, photo hidden by folder |
| `sel.hide` | Hide from timeline | Aus der Zeitleiste ausblenden | selection bar tip and aria label |
| `sel.unhide` | Show in timeline | In der Zeitleiste anzeigen | selection bar tip, all selected hidden |
| `hidden.folder_hide` | Hide folder from timeline | Ordner aus der Zeitleiste ausblenden | folder right-click menu, chip button tip |
| `hidden.folder_show` | Show folder in timeline | Ordner in der Zeitleiste anzeigen | same, folder is hidden |
| `hidden.folder_row_hint` | Hidden folder: its photos are not in the timeline | Ausgeblendeter Ordner: Seine Fotos sind nicht in der Zeitleiste | tip of a dimmed sidebar row |
| `info.hidden` | Hidden | Ausgeblendet | info panel row label |
| `hidden.via_folder` | via folder {folder} | über den Ordner {folder} | info panel row value |
| `hidden.filter_hint` | Show only hidden photos | Nur ausgeblendete Fotos zeigen | tip of the Hidden chip and sidebar row |
| `hidden.toast_hidden` (one / other) | 1 photo hidden from the timeline / {n} photos hidden from the timeline | 1 Foto aus der Zeitleiste ausgeblendet / {n} Fotos aus der Zeitleiste ausgeblendet | after hiding |
| `hidden.toast_shown` (one / other) | 1 photo shown in the timeline again / {n} photos shown in the timeline again | 1 Foto wieder in der Zeitleiste / {n} Fotos wieder in der Zeitleiste | after showing |
| `hidden.move_into` | The target folder is hidden: these photos will be hidden. | Der Zielordner ist ausgeblendet: Diese Fotos werden ausgeblendet. | move dialog |
| `hidden.move_out` | These photos come from a hidden folder: they will show in the timeline again. | Diese Fotos stammen aus einem ausgeblendeten Ordner: Sie erscheinen wieder in der Zeitleiste. | move dialog |
| `tags.reserved_fav` | Favorite set. Favorites show as a heart, not as a tag. | Favorit gesetzt. Favoriten erscheinen als Herz, nicht als Tag. | toast after typing a favorite word (option A) |
| `tags.reserved_hidden` | Hidden from the timeline. Marked through the eye button, not a tag. | Aus der Zeitleiste ausgeblendet. Erkennbar durch das Auge, kein Tag. | toast after typing a hidden word (option A) |
| `transfer.hidden` | Hidden ({n}) | Ausgeblendet ({n}) | Carry over data, check box, not ticked by default |
| `transfer.hidden_hint` | Not ticked by default: hiding is decided per drive. | Standardmäßig nicht angehakt: Ausblenden wird pro Laufwerk entschieden. | tip of that box |

Search words (not strings, in `tags::is_hidden_word`): `hidden`, `versteckt`,
`verborgen`, `ausgeblendet`, matched by prefix of 3+ letters like the favorite words.

## 7. Steps

1. Core: `tags::HIDDEN`, `check_name` mapping, `is_hidden_word`, `set_hidden`, `hidden_ids`;
   schema v18 `hidden_folders`, `tags::set_folder_hidden`; the table in
   `NOT_TRANSFERRED_TABLES`. Test `hidden_is_the_own_tag_hidden_and_a_folder_flag_is_inherited`
   in `core/tests/tags.rs`.
2. `browse.rs`: `Item.hidden`, `Query.hidden`, the visibility rule, the word loop.
   Tests: not in the timeline, not under a person, date, type or tag, shown by the chip,
   shown when a hidden folder is selected, counts and `tags_within` agree.
3. `serve.rs`: `hidden_of(pairs)`, `POST /api/hidden {ids, on}`,
   `POST /api/hidden/folder {path, on}`, `hidden` in the timeline response (like `favs`),
   the bypassing paths from 4.4, the export rule from 4.5.
4. `people.rs`, `geo.rs`, `multi.rs`, export: the same rule, one test each.
5. UI (`core/web/app.js`, `app.css`, `index.html`): viewer button and key `h` (slash eye / plain eye by state), selection
   bar icon button, chip spacing for fine and coarse pointers, folder row context menu (`showMenu`), chip button, dimmed hidden
   folders, Hidden chip, suggestion and sidebar row, info row, the tag-box toast, move
   dialog lines, the "Hidden" box of Carry over data (`transfer.rs`, `transfer::Boxes`, its test). The icons are the existing `IGNORE_ICON` and the same drawing without its slash (ours, no new licence question). i18n keys in both files.
6. Guide EN and DE (the `guide` skill): what hiding does, tidiness not privacy, the folder
   rule, backups, the tag-box note; `python3 docs/guide/check.py`.
7. `CLAUDE.md` rule: hidden is the own tag plus `hidden_folders` in `library.db` only,
   never in a file; no guard change (no original is touched). `plan.md` row 20.
8. Browser test (keyboard: `h`, chip, folder click) in `core/tests/browser/`.
9. Real-hardware check: hide photos and a folder on the iPad (chip button), look at the
   timeline, a person, Places and the counts; copy a photo into a hidden folder and scan.

## 8. Tests (written with the change)

- `tags.rs` unit tests: `is_hidden_word`, `means_hidden`, `check_name` mapping (`hidden`,
  `versteckt`, `verborgen`, `ausgeblendet` become `hidden`; `Hiddenstreet` stays a name).
- `core/tests/tags.rs` `hidden_is_the_own_tag_hidden_and_a_folder_flag_is_inherited`: the
  timeline, a person, a tag, a date and a type leave hidden photos out; the Hidden chip and the typed
  word show them; a hidden folder (and a folder below it) shows when selected; a photo tagged hidden in
  a normal folder does not; folder counts (the hidden folder keeps its own, parents drop it);
  a moved photo follows its new folder; a rescan keeps the flags and hides a photo copied into a hidden folder.
- `core/tests/serve.rs`: `POST /api/hidden` and `/api/hidden/folder` (replies, a broken body refused,
  a read-only library accepts them), `hidden=1`, `hids` in the timeline, `hidden` and `hidden_own` in the folder list,
  tag counts that leave hidden photos out, the info reply, export by tag leaving them out.
- `core/tests/multi.rs`: All drives hides them, `hidden=1` lists them from every drive (tag and folder),
  a photo hidden on one drive only is hidden in the common timeline, suggestion counts agree with the list.
- `core/tests/transfer.rs`: the "Hidden" box (off by default), the Tags box does not carry `hidden`,
  folder flags by path, `userdata.json` round trip.
- Browser, mocked (`faces-mocked.mjs` with `fake-api.mjs`): key `h`, the selection bar button and its toggle,
  the folder menu and chip button, the Hidden chip, the toast after typing a reserved word.
- Each new test is shown to fail with the change broken, and run several times.
