# Phase 5: faces, own tags, reveal in Finder/Explorer

Phase 4 stores a box and a 128-number embedding per face. Phase 5 turns that
into people: clustering in Rust, naming, groups, corrections, and faces in
the sidebar and the info panel. Along the way the UI gets own tags (on top
of the folder tags) and "show in Finder / Explorer".

Before any clustering is built, the phase 4 run on the real drive tells us
whether recognition worked and which thresholds fit (5c-1).

## Split and pull requests

Separate PRs, merged in this order. Each one starts from (or merges) the
latest `main`, so conflicts stay small.

| Part | Scope | PRs | Schema |
|---|---|---|---|
| **5a** | Show in Finder / Explorer, copy path | 1 (worked on in a separate chat) | none |
| **5b** | Own tags: add/remove, many photos at once, search; user data backup | 1 (done) | `library.db` v3 |
| **5b-2** | Search by several tags at once (AND); people join in with 5c-3 | 1 (done) | none |
| **5c** | Faces | 3: **5c-1** check recognition, **5c-2** people/groups/clustering backend, **5c-3** UI | `library.db` v4 (5c-2) |

Conflict hot spots and how to avoid them:

- **`core/web/app.js`, info panel.** 5a, 5b and 5c-3 all add a section to
  the viewer's info panel. Each part adds its own function
  (`infoReveal`, `infoTags`, `infoFaces`) and one call where the panel is
  built, nothing else in that block. 5a merges first; the others merge
  `main` before their PR goes up.
- **`core/src/serve.rs`, route list.** New routes are appended at the end of
  their area, one line each; handlers live in their own module where they
  grow beyond a few lines (`tags.rs`, `people.rs`).
- **`core/src/db.rs`, schema version.** 5b takes v3, 5c-2 takes v4. If 5c-2
  were ready first, it takes v3 and 5b renumbers; never two PRs with the
  same version on `main`.
- **`core/web/app.js`, filter and search box.** 5b-2 turns the filter into
  a list of terms (chips); 5c-3 only adds the term kind `person` to it, no
  second filter.
- **`docs/plan.md`.** Each PR only ticks its own row in the status table
  and its own "Phase 5 details" bullets.

## Decisions

| Topic | Decision |
|---|---|
| Folder tags | Not removable. They are derived from where the file is; removing one in the UI would come back on the next scan or make the index disagree with the drive. To get rid of one, move the photo. Shown with a folder icon and no ✗. |
| Own tags | `file_tags.source = 'user'`, attached to the file id (survives moves and rescans). Never written into originals or as XMP sidecars next to them. |
| Person ↔ group | **A person is in at most one group** (`people.group_id`, NULL = "No group"). Groups can be created, renamed, reordered and deleted (their people become "No group"); a person's group can be changed at any time. |
| Where user data lives | Own tags, people, groups and face decisions (confirmed / rejected) go into `library.db`. They are the first data that can't be rebuilt from the drive. `recognition.db` stays a cache that can be thrown away: faces, embeddings, clusters and automatic suggestions. |
| Face decisions survive model changes | A decision is stored by content and box (`quick_hash` + box), not by face row id. After a model change the new face with the same key and an overlapping box (IoU ≥ 0.5) takes it over. |
| Backups of user data | After user changes the server copies `library.db` to `library.db.bak` (debounced, at most once a minute, and on shutdown), and writes `.shoebox/userdata.json` (own tags, people, groups, decisions; readable, easy to back up). |
| Search by several terms | **All terms must match (AND)**: "Spielplatz" and "Winter" finds photos that carry both tags; Aurelia and Grandpa finds photos where both appear. Terms are chips (tag, person, folder, free text), picked from suggestions; picking adds a chip instead of replacing the search. "Any of" (OR) is not planned. |
| Reveal | The server opens Finder / Explorer **on the computer it runs on**, so only for requests from that computer (localhost). Other devices (iPad) get "copy path" only. |

## 5a: show in Finder / Explorer

Done (PR "Phase 5-A"). Scope as agreed:

- `POST /api/files/{id}/reveal` → `{ok, app}`: path from the index only (never
  from the request; query and body are ignored), command started without a
  shell, output discarded:
  - macOS: `open -R -- <path>` (Finder, file selected)
  - Windows: `explorer.exe /select,<path>` (once there is a Windows build)
  - Linux: `xdg-open <folder>`
  `403` unless the request comes from this computer (loopback peer and a
  `localhost` or IP `Host`, the same test that skips the PIN), `404` for
  unknown ids and files that are not on the drive, `500` with the reason if
  the command cannot start. Needs the `X-Shoebox` header like every
  non-GET request.
