# Phase 19: Hidden photos and folders

**Planned, nothing built.** Photos you want out of the way (nude pictures,
documents, surprises) are marked *hidden*. They vanish from every normal view
and come back only when you ask for them by name: the Hidden chip, the word
*hidden*, or a click on a hidden folder in the sidebar. Like Favorites (see
"Favorites" in [plan.md](plan.md)), but the tag does the opposite: it removes.

This is **tidiness, not privacy.** The files stay where they are, in plain
view in the file manager, and their thumbnails stay in `thumbs.db`. There is
no PIN for it. The guide must say so.

## 1. What the user sees

| Action | Result |
|---|---|
| Viewer: eye-slash button in the top bar (key `h`) | Hides / shows the photo again |
| Selection bar: "Hide" / "Show again" | Same for many photos |
| Sidebar, right-click a folder: "Hide folder" / "Show folder again" | Flags the folder; everything in it and below is hidden, **including photos copied in later** |
| Hidden folder in the sidebar | Stays in the tree (dimmed, eye-slash) so you can click it; its own count is shown, its parents' counts do not include it |
| Click a hidden folder (or a folder below one) | Its photos are shown |
| Hidden chip (eye-slash button next to Type and the heart, search suggestion "Hidden", typed *hidden* / *versteckt* / *verborgen*) | Only hidden photos, AND with the other chips (so *Hidden + Anna* finds hidden photos of Anna) |
| Info panel | Row "Hidden", with "via folder Urlaub/Privat" when the folder does it. A photo hidden by its folder is shown again by moving it out of that folder, or by un-hiding the folder |
| Timeline, folders, People, Places, dates, Type, tag suggestions, search counts, Overview, All drives | Hidden photos are **not there**, whatever filter is set. A person's photo that is hidden is not in the person's photos either |

