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
| **5b** | Own tags: add/remove, many photos at once, search; user data backup | 1 | `library.db` v3 |
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
| Reveal | The server opens Finder / Explorer **on the computer it runs on**, so only for requests from that computer (localhost). Other devices (iPad) get "copy path" only. |

## 5a: show in Finder / Explorer

Being worked on in a separate chat. Agreed scope:

- `POST /api/files/{id}/reveal`: path from the index only (never from the
  request), command started without a shell:
  - macOS: `open -R <path>` (Finder, file selected)
  - Windows: `explorer.exe /select,<path>` (once there is a Windows build)
  - Linux: `xdg-open <folder>`
- The OS is known at compile time (`cfg!(target_os)`); `/api/info` tells the
  UI the label ("Show in Finder" / "Show in Explorer" / "Open folder") and
  whether reveal is possible for this client (localhost only).
- "Copy path" works everywhere, including the iPad.
- Where: right-click menu on a grid cell and a button in the info panel.
- Touches no original. Finder may write a `.DS_Store` into the folder,
  which the scanner already skips.
- Test: the endpoint refuses non-local clients and unknown ids; the guard
  still holds; the command itself is replaced by a stub in tests.

## 5b: own tags

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

## 5c: faces

### 5c-1: did recognition work?

Done on the real drive after the phase 4 hardware run, before any
clustering:
- `shoebox faces stats <root>`: photos looked at, errors, faces found,
  distribution of face size and score, share of tiny faces, time taken.
- A debug page in the UI: all faces as crops, sortable by size and score,
  so false detections (posters, statues, background) are easy to see.
- From that: the minimum face size for clustering, and the similarity
  threshold for "same person" (SFace's usual cosine threshold is about
  0.36; calibrated on our photos).
- **Decided from the phase 4 run** (9564 faces, see
  [phase4.md](phase4.md)): faces **under 40 px** wide (in the ≤1600 px
  copy, about 5% of all) are listed but neither clustered nor suggested.
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
- Face crops: made from the original under the guard, cached in
  `thumbs.db` keyed by face.

### 5c-2: people, groups, clustering (backend)

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
-- What the user decided about a face. Keyed by content and box, so it
-- survives moves, rescans and model changes.
CREATE TABLE face_decisions (
    key       TEXT NOT NULL,       -- files.quick_hash
    x REAL NOT NULL, y REAL NOT NULL, w REAL NOT NULL, h REAL NOT NULL,
    person_id INTEGER REFERENCES people(id) ON DELETE CASCADE,  -- NULL with 'ignored'
    decision  TEXT NOT NULL,       -- confirmed, rejected (not this person), ignored (stranger)
    at        INTEGER NOT NULL
);
```

In `recognition.db` (cache, rebuilt when needed): the cluster and the
suggested person per face.

Matching and clustering:
- Only faces above the 5c-1 size and score thresholds take part; smaller
  ones are listed but never suggested.
- A new face is suggested for a person when it is close enough to that
  person's confirmed faces, and never for a person it was rejected for.
- Unassigned faces go into clusters, incrementally: each new face looks up
  its nearest neighbours (approximate index, `instant-distance` or
  `hnsw_rs`, or plain SIMD dot products if that is fast enough for ~150k
  faces) and joins a cluster or starts one. No all-pairs pass.
- Runs after `shoebox recognize` and in the background in `serve`;
  resumable, progress in `/api/info`.

API: people (list, create, rename, merge, hide, change group, set cover),
groups (list, create, rename, reorder, delete), clusters (list, name,
ignore, split), faces (confirm, reject, assign, ignore), photos of a person
for the timeline.

Tests (`core/tests/people.rs`): with the fake worker's embeddings: suggest,
confirm, reject, merge, split, groups (one per person, delete → no group),
decisions survive a model change and a move, guard.

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
  assigning quick. "Ignore" for strangers.
- **Corrections:** merge two people, take faces out of a person ("not this
  person"), split a cluster, name several selected faces at once.
- **Groups:** create, rename, reorder, delete; change a person's group by
  drag and drop or "Move to group…".
- **Info panel (viewer):** a "People" section with face crops and names.
  Unnamed faces show "+ Name"; suggested ones ✓ (confirm) and ✗ (reject).
  Hovering a face highlights its box in the photo (boxes exist since
  phase 4).
- **Add a missed face by hand** (moved up from "later": it is what makes
  missed faces acceptable): "Add face" in the viewer, draw a box, name it.
  - The box is user data: stored in `library.db` with the face decisions
    (key + box, `decision = 'manual'`), so it survives model changes.
  - Its embedding comes from the worker: protocol v2 adds a task `embed`
    (`{"tasks": ["embed"], "image": …, "boxes": [[x, y, w, h]]}`). The
    worker looks for landmarks inside the box with a low threshold and
    aligns as usual; without landmarks it embeds the plain crop (less
    reliable, so such faces are not used as references for suggestions).
  - Core and worker are updated together (the hello's protocol must
    match, as now).
- Search finds people by name, like tags.
- Recognition and clustering progress in the status line.

## Later (not in phase 5)

- Filter the timeline by several people ("Aurelia and Grandpa").
- Undo for assignments.

## Still to check on real hardware

- [ ] 5a: "Show in Finder" on the old Intel MacBook opens the right folder
      with the file selected; the iPad only offers "copy path".
- [ ] 5b: tag a few hundred photos at once on the exFAT drive; tags survive
      a move, a rescan, trash and restore.
- [ ] 5c-1: `shoebox faces stats` on the drive; pick the thresholds.
- [ ] 5c-2: clustering time over all faces on the old Intel MacBook.
- [ ] 5c-3: naming and correcting from the iPad.
