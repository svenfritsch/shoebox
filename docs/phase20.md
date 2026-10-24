# Phase 20: Hidden from the timeline

**Planned, nothing built.** Photos you want kept out of the timeline (nude
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
| Viewer top bar | New icon button right after the heart: a closed eye (our own drawing, unlike the open eye with a slash of the face "Ignore"). Key `h`. Tip "Hide from timeline (h)" / "Show in timeline (h)". Lit in the accent colour when the photo is hidden. In All drives it is shown and disabled, like the heart |
| Selection bar | Icon button (no text) with the tip "Hide from timeline"; "Show in timeline" when every selected photo is hidden already |
| Sidebar folder | **Right-click** on a folder row opens a menu with one entry: "Hide folder from timeline" / "Show folder in timeline" (new: today only photos have a context menu). No extra button in the row; there is no room for one |
| Folder chip (the opened folder, where the pencil is) | A closed-eye button left of the pencil: the same action, and the way on the iPad, where there is no right-click. Lit when the folder is hidden. It stays in a read-only library (the flag is in the index, not in the folder), the pencil does not |
| Hidden folder in the sidebar | Stays in the tree, **dimmed** with a small closed eye before its count, and stays clickable. Its parents' counts do not include it |
| Click a hidden folder (or one below it) | Its photos are shown |
| Hidden chip | Like the Favorites chip: eye icon, "Hidden" / "Ausgeblendet", ✕. Reached by a search suggestion, by typing a word for it, or by the sidebar's one "Hidden" row under Tags. AND with the other chips: *Hidden + Anna* finds Anna's hidden photos. (No extra button next to Type and the heart; say if you want one.) |
| Info panel | No tag chip. Only for a photo hidden by its **folder** a row "Hidden: via folder Privat", because the viewer button cannot undo that (it shows lit and disabled with the hint) |
| Typing a word for it in the tag box | The photo is hidden, no tag appears, and a muted note line says so (section 3) |
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
  added_at INTEGER)` in `library.db` (schema v16), keyed by NFC path so it survives
  rescans. A folder is hidden if it or an ancestor has a row. Listed in
  `transfer::NOT_TRANSFERRED_TABLES` (a path is not content; the own tag part does
  travel). Nothing is written into a file or a folder.
- Moving a photo into a hidden folder hides it; out of it shows it again.
- **Backups.** The flags live in each drive's own `library.db` and are never copied
  to a backup. A backup connected **alone** behaves like an original: its own timeline,
  its own hiding. With original and backup connected, the original's flags apply to
  the original's timeline and the backup's timeline shows its own (unhidden) photos.

## 3. The notice after typing a reserved word in the tag box

Typing `favorite`, `favorit(en)` (today) or `hidden`, `versteckt`, `verborgen`,
`ausgeblendet` (new) in the info panel's tag box, or in the selection bar's
"Add tag…" dialog, gives the heart or hides the photo and creates no tag. Today
nothing says so. New: a muted note line under the tag row (the same plain muted
text the Unnamed page uses for its summary line; `.note` in the panel; in the
dialog under the input, shown as soon as the word is recognised, before OK).

| Option | Favorite, English | Favorite, German | Hidden, English | Hidden, German |
|---|---|---|---|---|
| **A (mocked)** | That is the heart: the photo is now a favorite. The heart above shows it, no tag is added. | Das ist das Herz: Das Foto ist jetzt ein Favorit. Das Herz oben zeigt es an, ein Tag wird nicht angelegt. | Hidden from the timeline. The eye button above shows it, no tag is added. | Aus der Zeitleiste ausgeblendet. Das Auge oben zeigt es an, ein Tag wird nicht angelegt. |
| B | Marked as a favorite. Favorites are shown as a heart, not as a tag. | Als Favorit markiert. Favoriten werden als Herz gezeigt, nicht als Tag. | Hidden from the timeline. Hidden photos are shown by the eye button, not as a tag. | Aus der Zeitleiste ausgeblendet. Ausgeblendete Fotos zeigt der Augen-Button, nicht ein Tag. |
| C (shortest) | Favorite set (the heart above). No tag was added. | Favorit gesetzt (das Herz oben). Es wurde kein Tag angelegt. | Hidden (the eye above). No tag was added. | Ausgeblendet (das Auge oben). Es wurde kein Tag angelegt. |

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
5. **Export** (phase 13). The button that shows when a single tag chip is active
   (the collection) leaves hidden photos out and says "n hidden photos left out";
   the selection bar's Export still exports hidden photos that are selected (they
   are selected only while the Hidden chip or a hidden folder shows them).

## 5. Open questions

1. **Same photo on two drives, hidden on one** (All drives, phase 18 shows it once).
   Proposal: hidden if hidden on any drive that has it. Check at build time how a
   backup takes part in All drives today; if it is left out, there is nothing to decide.
2. Typing `ausgeblendet` is accepted as a word although you named two German terms
   (`versteckt`, `verborgen`): it is the word the UI shows, so a user who copies it
   must find it. Say if you want it out.
3. Whether the bulk "Add tag…" dialog gets the live note (proposal: yes).

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
| `export.hidden_left_out` (one / other) | 1 hidden photo left out / {n} hidden photos left out | 1 ausgeblendetes Foto ausgelassen / {n} ausgeblendete Fotos ausgelassen | export dialog of a tag |
| `tags.reserved_fav` | option A above | option A above | tag box note |
| `tags.reserved_hidden` | option A above | option A above | tag box note |

Search words (not strings, in `tags::is_hidden_word`): `hidden`, `versteckt`,
`verborgen`, `ausgeblendet`, matched by prefix of 3+ letters like the favorite words.

## 7. Steps

1. Core: `tags::HIDDEN`, `check_name` mapping, `is_hidden_word`, `set_hidden`, `hidden_ids`;
   schema v16 `hidden_folders`, `tags::set_folder_hidden`; the table in
   `NOT_TRANSFERRED_TABLES`. Test `hidden_is_the_own_tag_hidden_and_a_folder_flag_is_inherited`
   in `core/tests/tags.rs`.
2. `browse.rs`: `Item.hidden`, `Query.hidden`, the visibility rule, the word loop.
   Tests: not in the timeline, not under a person, date, type or tag, shown by the chip,
   shown when a hidden folder is selected, counts and `tags_within` agree.
3. `serve.rs`: `hidden_of(pairs)`, `POST /api/hidden {ids, on}`,
   `POST /api/hidden/folder {path, on}`, `hidden` in the timeline response (like `favs`),
   the bypassing paths from 4.4, the export rule from 4.5.
4. `people.rs`, `geo.rs`, `multi.rs`, export: the same rule, one test each.
5. UI (`core/web/app.js`, `app.css`, `index.html`): viewer button and key `h`, selection
   bar icon button, folder row context menu (`showMenu`), chip button, dimmed hidden
   folders, Hidden chip, suggestion and sidebar row, info row, the tag-box note, move
   dialog lines. The closed-eye icon is our drawing (`lineIcon`), made to look unlike
   any set's eye-off and unlike our own "Ignore" icon. i18n keys in both files.
6. Guide EN and DE (the `guide` skill): what hiding does, tidiness not privacy, the folder
   rule, backups, the tag-box note; `python3 docs/guide/check.py`.
7. `CLAUDE.md` rule: hidden is the own tag plus `hidden_folders` in `library.db` only,
   never in a file; no guard change (no original is touched). `plan.md` row 20.
8. Browser test (keyboard: `h`, chip, folder click) in `core/tests/browser/`.
9. Real-hardware check: hide photos and a folder on the iPad (chip button), look at the
   timeline, a person, Places and the counts; copy a photo into a hidden folder and scan.