Rule in one line: *a hidden photo is shown only if the Hidden chip is on, or if
the selected folder is itself hidden (the folder's subtree is then shown).* A
photo tagged hidden in a normal folder is shown by the chip only, not by
clicking its folder.

## 2. Data

- **Hidden photo = own tag `hidden`** (`tags::HIDDEN`, source `user`), exactly like
  `tags::FAVORITE`: reserved name, `tags::check_name` maps `hidden`, `versteckt`,
  `verborgen` (and the aliases `is_hidden_word` accepts) to it, so there is no
  second tag; the UI never lists it as a tag (info chips, suggestions); the
  sidebar's Tags list has one "Hidden" row. A folder called "hidden" does not hide
  its photos (only the own tag counts, as with favorites). It is in `userdata.json`
  and carried over by matching content (phase 15) like any own tag, and survives
  moves, rescans and the trash.
- **Hidden folder = a row in a new table** `hidden_folders(path_nfc TEXT PRIMARY KEY,
  added_at INTEGER)` in `library.db` (schema v16), keyed by NFC path, not by folder
  id, so it survives rescans and a folder that empties and returns. A folder is
  hidden if it or an ancestor has a row. A new table must be listed in
  `transfer::NOT_TRANSFERRED_TABLES` (a path is not content; the own tag part
  does travel). Nothing is written into a file or its folder, and it works in a
  read-only library (phase 16), because the flag lives in `libraries/<name>/library.db`.
- Moving a photo into a hidden folder hides it; out of it shows it again (the
  move dialog says so when the target or source is hidden).

## 3. Where the rule lives

Everything list-like already goes through `browse::Snapshot::query`
(`core/src/browse.rs`, used by the timeline, counts, tag suggestions, year
strip and search in `serve.rs`). So:

1. `Snapshot::load` also loads the set of hidden folder paths and the ids of
   files with the `hidden` tag, and marks each `Item` `hidden: bool` (own tag,
   or folder at or below a hidden row).
2. `browse::Query` gets `hidden: bool` (the chip; `hidden=1` in the URL hash
   and API, like `fav=1`) and `Snapshot::query` first drops hidden items unless
   `q.hidden` or the item lies in the subtree of `q.folder` and `q.folder` is hidden.
   With `q.hidden` it keeps only hidden items.
3. The typed word goes through `is_hidden_word` in the word loop (next to
   `is_favorite_word`) and turns into `hidden: true`, so typing *hidden* does not
   also text-search the file names.
4. Places that **bypass** `Snapshot::query` need the same rule; each is a test
   case: `browse::all_tags` (the unfiltered tag list, SQL), people and pet
   counts and photo lists (`people.rs`), clusters / face crops in the
   Calibration screens and the Unnamed lists (a hidden photo's face must not show
   up as a crop), Places and clusters of pins (`geo.rs`), the duplicates screen,
   `multi.rs` (All drives, backup comparison, `forward`), export by tag,
   Overview and Stats, slideshow / next-previous in the viewer.
   A grep for direct `FROM files` queries in `serve.rs`, `people.rs`, `geo.rs`,
   `multi.rs` makes the list complete.

## 4. Open questions (decide before building)

1. **Duplicates and "Delete the new copies".** A hidden copy next to a visible
   one: show it on the duplicates screen? Proposal: the screen leaves hidden
   photos out unless the Hidden chip is on, and `arrivals::cleanup` / `remove_copies`
   still work on full hashes (the user asked for a clean-up) but never trash the
   *only visible* copy because of a hidden one; a hidden new copy of a visible
   photo is removed like any new copy. Needs a test.
2. **Export.** Collection tag export (phase 13): do hidden photos with the
   collection tag go along? Proposal: yes when picked explicitly in a selection,
   no when only by tag; the dialog counts "n hidden photos left out".
3. **Backups.** The backup keeps its own `library.db`; the hidden flags are not
   copied to it (a backup is read-only for shoebox). On the backup drive the
   photos are not hidden unless the user hides them there too. Say so in the guide.
4. **Names for German.** *versteckt* (chip text "Versteckt") or *ausgeblendet*;
   the word list accepts both, the chip uses one.
5. **A setting?** Favorites have none, trash is opt-in. Proposal: none, the
   button is always there (hidden is harmless, nothing is deleted).

## 5. Steps

1. Core: `tags::HIDDEN`, `check_name` mapping, `is_hidden_word`, `set_hidden`,
   `hidden_ids`; schema v16 `hidden_folders`, `tags::set_folder_hidden`; list the
   table in `NOT_TRANSFERRED_TABLES`. Tests in `core/tests/tags.rs`
   (`hidden_is_the_own_tag_hidden_and_a_folder_flag_is_inherited`).
2. `browse.rs`: `Item.hidden`, `Query.hidden`, the visibility rule, word
   loop. Tests: not in timeline, not under a person / date / type / tag, shown by the chip,
   shown when a hidden folder is selected, counts and `tags_within` agree.
3. `serve.rs`: `hidden_of(pairs)`, `POST /api/hidden {ids, on}`, `POST /api/hidden/folder
   {path, on}`, `hidden` in the timeline response (like `favs`), the bypassing
   queries from section 3.4.
4. `people.rs`, `geo.rs`, `multi.rs`, duplicates, export: the same rule, one test each.
5. UI (`core/web/app.js`, `app.css`, `index.html`): viewer button and key `h`,
   selection bar button, folder context menu, dimmed hidden folders, Hidden chip
   and button, suggestion, info panel row. Icon: our own line drawing
   (`lineIcon`), visibly unlike the usual eye-slash: a closed lid with lashes,
   for instance. i18n keys in `en.json` and `de.json`.
6. Guide EN and DE (follow the `guide` skill: what hidden does, that it is not
   privacy, the folder rule, the backup note); `python3 docs/guide/check.py`.
7. `CLAUDE.md` rules: hidden is own tag plus `hidden_folders` in `library.db`
   only, never in a file; no guard change (no original is touched). `plan.md`
   status table row 19.
8. Browser test (keyboard: `h`, chip, folder click) in `core/tests/browser/`.
9. Real-hardware check: hide a few photos and a folder on the iPad, look at
   the timeline, a person, Places and the counts; copy a photo into a hidden folder
   and scan.