- The OS is known at compile time (`cfg!(target_os)`; `core/src/reveal.rs`).
  `/api/info` has `reveal`: the label ("Show in Finder" / "Show in Explorer" /
  "Open folder") for requests from this computer, `null` for everyone else, so
  the button is hidden over the LAN.
- "Copy path" works everywhere, including the iPad: it copies the path within
  the library (as shown in the info panel), not the absolute path on the
  shoebox computer. Without https `navigator.clipboard` does not exist (an
  iPad opening `http://<ip>:7878/`), so the UI falls back to a selected text
  field and `execCommand('copy')`, and shows the path in a dialog if that
  fails too.
- Where: right-click menu on a grid cell and the info panel. The panel part is
  its own function, `infoReveal` in `app.js`, called with one line where the
  panel is built (see conflict hot spots).
- Touches no original. Finder may write a `.DS_Store` into the folder, which
  the scanner already skips.
- Test (`core/tests/serve.rs`): the command is replaced by a recorder
  (`serve::Options::reveal`); the endpoint passes exactly the indexed path,
  refuses non-local clients (a LAN device logged in with the PIN) and unknown
  or vanished files, needs the header, and the guard still holds. The command
  lines per OS are unit tests in `reveal.rs`.
- **Refinement, to be done in the 5c-2 PR:** the "Copy path" button in the
  info panel gets a `title` with the path it copies (the path within the
  library, as `copyPath` copies it), so hovering shows what will land on
  the clipboard. Where: `infoReveal` in `app.js` (`copy.title = info.path`).
  The right-click menu's "Copy path" has no element of its own to hover
  (`showMenu` items), so it stays as it is unless the menu gets titles too.

## 5b: own tags

Done. As built (on top of the scope below):

- `core/src/tags.rs` holds the logic; `serve.rs` only adds three routes:
  `POST /api/tags/add` and `/api/tags/remove` (`{ids, name}` →
  `{tag, files, folder}`; `folder` counts files that keep a folder tag of
  that name) and `POST /api/tags/selection` (`{ids}` → the own tags on a
  selection with counts, for "Remove tag…"). Changes go through the same
  path as moves (one at a time, not while a scan runs).
- Schema v3 adds `trash.user_tags` and `tags.fold` (NFC + lowercase, filled
  in by the migration). Own tags are looked up by `fold`, so an own tag
  "familie" on a photo is the folder tag "Familie" (`kind: both` in
  `GET /api/tags`, which also takes `own=1`). Folder tags keep the folder's
  exact spelling, so a case-only folder rename still renames its tag.
- Removing the last use of an own tag deletes the tag row; the trash keeps
  names, not ids, so a restore recreates it.
- `userdata.json` (next to `library.db`): `{shoebox, version: 1,
  written_at, own_tags: [{name, files: [{path, quick_hash, full_hash,
  in_trash?}]}]}`. Written with the `library.db.bak` copy right after the
  first change, then at most once a minute, and when the server stops.
- UI: `infoTags` in `app.js` (folder tags with 📁, own tags with ✕, "+ Tag"
  with autocomplete over all tags), "Add tag…" / "Remove tag…" in the
  selection bar, and a "Tags" section in the sidebar listing own tags.
- Tests: `core/tests/tags.rs`, plus a migration test in `db.rs`.

Backend:
- `POST /api/tags/add {ids, name}` and `POST /api/tags/remove {ids, name}`.
  Remove only ever deletes `source = 'user'` rows; a folder tag stays.
- Tag names are trimmed and compared NFC and case-insensitively, so
  "Europa-Park" and "europa-park" are one tag (the first spelling wins).
- `GET /api/files/{id}` returns tags with their source; `GET /api/tags`
  returns counts and whether a tag is a folder tag, an own tag or both.
- Search finds own tags like folder tags ("Aurelia Europa-Park" finds photos
  in the Aurelia folder tagged Europa-Park).
- **Trash keeps own tags:** today trashing deletes all `file_tags` rows and
  restoring indexes the file anew, so own tags would be lost. Schema v3 adds
  `trash.user_tags` (JSON) and restore puts them back.
