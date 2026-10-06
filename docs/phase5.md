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
| **5d** | Duplicates UI and tag carry-over (see [plan.md](plan.md)) | 1, separate commits per step | `library.db` v5 (capture-date override) |

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
- **Refinement, done in the 5c-2 PR:** the "Copy path" button in the
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
  of content that is gone. (Since 5e only faces without a decision and
  people's pictures stay stored.)
- Tests (`core/tests/recognize.rs`): `--rotated` under the guard (a face
  found only turned, put back at the right box, nothing counted twice,
  resumable, redone after a new upright result), Ctrl-C during the rotated
  pass, `faces stats` (counts, buckets, read-only, v1 database), the face
  check API under the guard (list, sort, filter, crops cached and turned
  upright, neighbours, changed files). The fake worker has two new cues
  for this (a cyan or yellow edge, see its header).

### 5c-2: people, groups, clustering (backend)

Built (PR "Phase 5c-2"); the run on the real drive is in "Still to check on
real hardware" below. Also in this PR: the 5a refinement (a `title` with
the path on "Copy path", see 5a above) and "Not a face" on the face check
page (from 5c-3, see below). As built, on top of the plan that follows:

- Code: `core/src/people.rs` (decisions and how they are matched to faces,
  people, groups, lists, the user data), `core/src/clusters.rs` (the
  clustering job), `core/src/ann.rs` (nearest neighbours),
  `core/src/serve/people_api.rs` (handlers). `library.db` v4 is the schema
  below plus an index on `face_decisions(person_id)`; `recognition.db` v3
  adds the cache (`neighbours`, `clusters`, see below).
- **Matching**: every decision belongs to the face of its content whose
  box overlaps it best with IoU ≥ 0.5 (`people::MATCH_IOU`). Of several
  decisions on one face the latest `confirmed`/`ignored`/`not_face` counts;
  `rejected` rows add up (one per person). Everything shown is matched
  live, so a decision shows at once, before the cache is recomputed.
- **Thresholds** (named constants, from the calibration below):
  `people::SUGGEST_SIM` 0.55, `people::MAYBE_SIM` 0.35,
  `clusters::CLUSTER_SIM` 0.60, `faces::MIN_CLUSTER_PX` 30 px. A face is
  compared with *all* confirmed faces of every person (the 32 most similar
  through the index) and gets the best person it was not rejected for.
  Confirmed faces under 30 px are no references either.
- **Clustering** (`clusters::run`): faces of present photos, of the current
  model (the latest upright result's), ≥ 30 px, rotated ones included.
  Each looks up its 24 nearest neighbours with similarity ≥ 0.60 once; the
  lists are kept in `recog.neighbours`, so a stopped run resumes and after
  a `recognize` run only new faces need lists (a deleted face's list goes
  with it: SQLite can give a new face the old id). Then, from scratch every
  time: faces without a decision that are neighbours are joined into
  clusters (similarities checked again), and the suggestions are computed.
  `recog.clusters` gets one row per face without a decision (cluster
  number, largest first; suggested person and similarity).
- **Nearest neighbours without all pairs** (`ann.rs`): an inverted-file
  index; k-means centres (√n of them, trained on at most 40,000 faces)
  split the embeddings into cells, and a search compares with the centres
  and the faces in the closest eighth of the cells (at least 12). Up to
  2,000 faces it compares with all. Dot products in eight lanes (SIMD
  without `-ffast-math`), on all cores. On made-up faces of 128 numbers
  like SFace's it finds 99% of the neighbours ≥ 0.55.
- **When**: at the end of every `shoebox recognize` (which prints
  "Clusters: … (N s)"; Ctrl-C stops it like the passes), and in `serve` on
  its own connection and thread: at start, after every change to people or
  decisions (300 ms later, changes in between together), and when
  `/api/info` sees that `recognition.db` has other faces than were last
  clustered (a run that stopped before clustering). Not while `recognize`
  runs (it clusters itself). Progress in `/api/info` → `clusters: {running,
  stale, done, total, state, finished_at, clusters, unnamed, suggested,
  people}` (`done`/`total`: neighbour lists of the latest job, in
  `recog.jobs` with kind `clusters`).
- **Time**: 10,000 made-up faces (1,000 people) in 0.34 s on 4 cores of a
  2.1 GHz Xeon, 0.67 s on one; again after naming 100 people (neighbour
  lists kept): 0.07 s (`cargo test --release --test people --
  --ignored`). Real drive: see below.
- **API** (every change needs `X-Shoebox` and goes through the same path as
  moves and tags: one at a time, not while a scan runs, backed up):
  - People: `GET /api/people[?hidden=1]` → `[{id, name, group_id, hidden,
    faces, photos, suggested, maybe, cover}]` (`cover`: a face id for
    `/api/faces/{id}/crop`, the chosen one or the largest), sorted by group
    position then name, no group last; `POST /api/people {name,
    group_id?}`; `GET /api/people/{id}`; `GET /api/people/{id}/faces?
    state=confirmed|suggested|maybe|rejected&offset&limit` → `{total,
    faces}`; `POST /api/people/{id}/rename {name}`, `/merge {into}`,
    `/hide {hidden}`, `/group {group_id|null}`, `/cover {face}`. Names are
    compared like tags (NFC, ignoring case).
  - Groups: `GET /api/groups` → `[{id, name, position, people}]`; `POST
    /api/groups {name}` (at the end); `POST /api/groups/reorder {ids}`;
    `POST /api/groups/{id}/rename {name}`; `POST /api/groups/{id}/delete`
    (its people get no group).
  - Clusters: `GET /api/clusters?offset&limit&samples` → `{generation,
    total, unnamed, clusters: [{id, size, faces, suggestion: {person,
    faces}}]}`; `GET /api/clusters/{id}/faces`; `POST
    /api/clusters/{id}/name {name|person_id, faces?, generation?}`,
    `/ignore`, `/not-face`. `faces` (some of the cluster's) is the split;
    with `generation` a cluster that changed since is refused (409).
    (5c-3 made `generation` per cluster, see there.)
  - Faces: `POST /api/faces/confirm {faces}` (the suggestion, also a
    "maybe"; decided faces are passed over), `/reject {faces, person_id?}`
    (the suggested person if none), `/assign {faces, person_id|name}` (a
    new name makes a person), `/ignore`, `/not-face`, `/undo {faces,
    manual?}` (forgets every decision about the faces; deletes hand-drawn
    ones). `POST /api/faces/manual {file, box, person_id|name}` → `{manual}`
    adds a hand-drawn face (the drawing UI is 5c-3). All return `{faces,
    person}`.
  - `GET /api/files/{id}`: `faces` now carry `id`, `state` (`confirmed`,
    `ignored`, `suggested`, `maybe` or `null`), `person {id, name}`,
    `similarity`, `small`, `rejected`, and `manual` for hand-drawn ones
    (their box wins over a detected face they overlap); "not a face" is left
    out. `faces_lost` lists confirmed faces that are no longer found.
  - `GET /api/timeline?person=<id>`: photos with a confirmed face of the
    person (also ones no longer found).
  - Face check page: `GET /api/faces` leaves out "not a face", `?not_face=true`
    lists only those; `/api/faces/stats` and `shoebox faces stats` count
    them (`not_faces`, and `not_face` per width and score bucket);
    neighbours (`similar`) leave them out.
- **User data**: `userdata.json` version 2 adds `groups [{name,
  position}]`, `people [{name, group, hidden?, cover}]` and
  `face_decisions [{person, decision, manual?, key, box, at, files: [{path,
  full_hash, in_trash?}]}]`; `library.db.bak` as for tags.
- **Face check page**: "✕" (Not a face) on every crop, "Select" to mark
  several at once, and a filter "Marked “not a face”" whose crops have "↺"
  to undo (and "It is a face" for several).
- **Fake worker**: a white top quarter and grey bottom quarter make a face
  of the person of the middle's colour with a similarity of
  round(40·grey/255)/40 to their plain face (see its header).
- Tests: `core/tests/people.rs` (suggest, maybe, confirm, reject, assign,
  undo, ignore, not a face everywhere and across a model change and in the
  stats, groups, merge, split and stale clusters, a move, a model change, a
  lost `recognition.db`, a shifted box, "no longer found", hand-drawn faces,
  the user data backup, resuming, the guard; the timing over 10,000 faces
  on request), unit tests in `people.rs`, `ann.rs`, `db.rs`.

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

Built (PR "Phase 5c-3"); the run on the real drive is in "Still to check on
real hardware" below. Decided before building: a generation per cluster,
`serve` embeds drawn faces itself, free text finds names, "Use as …’s
picture" (cover) but no hiding of people. As built, on top of the plan that
follows:

- **Sidebar** "Faces" (between the top links and "Tags"; a "Folders"
  heading now marks the folder tree): groups with their people (▸ opens a
  group; dropping a person on a group moves them), "No group", "Unnamed (N
  clusters)". "Faces" itself opens the overview. Shown once there are faces
  or people.
- **Overview** (`#view=people`): round pictures by group, photos and "N to
  check" per person, ⋯ with show photos, review faces, rename, move to
  group, merge; "New group…", "Groups…" (rename, ↑/↓, delete), drag and
  drop onto a group. A person's photos are the timeline with a person
  chip (`#person=12`) and a head above it ("Check N faces", ⋯).
- **A person's faces** (`#view=person&id=12&tab=…`): tabs Confirmed,
  Suggested, Maybe, Not them. ✓/✗ on suggested and maybe faces, ↺ on "Not
  them" (undoes only that rejection, `POST /api/faces/unreject {faces,
  person_id}`), ⋯ on confirmed ones (not this person, name someone else,
  use as picture, not a face; for drawn faces: name again, delete).
  "Select" for several at once; "Confirm all shown" only on Suggested.
- **Unnamed** (`#view=unnamed`): a card per cluster (8 sample faces, "+N"
  shows all), "Looks like X (n of m) ✓ X" when a person is suggested, a name
  field whose list shows people by group (and "+ New person"), Ignore,
  Not a face, and "Select" to act on some faces only (the split). Cards
  change in place; the page is not reloaded after each action.