- Scan already touches only `source = 'folder'`; a test makes sure it stays
  that way (rescan, move, case-only rename, move behind shoebox's back).
- Backup of user data as in the decisions above.

UI:
- Info panel: folder tags (folder icon, not removable), own tags (✗ to
  remove), "+ Tag" field with autocomplete over existing tags.
- **Tagging many photos at once:** select photos, "Add tag…", type a name.
  Also "Remove tag…" for own tags present on the selection.
- Optional: a "Tags" section in the sidebar listing own tags.

Tests (`core/tests/tags.rs`): add/remove, folder tags can't be removed, bulk,
names folded, tags survive move/rescan/trash-restore, guard around every
endpoint.

## 5b-2: search by several tags

Feedback after 5b: a photo tagged "Spielplatz" and "Winter" cannot be found
by asking for both. What happens today:

- Clicking a tag (sidebar, info panel) sets *the* tag filter and replaces
  the previous one; the URL holds one `tag=`.
- Picking a suggestion in the search box (a `<datalist>`) replaces the whole
  text with that tag's name.
- Typing several words does AND them, but each word only has to appear
  *somewhere* in the path or in any tag name: "Fall" also finds
  "Fallschirm" and a folder "Wasserfall", and a tag of two words
  ("Oma Inge") is split into two unrelated words.

Done. As built (on top of the plan below):

- `browse::Query.tags` (all must match); a tag id also matches the other
  spellings of its name (`tags.fold`), so a folder "Winter" and one
  "winter" count as one tag.
- `GET /api/timeline?tag=…&tag=…&folder=…&q=…` (the handlers read the query
  string as pairs, so keys can repeat); the reply has `tags: [{id, name}]`
  for the chips, so a bookmarked link shows names on a fresh page.
- `GET /api/tags?q=…&tag=…&folder=…` counts within that filter
  (`browse::tags_within`) and leaves out the tags of the filter itself;
  without a filter it is the old list.
- UI (`app.js`, "search box"): `state.filter.tags`, `withFilter()`, chips
  with ✕ and "+", "Clear all" at the end of the row once the search has two
  or more terms (the ✕ inside the search box only clears the typed text;
  "All photos" in the sidebar resets too), a suggestion list of its own (`#suggest`; ↑/↓, Enter,
  Escape, Backspace removes the last chip), folders suggested by name.
  Clicking a folder in the sidebar keeps the search text but clears tags,
  as before. The `<datalist>` stays only for the tag fields of 5b.
- Tests: `search_by_several_tags_at_once` in `core/tests/tags.rs`.

What changes:

- **The filter is a list of terms**, shown as chips above the grid, all of
  which must match (AND):
  - `tag`: an exact tag (folder or own; folded names, as in 5b);
  - `folder`: a folder and everything below it (as now, at most one);
  - `text`: free text, matched as today (path and tag names, each word);
  - `person` (added by 5c-3): photos where that person is confirmed.
- **Search box:** typing shows a suggestion list of its own (not a
  `<datalist>`), grouped Tags / Folders (/ People with 5c-3), each with its
  count. Picking one adds a chip and clears the text; Enter without a pick
  adds the text as a `text` chip. Backspace in the empty box removes the
  last chip; ✕ on a chip removes it. Works with touch on the iPad.
- **Clicking a tag** in the sidebar or the info panel still shows just that
  tag (a fresh filter); the chip bar has "+" to add another term with the
  same suggestion list. The sidebar marks every tag in the filter.
- **Counts that help narrowing:** suggestions shown while a filter is active
  count only photos that also match the current filter, and tags that would
  leave nothing are left out.
- **URL:** `#tag=12&tag=40&q=…` (repeated keys), so a combined search can be
  bookmarked; old single-tag links keep working.
- **API:** `GET /api/timeline` takes `tag` several times (AND) and later
  `person` the same way; `GET /api/tags?q=…` takes the current filter to
  count within it.
- **Selection still works on the result:** "Add tag…" on a combined search
  is how to tag "all Spielplatz photos from winter" at once.

Not in 5b-2: OR ("Spielplatz or Wald"), excluding a term ("not Winter"),
date ranges.

Tests (`core/tests/tags.rs`): two and three tags together, a folder tag and
an own tag together, tag + folder + text, a tag name that is a substring of
another ("Fall" / "Fallschirm") only finds the exact tag, counts within a
filter, old `?tag=` links, guard.

## 5c: faces

### 5c-1: did recognition work?

Built (PR "Phase 5c-1"), before any clustering; the run on the real drive is
in "Still to check on real hardware" below.
- `shoebox faces stats <root>`: photos looked at, errors grouped by message,
  faces found (and how many by the rotated pass), face width in the ≤1600 px
  copy (under 30, 30–40, 40–60, 60–120, 120+ px) and score (under 0.90,
  0.88–0.90, 0.90–0.92, 0.92–0.94, 0.94+ ; YuNet's scores lie between 0.85
  and ~0.96), the share under 30 px, and the
  last five runs with their time. Only reads: `library.db` and
  `recognition.db` are opened read-only (a phase 4 `recognition.db` at v1
  works as it is), nothing is created.
- **Face check** page in the UI (sidebar, shown once there are faces;
  `#view=faces`): all faces as crops, smallest/largest or lowest/highest
  score first, filter "too small for clustering" (< 30 px), "large enough",
  "found turned"; small faces have a dashed frame. Clicking a crop opens its
  photo in the viewer, closing it goes back to the page. "≈" on a crop
  lists its 24 nearest neighbours by embedding (cosine similarity), to see
  where "same person" ends; read-only, nothing about clusters is stored.
- API (handlers in `serve/faces_api.rs`, logic in `faces.rs`):
  `GET /api/faces?sort=size|score&desc=true&min_px=&max_px=&rotated=true&offset=&limit=`
  → `{total, faces: [{id, file, kind, version, px, score, roll, small}], min_cluster_px}`;
  `GET /api/faces/stats` (what `faces stats` prints, as JSON);
  `GET /api/faces/{id}/crop` (JPEG, ≤ 160 px);
  `GET /api/faces/{id}/similar?limit=`.
- The width of a face is measured across the face: for one the rotated
  pass found (lying sideways in the picture) that is its box's height.
  `faces::MIN_CLUSTER_PX` (30) and the `small` flag are what 5c-2 uses.
- From that: the minimum face size for clustering, and the similarity
  threshold for "same person" (SFace's usual cosine threshold is about
  0.36; calibrated on our photos).
- **Decided on the real drive**: faces **under 30 px** wide (in the
  ≤1600 px copy; 296 of 9598, 3.1%) are listed but neither clustered nor
  suggested. First set at 40 px from the phase 4 run, but that left out
  8.6% (828), and on the face check page all 532 faces of 30–40 px were
  real, recognisable people.
  No score threshold beyond the detector's own 0.9: every stored face
  scores ≥ 0.85, so the score separates nothing.
- **Sideways faces: a second, optional pass.** YuNet misses faces rolled
  more than ~30–45° (people lying down). `shoebox recognize --rotated`
  looks at the photos again, turned 90° and 270°, after the normal pass
  and whenever there is time:
  - its own task in `recog.looked` (`faces-rot`), so it has its own
    progress, is resumable and can be run any time later;
  - shoebox turns the copy it sends, so the protocol stays as it is; boxes
    and landmarks are turned back before they are stored;
  - a face is only added where the upright pass found none (box overlap,
    IoU < 0.3), so nothing is counted twice;
  - costs about two upright passes (~40 min for the family folder on the
    old Intel MacBook).
  As built: `recognition.db` v2 adds `faces.roll` (0, 90, 270); shoebox
  turns the ≤1600 px copy with `image`'s `rotate90`/`rotate270` and turns
  boxes and landmarks back before storing them upright. Rotated faces are
  checked in score order against the upright ones and against each other.
  A new upright result for a photo (another model, `--retry-failed`) drops
  its rotated result, which the next `--rotated` run redoes. `--rotated`
  runs the upright pass first if anything is left; `--limit` counts per
  pass. Ctrl-C ends it like the upright pass (job `faces-rot` marked
  interrupted). Checked with the real worker on a photo turned 90° and
  270°: nothing upright, one face each in the rotated pass, at the box of
  the upright photo turned; nothing added to the upright photo.
- Face crops: made from the original under the guard, cached in
  `thumbs.db` keyed by face. As built: `thumbs.db` v3 adds `thumbs.faces`
  (key + box → JPEG or error); a square around the face with 25% room,
  turned upright for rotated faces, ≤ 160 px. All crops of a photo come
  from one decode at 1600 px; a photo that changed since the last scan
  gets none and nothing is stored. The scan's thumbnail pass prunes crops
  of content that is gone.
- Tests (`core/tests/recognize.rs`): `--rotated` under the guard (a face
  found only turned, put back at the right box, nothing counted twice,
  resumable, redone after a new upright result), Ctrl-C during the rotated
  pass, `faces stats` (counts, buckets, read-only, v1 database), the face
  check API under the guard (list, sort, filter, crops cached and turned
  upright, neighbours, changed files). The fake worker has two new cues
  for this (a cyan or yellow edge, see its header).

### 5c-2: people, groups, clustering (backend)

Also in this PR: the 5a refinement (a `title` with the path on "Copy
path", see 5a above).

Schema v4 in `library.db`:

```sql
CREATE TABLE groups (
    id       INTEGER PRIMARY KEY,
    name     TEXT NOT NULL UNIQUE,
    position INTEGER NOT NULL
);
CREATE TABLE people (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    group_id   INTEGER REFERENCES groups(id) ON DELETE SET NULL,  -- one group at most
    cover_key  TEXT,                -- quick_hash + box of the face shown for this person
    cover_box  TEXT,
    hidden     INTEGER NOT NULL DEFAULT 0
);
-- What the user decided about a face, for detected and hand-drawn faces
-- alike. Keyed by content and box, so it survives moves, rescans and model
-- changes.
CREATE TABLE face_decisions (
    id        INTEGER PRIMARY KEY,
    key       TEXT NOT NULL,       -- files.quick_hash
    x REAL NOT NULL, y REAL NOT NULL, w REAL NOT NULL, h REAL NOT NULL,
    person_id INTEGER REFERENCES people(id) ON DELETE CASCADE,  -- NULL with 'ignored' and 'not_face'
    decision  TEXT NOT NULL,       -- confirmed, rejected (not this person), ignored (stranger), not_face (false find)
    manual    INTEGER NOT NULL DEFAULT 0,  -- 1: the box was drawn by hand, not detected
    at        INTEGER NOT NULL
);
CREATE INDEX face_decisions_key ON face_decisions(key);
```

Detected and hand-drawn faces share the table on purpose: both are "this
box on this picture is (not) that person", and every query (photos of a
person, references for suggestions, the info panel) reads one table.

- **Detected face** (`manual = 0`): the decision belongs to the face in
  `recog.faces` with the same key whose box overlaps it best (IoU ≥ 0.5).
  After a model change the new face takes it over the same way; a decision
  that no face matches any more is kept and shown as "face no longer
  found" rather than dropped.
- **Hand-drawn face** (`manual = 1`): the row *is* the face; there is no
  `recog.faces` row behind it. Its embedding is cached in `recognition.db`
  (computed with the `embed` task, see 5c-3) and recomputed after a model
  change. It is always `confirmed` with a person: the UI asks for the name
  while drawing. Deleting it deletes the row. If a later pass (e.g.
  `--rotated`) detects a face overlapping it, the two are shown as one
  face, the hand-drawn box winning.
- A rejection can only be made for a detected face (one never draws a box
  to say who it is not).
- **Not a face** (`not_face`, asked for after looking at the 5c-1 face
  check page): a detection that is no face at all (poster, statue,
  pattern in the background). Unlike `ignored` (a real person nobody
  needs to name) it means the box itself is wrong. Such a face:
  - is hidden everywhere (viewer boxes, info panel, people, clusters,
    "Unnamed") and never takes part in clustering or suggestions;
  - is matched like other decisions (key + box, IoU ≥ 0.5), so it stays
    hidden after a new `recognize` run or a model change;
  - is counted in `shoebox faces stats` and on the face check page (false
    finds per size and score), which tells whether a stricter detector
    threshold would pay off;
  - can be undone (the row is deleted). Only for detected faces: a
    hand-drawn box is deleted instead.

In `recognition.db` (a cache: losing it loses no decision): the cluster
and the suggested person per face.

**Clusters are a pure cache** (decided): after every recognize run they
are recomputed from all faces without a decision, and nothing about them is
kept.

- Simple and always consistent: new photos are mixed in properly and a bad
  grouping can fix itself on the next run.
- The price: clusters have no stable number. Cards under "Unnamed" are
  sorted by size (most faces first) and labelled by a sample face, not
  "Unnamed #12", and a card may look different after new photos.
- Everything the user does with unnamed faces is therefore a decision in
  `face_decisions`, never cluster state:
  - naming a cluster (or some of its faces) → `confirmed` rows;
  - "Ignore" (strangers) → `ignored` rows;
  - "Not a face" (false finds) → `not_face` rows;
  - "split" means selecting faces in a card and naming or ignoring them;
    there is no "split but leave unnamed".
- Throwing `recognition.db` away loses nothing the user did.

Matching and clustering:
- Only faces above the 5c-1 size and score thresholds take part; smaller
  ones are listed but never suggested. Faces marked `not_face` never take
  part.
- A new face is suggested for a person when it is close enough to that
  person's confirmed faces, and never for a person it was rejected for.
- Unassigned faces are grouped into clusters after each run, from scratch
  (see above) but without an all-pairs pass: every face looks up its
  nearest neighbours in an approximate index (`instant-distance` or
  `hnsw_rs`, or plain SIMD dot products if that is fast enough for ~150k
  faces), and neighbours close enough end up in one cluster.
- Runs after `shoebox recognize` and in the background in `serve`;
  resumable, progress in `/api/info`.

API: people (list, create, rename, merge, hide, change group, set cover),
groups (list, create, rename, reorder, delete), clusters (list, name,
ignore, split), faces (confirm, reject, assign, ignore, not a face, undo), photos of a person
for the timeline.

Tests (`core/tests/people.rs`): with the fake worker's embeddings: suggest,
confirm, reject, not a face (hidden everywhere, survives a model change,
counted in the stats), merge, split, groups (one per person, delete → no group),
decisions survive a model change and a move, hand-drawn faces (`manual`)
appear for their person and never as a duplicate of a detected face, guard.

### 5c-3: UI

Left sidebar, a new section between "All photos" and "Folders":

```
All photos
Faces
  ▸ Family (5)
  ▸ Best friends (5)
  ▸ University (5)
  ▸ No group
  Unnamed (23 clusters)
Folders
  …
```

- **Faces overview:** round face crops grouped by group; click a person →
  timeline with their photos.
- **Unnamed:** one card per cluster with a few sample faces and a name
  field. The autocomplete lists people **by group**, which is what makes
  assigning quick. "Ignore" for strangers, "Not a face" for false finds
  (also for single faces selected in a card).
- **Corrections:** merge two people, take faces out of a person ("not this
  person"), split a cluster, name several selected faces at once.
- **Face check page (5c-1):** "Not a face" on each crop and on several
  selected crops, so false finds can be cleared while going through the
  smallest faces and lowest scores; a filter shows the ones marked, to
  undo a mistake.
- **Groups:** create, rename, reorder, delete; change a person's group by
  drag and drop or "Move to group…". No groups are made up front: the
  user creates them in the UI (the first ones on the real drive will be
  "Familie" and "Freunde").
- **Info panel (viewer):** a "People" section with face crops and names.
  Unnamed faces show "+ Name"; suggested ones ✓ (confirm) and ✗ (reject).
  Every detected face has "Not a face" in its menu.
  Hovering a face highlights its box in the photo (boxes exist since
  phase 4).
- **Add a missed face by hand** (moved up from "later": it is what makes
  missed faces acceptable): "Add face" in the viewer, draw a box, name it.
  - The box is user data: a `face_decisions` row with `manual = 1` (key +
    box + person), so it survives model changes.
  - Its embedding comes from the worker: protocol v2 adds a task `embed`
    (`{"tasks": ["embed"], "image": …, "boxes": [[x, y, w, h]]}`). The
    worker looks for landmarks inside the box with a low threshold and
    aligns as usual; without landmarks it embeds the plain crop (less
    reliable, so such faces are not used as references for suggestions).
  - Core and worker are updated together (the hello's protocol must
    match, as now).
- Search finds people by name, like tags: a person is a term of the 5b-2
  filter, so "Aurelia" + "Grandpa" lists the photos where both are
  confirmed, and people combine with tags ("Aurelia" + "Spielplatz").
  `GET /api/timeline` takes `person` several times (AND).
- Recognition and clustering progress in the status line.

## Later (not in phase 5)

- Undo for assignments.

## Still to check on real hardware

- [ ] 5a: "Show in Finder" on the old Intel MacBook opens the right folder
      with the file selected; the iPad only offers "copy path".
- [ ] 5b: tag a few hundred photos at once on the exFAT drive; tags survive
      a move, a rescan, trash and restore; `userdata.json` and
      `library.db.bak` appear in `.shoebox/`.
- [ ] 5b-2: on the iPad, combine two own tags and a folder from the search
      box; the chips, the counts and a bookmarked link.
- [ ] 5c-1: on the old Intel MacBook against the exFAT drive:
  - [x] `shoebox faces stats` on the family folder (7196 photos, all
    looked at; `verify` before it: 7994 files OK): 9598 faces = the phase 4
    9564 plus 34 from a partial `--rotated` run (258 photos). Widths: 296
    under 30 px, 532 of 30–40, 1297 of 40–60, 2971 of 60–120, 4502 larger;
    **828 (8.6%) under 40 px**. Scores 0.85–0.96 (2422 under 0.90, 4318 of
    0.90–0.93, 2855 of 0.93–0.96, 3 higher), so the first score buckets
    were too wide at the top; changed to 0.88/0.90/0.92/0.94. Still 48
    failures (43 "No 'ftyp' box", 5 "Illegal start bytes"): the JPEGs
    misnamed `.HEIC` etc. from phase 4, waiting for `--retry-failed`.
  - [x] `shoebox recognize --retry-failed`: all 48 looked at in 9 s, 65
    faces, 0 failed. Afterwards `faces stats`: 10043 faces, 0 failed;
    300 (3.0%) under 30 px. (These 48 have not been looked at turned yet:
    the next `--rotated` run takes them.)
  - [x] Is 40 px right? On the face check page all 532 faces of 30–40 px
    were real, recognisable people: the threshold is now 30 px (keeps 97%
    of faces instead of 91%).
  - [x] "Same person" threshold from "≈" (24 neighbours each, faces under
    30 px ignored), five people:

    | Reference | Right matches | Wrong ones |
    |---|---|---|
    | boy, teens (386 px) | 0.87 down to 0.37 | from 0.38 down |
    | older woman (447 px) | all 24, down to 0.65 | none in the list |
    | girl, teens (456 px) | all 24, down to 0.60 | none in the list |
    | girl, teens (481 px, found turned) | all 24, down to 0.65 | none in the list |
    | toddler (441 px) | down to 0.58, and one at 0.47 (69 px, the same girl a few years older) | 0.54 (other child), then from 0.49 down |

    Adults and teenagers are clear-cut: everything at 0.60 and above was
    right. Small children look alike and change fast: a wrong child at
    0.54, while the same girl a few years older scored only 0.47 against
    her toddler face. Highest wrong match seen: 0.54; lowest right one:
    0.37. So for
    5c-2: compare a face with all confirmed faces of a person (best
    match: confirmed faces from several ages bridge the gap a single
    reference cannot), **suggest from ~0.55**, offer only as **"maybe" between ~0.35
    and 0.55** (settled by ✓/✗), nothing below. A face found by the rotated
    pass matched its person's upright faces as well as any (0.65–0.75), so
    rotated faces can take part like the others.
  - [x] The whole rotated pass added 414 faces (4% of all; the first 258
    photos, 34 faces, were not typical), and 5 more for the 48 photos
    retried later (14 s). Looked through "Found turned": almost all are
    real faces, and "≈" on them finds the same people's upright faces.
    One photo checked in the viewer (IMG_6482.HEIC, three people lying
    down): one face found upright, two by the rotated pass, all correct,
    none twice. False finds seen: a leg (4a5a1198-….JPG) and a man's hands
    (IMG_9959.JPG), both in the lower corner of the photo. "Not a face"
    (5c-2/5c-3) is what clears them; if the face check page shows that
    false finds pile up among rotated faces with low scores, the rotated
    pass can get a stricter score cut-off than the upright one.
  - `shoebox serve`, "Face check": go through the smallest faces and the
    lowest scores; how many false faces (posters, statues, background),
    and are there any above 30 px? Open a few in the viewer. Try "≈" on
    faces of people you know: up to which similarity are the neighbours
    the same person (for 5c-2's threshold)? Crops of HEIC and EXIF-rotated
    photos upright?
  - [x] Time `shoebox recognize --rotated` (family folder, 7148 photos):
    interrupted after 281 photos (3 min), then continued with the other
    6866 in 2072 s (35 min, 3.3 photos/s; the estimate was ~40 min);
    0 failed, 414 faces added in all.
  - `shoebox verify` afterwards.
- [ ] 5c-2: clustering time over all faces on the old Intel MacBook.
- [ ] 5c-3: naming and correcting from the iPad.