- **Generation per cluster** (changed backend): naming one card makes the
  server recompute and renumber all clusters, so a single generation for
  the list made every other card stale a second later. Each cluster now has
  its own `generation` (a hash of its faces, below 2^53 for JavaScript);
  an action finds the cluster with those faces whatever its number now and
  is refused (409) only if its faces changed. Cluster actions reply with
  what is left of the cluster (`cluster: {id, generation, size}` or
  `null`), so a split card carries on. The list's top-level `generation` is
  gone.
- **Info panel**: `infoFaces` (one call where the panel is built; it took
  the place of phase 4's "Faces: N, Show" row): crop and name per face, "Maybe
  X? ✓ ✗" for suggestions, "+ Name" (or "Other…") with the same name list,
  ⋯ with not this person, name someone else, use as picture, ignore,
  forget what was said, "may be X after all" (undo a rejection), not a
  face. Pointing at a face (or tapping its crop, for touch) highlights its
  box; "Show boxes" draws all with names; faces no longer found are listed.
- **Add face**: "+ Add face" in the panel; the panel steps aside, a box is
  dragged over the photo (pointer events: mouse, pen, finger; the viewer's
  swipe is off meanwhile), then named. Stored with `POST /api/faces/manual`.
  Crops of drawn faces: `GET /api/faces/manual/{id}/crop` (made with the
  photo's other crops, under the guard). A person whose faces are all drawn
  shows a drawn one (`cover_manual` in `/api/people`).
- **Protocol 2, `embed`** ([protocol.md](protocol.md)): the worker looks for
  landmarks around the drawn box (YuNet, threshold 0.3) and aligns; else it
  embeds the plain crop. `recognition.db` v4 caches them in `recog.drawn`
  (key + box, model, aligned). `shoebox recognize` embeds drawn faces at the
  end of every run (again after a model change); `shoebox serve` starts the
  worker in the background right after a face is drawn
  (`serve --recognizer`, else `$SHOEBOX_RECOGNIZER` or the installed one)
  and stops it when done. Aligned ones are references for suggestions (like
  confirmed detected faces, ≥ 30 px); plain crops are not. On test photos a
  drawn box around a detected face embedded at 0.92–0.96 to it; even an
  upside-down face that YuNet missed got its landmarks.
- **Search**: the suggestions list "People" first (counted within the
  search, `GET /api/people/search?q&tag&folder&person`), chips "👤 Name";
  `person` repeats in the URL and in `/api/timeline` (all must be on the
  photo; the reply has `people: [{id, name}]` for the chips). A typed word
  also matches the names of people confirmed on a photo, as it matches tag
  names.
- **Status line**: "grouping faces N%" / "grouping faces…" while clusters
  are recomputed, "learning drawn faces…" while drawn faces are embedded
  (`/api/info` → `clusters.embedding`); the page asks more often meanwhile.
- Tests: `core/tests/people.rs` (drawn faces embedded by `serve` under the
  guard and again by `recognize` after a model change, aligned ones
  suggest and plain ones do not, crops; people as search terms, AND, free
  text, suggestions within a search, undoing a rejection; per-cluster
  generation and splits), `core/tests/recognize.rs` (the `embed` task, a
  worker without it, protocol 1 refused, drawn faces with the real worker
  under `SHOEBOX_RECOGNIZER`), `core/tests/serve.rs` (new endpoints under
  the guard), `recognizer/test_recognizer.py` (embed with and without
  landmarks). The UI was driven with Playwright (Chromium) at 1280 × 800 and
  as an iPad (gen 7, touch: taps and a finger drag for drawing) on a library
  of real faces with the real worker and one of the fake worker's cues.

The plan:

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
  undo a mistake. (Done in the 5c-2 PR.)
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

### 5c feedback: clusters too big

First look at the real drive: one cluster of 2,566 faces (a mother, her
fair-haired children and babies chained together), too long to scroll, "Select"
in a column of the grid was cramped, and an expanded card could not be closed
again. Built:

- **Cluster size capped at `clusters::MAX_CLUSTER` = 100.** A cluster grows
  from neighbour to neighbour at 0.60, which chains similar-looking people
  together. A cluster of more than 100 faces is split with a stricter
  similarity (0.62, 0.64, … up to 1.0, `SPLIT_STEP`) using the neighbour
  lists: the weak links break first, and every piece still too large is
  split again. Faces that are still joined at 1.0 are cut into pieces of
  100. So the smallest threshold that fits is used, rather than a fixed 0.90
  for all. Unit tests in `clusters.rs`.
- **"Looks like X (721 of 2566)"** has a second button "✓ Only the 721" next
  to "✓ X": it names only the faces suggested for X (≥ `SUGGEST_SIM`); the
  rest of the card stays. (Whole card: "✓ X", as before.)
- **Cards**: opening all faces ("+N") or "Select" makes the card as wide as
  the page, with bigger faces, and a "Close ✕" button in a sticky head that
  stays at the top while scrolling.
- **Shift-click** selects a range wherever faces are selected, as in the
  photo grid: the face check page, a person's faces and the cards under
  Unnamed (`pickSpan` in `app.js`; the range runs from the last face
  clicked to this one, in the order shown). The duplicates page has no
  range: its boxes keep at least one copy per group.

## 5d: duplicates UI and tag carry-over

Built, one commit per step. Scope and rules are in "Phase 5d details" in
[plan.md](plan.md). As built:

- **Trash dialog**: `openModal` takes `focus: true` on an action; the "Move to
  trash" button has focus, Enter confirms.
- **Move dialog**: "Keep tags" (default checked). `POST /api/move` takes
  `keep_tags` (default true); unchecked drops the moved photos' own tags
  (`organize::move_files_with`, `tags::drop_own`), folder tags follow the path.
- **Deleting copies**: `POST /api/duplicates/remove {keep, remove, dates?}`
  (`duplicates::remove_copies`). At least one copy must stay; every removed
  file must be a duplicate of a kept one (same full hash, or phash within 8
  bits), so the endpoint cannot delete anything else. Reply: `{trashed,
  tags_added, dates_set, conflicts}`.
- **Who inherits**: each removed copy hands over to the kept copy with the same
  content, else a similar one; the highest resolution, then the earliest
  record, then the first path. Its folder tags and own tags become **own tags**
  of the heir (not where the heir has that tag as a folder tag), read before the
  record goes, applied only for copies that really reached the trash.
- **Capture date** (`library.db` v5, `taken_overrides`, keyed by `quick_hash`,
  so it follows the content like thumbnails; `db::TAKEN` is the date as shown
  in the timeline and info panel; the scan keeps rewriting `files.taken`
  untouched, the file is never modified). Rule (`merge_dates`): the heir keeps
  its date when no copy differs; if it has none, or dates differ by less than a
  day, the **oldest** wins; further apart is a real conflict: nothing happens,
  the reply has `conflicts: [{keep, path, dates}]`, the UI asks and repeats the
  request with `dates: {<keep id>: "<chosen>"}`. Exact copies share content and
  therefore one date, so conflicts only arise among similar photos.
  `userdata.json` is version 3 and lists `taken_overrides`. Version 4 adds
  `view_turns` (photos shown turned in shoebox only, see plan.md, Rotate).
- **Same-folder button**: `GET /api/duplicates/same-folder` → `{groups, copies}`,
  `POST` does it (`duplicates::same_folder_plan`). Per (full hash, folder) one
  file stays: highest resolution (identical for exact copies, so in practice
  the tie rule decides), then the earliest record (`added_at`), then the first
  path. "Oldest path" in the plan is read as oldest record. Pairs already
  decided `distinct` or `linked` with the keeper are skipped; near duplicates
  and other folders are never part of it.
- **Three kinds of groups** (feedback on the first screens): `Group.kind` is
  `identical` (same content), `resolution` ("Same photo, different resolution":
  everything is one row, i.e. surely the same photo, differing in size,
  quality or name) or `similar` (several rows: different shots that look alike,
  a series, repeated clicks). A drop-down with check boxes at the top
  (`localStorage`) chooses which kinds are shown; deleting and the bar count
  only what is shown. The two bulk buttons sit on the right as "Clear Same
  Folder Copies" and "Clear Lower Quality Copies"; the number of files is in
  the tooltip and the confirmation, not in the label (a bracketed count was
  unclear); they are disabled when there is nothing to clear.
- **Similar photos as cards with thumbnails**: in a "similar" group the shots
  are not rows with one thumbnail each; all files lie side by side in one
  wrapping row, every card with its own thumbnail (a shot's other versions
  follow it). Identical and same-photo groups keep one thumbnail at the start
  of the row, since repeating it would add nothing.
- **The bar** (Preselect copies / Clear / Move to trash) stays while the page shows
  duplicates. Clear unticks everything in the shown groups, also the ones
  below the first page; “Preselect copies” puts the suggestion back and is disabled
  while the ticks are exactly the suggestion; Clear and Move to trash are
  disabled with nothing ticked, so one can Clear, tick a single copy and move
  just that. The suggestion is the clearly worse copies and exact repeats of
  the best file, never a different shot.
- **Series are not versions** (feedback: IMG_4284 and IMG_4285 were shown as
  one photo): files with a capture date each, the same size and different
  bytes are different shots (a burst puts several in one second), and two
  camera-style names with the same prefix and different numbers
  ("IMG_4284", "IMG_4285 1") never count as the same photo. They are rows of
  a "Similar photos" group.
- **Pre-selection and the original name**: of every photo with several files
  all but the `pick` are ticked (the page, once; an untick stays). The pick is
  the best quality, then a capture date, then **the file without a copy's
  name**, then size, the earliest record, the first path. The same preference
  decides which file "Clear Same Folder Copies" keeps. A name counts as a copy's
  only if the name without the marker belongs to another file of the group
  (`copy_named`), so a legitimate "Bild 1.jpg" is safe. Markers handled:
  Windows "x - Copy", "x - Copy (2)"; macOS "x copy", "x copy 2", "x 2";
  Chrome/Edge/Firefox/Explorer imports "x (1)", "x(1)"; GNOME "x (copy)",
  "x (another copy)", "x (3rd copy)"; Dropbox "x (Name's conflicted copy …)";
  "x-1", "x_2" (Image Capture and others); the word for copy in German, French,
  Spanish, Italian, Dutch, Polish, Portuguese, Danish/Norwegian and Russian/
  Ukrainian ("Kopie", "copia", "copie", …). Windows, macOS and Explorer patterns
  were checked against web sources, the rest is from experience.
  Identical copies in different folders are ticked too (all but the pick):
  the page only suggests, the folder tags go to the file that stays.
- **Duplicates page** (`app.js`, `groupNode`): a row per photo, i.e. per
  `row` of `/api/duplicates` (below); left one thumbnail (the best version) and
  its file name, right a card per file (resolution, MB, folder, capture date,
  tags: 📁 folder tags, own tags filled). "delete this copy" on each card; the
  last unticked box of a group is disabled. Ticks work across groups; the bar
  at the bottom (“N copies marked”, Clear, Move to trash) sends one request per
  group, then asks about conflicting dates once. `GET /api/duplicates` files
  carry `tags: [{name, own}]`, `row`, and `keeper` (see below).
- **Lower-quality versions** (feedback: an original and a messenger copy with
  another name and resolution are one photo, not two rows). `same_photo` says
  when two files are surely the same photo: identical content, or `phash` at
  most `SURE_BITS` (4) apart, or `SURE_BITS_STRIPPED` (6) when exactly one has
  lost its capture date (what a messenger strips; two undated pictures get no
  leeway), the same shape (long side / short side within 2%, a turned copy
  counts), videos never, and no capture times that differ by more than 2 s.
  Rows are the files that are `same_photo` as the **best** file of their
  component (most pixels, then a capture date, then size, earliest record, first
  path); sameness is not transitive, so anything else gets a row of its own
  (a messenger copy without a date fits two shots a day apart: it joins the one
  it is the same photo as). A file is `worse` (`keeper` = the best file's id)
  when it has fewer pixels, or as many and not the capture date the best has.
  The page ticks every file with a `keeper` once (an untick stays after a
  reload). `GET/POST /api/duplicates/lower-quality` is the button: preview
  `{groups, copies}`, then each best file with its worse versions through
  `remove_copies`, so folders, tags and dates are carried over. Pairs decided
  `distinct`/`linked` never count. Near-but-not-sure photos (bursts, other
  shots) are only ever deleted by hand.
- Tests: `core/tests/dupes.rs` (at-least-one rule and non-duplicates refused,
  tag carry-over without duplicating folder tags, date merge incl. conflict
  and rescan, bulk action only on exact duplicates in one folder and skipping
  decided pairs, lower-quality versions: rows, `keeper`, a burst shot with another
  capture time and a stretched picture stay out, 6 bits count only with a lost
  date, guard: originals unchanged, `verify` clean), plus
  `move_without_keep_tags_drops_own_tags` in `tags.rs`. The page was driven with
  Playwright (Chromium, 1280 × 800) on a small library.

## 5e: lean `thumbs.db` (crops only for faces that wait)

Feedback after 5d: on the real drive `thumbs.db` was 285 MB (7196 photos × 28 KB
= 197 MB, 8836 face crops × 6.1 KB = 53 MB). Photo thumbnails have to stay; the
face crops grew with every face anyone ever looked at, up to ~1 GB at the
expected 150k faces, although only the ones waiting for a decision need a
picture of their own.

Decided:
- A face is **decided** when it is confirmed, ignored or "not a face". A face
  that is only "not this person" still waits (it is unnamed).
- `thumbs.faces` keeps a crop only for **faces without a decision** and for
  **the picture of each person**. Decided faces have none.
- Where a decided face is shown, the browser cuts it out of the photo's
  thumbnail (`zoomFace` in `app.js`: same square and 25% room as the server's
  crops, rotated for turned faces; no request, no stored bytes): the
  "Confirmed" tab of a person's faces and ignored faces in the info panel.
  Confirmed faces in the info panel show the person's picture instead; "Show
  boxes" / hovering still shows where that face is.
- A person's picture is stored once and stays until the user changes it:
  `people::ensure_covers` gives everyone with confirmed faces a picture (the
  largest confirmed face; the face itself when there is one) and picks a new one
  only when the old face is not theirs any more. It runs after every change to
  people and at the start of `serve`. A picture whose photo is missing is kept.
  Pictures stay 160 px (shown at 72 px).
- One cleanup rule instead of a hook in every action: `faces::prune_crops`
  deletes every stored crop that is neither a face without a decision nor a
  person's picture. It runs at the start of `serve` (this is what removes the
  old crops once) and after every change to people or decisions
  (`people_api::tidy`), so "Use as … picture" drops the old picture by itself.
  `crop_of` stores a crop only when `faces::wanted` says so; a decided face asked
  for anyway (face check page, nearest neighbours) is made again, not stored.
- Right-click on a photo while the grid shows exactly one person (sidebar or
  the only search term): "Use as <name>'s picture" next to "Show in Finder" /
  "Copy path". `POST /api/people/{id}/cover` takes `{face}` or `{file}` (their
  largest confirmed face in that photo; 400 when there is none).
- No `VACUUM`: the freed pages of `thumbs.db` are reused by the thumbnails of
  the scans still to come, so the file stops growing instead of shrinking.

Tests: `only_faces_waiting_for_a_decision_and_pictures_keep_a_crop` in
`core/tests/people.rs` (crops of waiting faces, deciding removes them, a
decided face still shown but not stored, the picture changed from a photo, old
crops removed at the start, guard).

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
- [x] 5c-1: on the old Intel MacBook against the exFAT drive:
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
  - [x] `shoebox serve`, "Face check": small faces, rotated faces and "≈"
    gone through (results above); opening photos in the viewer works,
    HEIC included.
  - [x] Time `shoebox recognize --rotated` (family folder, 7148 photos):
    interrupted after 281 photos (3 min), then continued with the other
    6866 in 2072 s (35 min, 3.3 photos/s; the estimate was ~40 min);
    0 failed, 414 faces added in all.
  - [x] `shoebox verify` afterwards (after both passes, the retry and the
    face check page's crops): 7994 files OK.
- [ ] 5c-2 and 5c-3 together, everything in the UI (the 5c-2 checks moved
      here, so no `curl` is needed), on the old Intel MacBook against the
      exFAT drive; then from the iPad:
  - [ ] Install the new recognizer (protocol 2) before anything else:
    `recognizer/install.sh /Volumes/<drive>` (an old `recognizer.py` is
    refused: "speaks protocol 1, this shoebox 2").
  - [ ] Clustering time: run `shoebox recognize` once after the update
    (nothing new to look at); it looks up the neighbours of all ~10,000
    faces. Note the time on its last line ("Clusters: … (N s)"; 0.7 s on
    one core of a 2.1 GHz Xeon for 10,000 made-up faces, so expect a few
    seconds). Run it again: neighbour lists are kept, it should take a
    fraction.
  - [ ] After 5c feedback (clusters of at most 100): run `shoebox recognize`
    once, open Unnamed: no card over 100 faces, how many cards now, is the
    largest one a single person? "✓ Only the N" on a card with a
    suggestion; "+N" and "Select" widen the card, "Close ✕" stays visible.
  - [ ] `shoebox serve`: "Faces" appears in the sidebar with "Unnamed (N
    clusters)". Open it: are the largest cards one person each? Big mixed
    ones (small children: 0.60 may be too loose for them)? Scroll to the
    end: how many cards of a single face?
  - [ ] Name a few people from their cards (type a new name, Enter), one
    child with cards from several ages. Name a second card of the same
    person by picking them from the list. Try "Select" on a mixed card:
    name some faces, "Ignore" the stranger; the card keeps the rest.
  - [ ] Groups: "New group…" twice ("Familie", "Freunde"); put people in
    with "Move to group…" and by dragging onto a group in the sidebar; the
    name list in a card shows people by group; reorder and rename in
    "Groups…".
  - [ ] Suggestions: wait for "grouping faces…" to go from the status line;
    the overview shows "N to check". On a person's page, Suggested and
    Maybe: how many are right (✓) and wrong (✗)? Does the child's older
    face come up as suggested or maybe once faces of several ages are
    confirmed? A face marked ✗ shows under "Not them" and ↺ brings it back.
  - [ ] Corrections: take a wrong face out of a person (Confirmed, ⋯, "Not
    …"), name several faces at once (Select, "Name…"), merge two people
    (⋯, "Merge into…"), "Use as …’s picture".
  - [ ] "Not a face" on the leg in 4a5a1198-….JPG and the hands in
    IMG_9959.JPG, from the viewer's info panel (⋯ on the face): gone from
    the boxes and the panel, listed on the face check page under "Marked
    “not a face”", counted by `shoebox faces stats`; still hidden after
    restarting `serve` and after the next `shoebox recognize --rotated`.
  - [ ] Info panel on a group photo: hover a face (or tap its crop) to see
    its box; "Show boxes" shows names; ✓/✗ on a suggested face; "+ Name" on
    an unnamed one.
  - [ ] Add a missed face: a face lying down or half hidden, "+ Add face",
    drag a box, name it. The status line shows "learning drawn faces…"
    (the recognizer starts in the background; how long on the old Mac?),
    then the face counts for suggestions (or not, if no landmarks were
    found: the terminal says "… without landmarks").
  - [ ] Search: type part of a name; "People" in the suggestions; pick two
    people: only photos with both. Add a tag. A typed name without picking
    it finds the person's photos too. Bookmark the URL and open it again.
  - [ ] From the iPad (`serve --lan`): name a card, ✓/✗ in the info panel,
    draw a face with a finger, "Move to group…", the person's ⋯ menus; no
    action needs hover.
  - [ ] `userdata.json` and `library.db.bak` in `.shoebox/` have the people,
    groups and decisions (drawn faces with `manual`); delete
    `recognition.db` and run `shoebox recognize`: every name and decision
    is still there. `shoebox verify` afterwards.
- [ ] 5d, on the old Intel MacBook against the exFAT drive, then the iPad
      (feedback needed from you; note what looks wrong):
  - [ ] Trash dialog: select a photo, "Move to trash": the button has focus,
    Enter confirms, Escape cancels.
  - [ ] Move dialog: "Keep tags" checked keeps the photo's own tags after
    the move; unchecked drops them (folder tags follow the new folder).
  - [ ] Duplicates page: one thumbnail per group, one card per copy with
    resolution, MB, folder, tags and capture date. Do the numbers match the
    info panel? Is the layout readable on the iPad?
  - [ ] Tick "delete this copy" on all but one card: the last unchecked
    box is disabled, so the original cannot be deleted.
  - [ ] The three kinds: do “Identical photos”, “Same photo, different
    resolution” and “Similar photos” hold what the names say? The “Show” menu
    hides and shows them. Does a file with a copy-style name
    (“IMG (2)”, “IMG - Copy”, “IMG copy 2”, “IMG (1)”) ever stay while the
    original name is ticked? Note any pattern that is not recognised.
  - [ ] A photo and its WhatsApp (or other messenger) copy: they are one row,
    the messenger copy is ticked and says “lower quality”. Do other shots
    of a series stay in rows of their own? Any wrongly ticked copy, or a
    messenger copy that is not recognised (note its name and size)?
  - [ ] “Remove lower-quality versions”: note the count, run it; the better
    file stays with the folder name (“WhatsApp”) as an own tag. Restore one
    from the trash.
  - [ ] Delete a copy in another folder: the survivor shows the copy's
    folder as a removable own tag (and the copy's own tags). Remove it
    again with ✕. Folder tags stay without ✕.
  - [ ] Capture date: a pair where the dates differ or one is missing; the
    survivor shows the existing/oldest date. Check the file itself in the
    Finder: modified and created dates unchanged (`shoebox verify`).
  - [ ] Multi-select across groups, trash in one action; restore one from
    the trash page (tags come back).
  - [ ] Bulk action "same folder": note the count in the confirm dialog,
    run it; the highest resolution stays, near duplicates and copies in
    other folders are untouched. How long on the full library?
  - [ ] `shoebox verify` afterwards; restart `serve`: the override and the
    carried tags are still there; `userdata.json` lists them.

- [ ] 5e: update, start `serve` once. It prints "Tidied up: N people got a
      picture, M face crops nobody needs were removed" and `thumbs.db` stops
      growing (the file keeps its size, the pages are reused by the next scans).
  - [ ] Every person still shows their picture (sidebar, overview, person page).
  - [ ] A person's "Confirmed" tab shows the faces zoomed out of the photos;
        the faces are recognisable (the thumbnails are 384 px).
  - [ ] Info panel: named people show their picture, unnamed ones their face;
        hovering a line shows the box on the photo.
  - [ ] Name a cluster: the cards disappear and `thumbs.db`'s face rows shrink
        (`sqlite3 -readonly .../thumbs.db "SELECT count(*) FROM faces"`).
  - [ ] Right-click a photo on a person's page (from the sidebar and from the
        search box): "Use as … picture" changes the picture; on a photo of
        someone else it says why not.
