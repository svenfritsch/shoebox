# Phase 7: pets

Dogs and cats are recognised and handled like people: they appear in the
**Faces** section, are named, put in groups, found in the timeline and
searched like anyone else, by name and by kind ("all cats"). Individual pets
("Spooky") are told apart by the look of the whole pet. This file is the
record of what was built, how it was checked, and what is still to check on
real hardware.

Built on top of phase 6 (launcher, several drives). Pull request:
svenfritsch/shoebox#30.

## 1. Decisions

| Topic | Decision |
|---|---|
| Name | **pets** everywhere the user reads, and in the code too: `--pets`, `pets.rs`, task `pets`, `view=pets`, `kind=pets`. The generic species of a pet nobody classified is `pet`. (It started as "animals"; renamed before anything shipped, so there is no migration) |
| Species | Cats and dogs only (COCO classes). The label is stored, so another species is a small change |
| Detector | YOLOX-S from the OpenCV zoo (COCO, 80 classes), run by OpenCV. A box counts only where cat or dog is the best of all 80 classes, so a teddy bear stays one |
| Box | The **whole pet**, not just its face. COCO has no pet-face class; an embedding of the whole pet is also more reliable for telling individuals apart (coat, size, markings). On a close-up the box is essentially the face |
| Embedder | The first of `dinov2-small` (preferred: best at individuals) and `ppresnet50-2022jan` (PP-ResNet50 from the OpenCV zoo, its pooled 2048-d feature) whose file is in the models folder. The model id (`yolox-s-2022nov+<embedder>`) is stored with every result, so changing it redoes the pets, never mixes them |
| Runtime | onnxruntime if installed, else OpenCV's own runner, so the old Intel Mac (macOS 12, OpenCV 4.10) keeps working and modern computers use the faster path. `install.sh` tries to install onnxruntime and goes on without it |
| Storage | Rows of `recog.faces` with a `species` (`recognition.db` v5). Everything downstream (viewer boxes, people, decisions, crops, check page) works unchanged. Faces and pets never share a pass: `recognize` leaves pet rows alone and `--pets` leaves faces alone |
| Spaces | Embeddings of different models are never compared. Faces and pets are neighbours, clusters and suggestions each in a space of their own (`clusters.rs`), with their own thresholds and minimum size (`pets.rs`, `Space`). Cluster numbers continue (faces first) |
| People | A pet is an ordinary person. Which kind is derived (most confirmed faces are pets: `species` in `/api/people`); no restrictions, so groups mix people and pets |
| Decisions | `face_decisions.species` (`library.db` v7): a decision only ever matches a detected face of its own kind, so a person's face and a pet with the same box in one photo are never mixed up. NULL = a person's face (everything decided before); `cat` / `dog` for a decision made on a detected pet; `pet` for a pet drawn by hand, which matches any detected cat or dog (`pets::kind_matches`) |
| Calibration | Settings → Calibration holds the **Face check** and the **Pet check** (same page, `view=pets`), replacing the Face check link in the sidebar |
| Drawn boxes | The Add face dialog has a checkbox "This is a pet". Ticked, the box is a pet of unknown species |
| Search | Pets are a search term: `pet=cat`, `pet=dog`, `pet=pet` (any pet), and typed words in English and German. Nothing is written as a tag: the species is already stored and a term reads it, so the result always follows the current detections and decisions |
| Text search (CLIP) | Later and separate; it needs no change here |

## 2. The worker (`recognizer/recognizer.py` 0.3.0, still protocol 2)

[protocol.md](protocol.md) has the messages. Optional tasks, loaded only when
the worker is started with `--pets` (the models are big and the drive may be
slow, so face-only runs do not pay for them):

- `pets`: detect, then embed each box (enlarged by 8%, padded to a square
  with grey, scaled to the embedder's input), as the mean of the box and its
  mirror image, so a pet looking left matches itself looking right. Boxes
  under 48 px are dropped.
- `embed-pets`: the same embedding for a box the user drew (the detector is
  not asked), so drawn and detected pets compare.
- `Embedder`: either runtime; reads the output named `pooler_output`, or the
  class token of a token sequence, or a named output (PP-ResNet50's second
  output).
- Env: `SHOEBOX_PET_EMBEDDER=dinov2-small|ppresnet50-2022jan` forces an
  embedder, `SHOEBOX_PET_BACKEND=onnxruntime|opencv` a runtime.
- `fetch-models.sh` fetches YOLOX and PP-ResNet50 (checksummed;
  `SHOEBOX_NO_PETS=1` skips them, about 140 MB). `install.sh` also tries
  `pip install onnxruntime` and carries on without it.

## 3. The core

- **`shoebox recognize --pets`**: a pass of its own (task `pets` in
  `recog.looked`, job kind `pets`), resumable like the others, `--limit` per
  pass, `--retry-failed`. It runs after the faces pass (quick when done).
  `recognize::Worker::pets`, `Worker::embed_pets`.
- **`shoebox faces stats --pets`**: counts, widths, scores of the pets
  (own buckets: widths 64/100/200/400 px, scores 0.45/0.55/0.70/0.85).
- **Pass isolation**: the faces pass and the rotated pass only touch
  `species IS NULL` rows, the pets pass only the others. A model change
  redoes only its own pass.
- **Clusters** (`clusters.rs`): `compute` runs one analysis per space
  (model, neighbours, clusters, suggestions); a person is suggested for
  faces of a space only through their confirmed faces in that space. Cluster
  ids continue after the faces' ones.
- **People** (`people.rs`): `Detected.species`, per-face thresholds
  (`Detected::thresholds`), `FaceItem.species`, `Person.species`
  (`cat` / `dog` / `pet`, derived from confirmed faces including pets drawn
  by hand), decisions record their species and match by `kind_matches`.
- **Drawn pets**: `POST /api/faces/manual` takes `pet: true` (decision
  species `pet`). `embed_drawn` sends faces to `embed` and pets to
  `embed-pets`; pets wait for a worker started with `--pets`. `serve`
  starts the worker with `--pets` only when a pet is waiting, even if no
  pets pass ever ran. A drawn pet is a reference for suggestions among
  pets like a drawn face (a box under 64 px is not).
- **Lean crops** (from main): the crop cache keeps only faces waiting for a
  decision and people's pictures; it is species-agnostic, so pets get the
  same treatment.

### Thresholds (a first guess: set them from the Pet check)

Similarity is the cosine of the embeddings. Measured with PP-ResNet50 on
four pets: one pet after brightness, blur, crop, size and mirror changes
scores 0.95–1.00; different pets 0.50–0.68 (different photos of one pet in
other poses and light will lie in between). Faces: 0.60 / 0.55 / 0.35,
calibrated on the real drive.

| | cluster | suggest | maybe | smallest box |
|---|---|---|---|---|
| faces (SFace) | 0.60 | 0.55 | 0.35 | 30 px |
| pets, PP-ResNet50 (and any unknown embedder) | 0.90 | 0.85 | 0.75 | 64 px |
| pets, DINOv2-small (a guess, never run) | 0.70 | 0.60 | 0.45 | 64 px |

`pets.rs`, `Space::thresholds`. A different embedder changes the whole
scale, so an unknown one starts strict.

## 4. Search by pet

- **Terms**: `pet=cat` (all cats), `pet=dog`, `pet=pet` (any pet). Repeated
  terms must all match, like tags and people (cat and dog together: nothing).
  They combine with folders, tags, people and text.
- **Which photos**: those with a detected pet of the species, named or not,
  plus pets drawn by hand (any pet only, their species is unknown); never a
  detection marked "not a face" (`people::keys_of_pets`).
- **Typed words** (`pets::species_for_word`): at least three letters that
  start a word for the term. Cats: cat, cats, katze, katzen, kater; dogs:
  dog, dogs, hund, hunde; any pet: pet, pets, haustier, haustiere, tier,
  tiere. "ca", "catalog" or "petra" mean nothing. A typed word also still
  matches paths, tags and names as before.
- **Suggestions**: `GET /api/pets/search?q=…&(filter)` returns the terms that
  fit what is typed and would still show photos within the search, with the
  count; terms already in the search are left out. The search box shows them
  under "Pets" (🐱 All cats, 🐶 All dogs, 🐾 Any pet), the search chip carries
  its ✕, Backspace removes the last chip (pets first).
- **All drives**: `pet=` means the same on every drive (no ids), so
  `/api/all/timeline` takes it; the suggestions add up what the drives that
  take part answer.
- **URL**: `#pet=cat` etc., so a pet search can be bookmarked.

## 5. The launcher

A **Recognize pets** button (job `recognize_pets`: the faces pass, then the
pets pass), progress and result like the other jobs; its summary line shows
the pets found and failed.

## 6. The UI

- Settings page; Face check and Pet check pages; the Pet check lists whole
  pets with species tags, "≈" for nearest neighbours, "Select" to mark false
  finds "not a pet".
- Species badge (🐱 🐶; 🐾 for a pet of unknown species) on avatars, names,
  cards, the info panel, and a blue box in the viewer.
- Unnamed: filter "People and pets / People / Pets (cats and dogs)".
- "+ Add face or pet": the dialog has the checkbox "This is a pet (a cat or a
  dog)" and a hint that says what is learned; naming a drawn box again keeps
  it a pet.
- Pet progress in the status line; the search box says "Search people, pets,
  tags, folders".

## 7. API, CLI and compatibility changes

| Where | Change |
|---|---|
| CLI | `shoebox recognize --pets`, `shoebox faces stats --pets` |
| Env | `SHOEBOX_PET_BACKEND`, `SHOEBOX_PET_EMBEDDER`, `SHOEBOX_NO_PETS` (fetch), `SHOEBOX_TEST_PET` (tests) |
| Worker | tasks `pets` and `embed-pets` (only with `--pets`), hello `tasks.pets`, `tasks.embed-pets`; protocol number unchanged |
| `recognition.db` | v5: `faces.species`; tasks `pets` in `looked`, kind `pets` in `jobs` |
| `library.db` | v7: `face_decisions.species`. **An older shoebox cannot open an upgraded library** |
| `userdata.json` | decisions carry `species` (omitted for people's faces) |
| `POST /api/faces/manual` | `pet: true` |
| `GET /api/clusters` | `kind=faces\|pets` |
| `GET /api/faces`, `/api/faces/stats` | `kind=pets`; items and `kind` in the stats |
| `GET /api/pets/search` | new, the pet terms with counts |
| filters | `pet=cat\|dog\|pet` on `/api/timeline`, `/api/tags`, `/api/people/search`, `/api/pets/search`, `/api/all/timeline`; other values are a 400 |
| `/api/people`, face items | `species` (absent for people and their faces) |
| `/api/info` | `faces.pets_done`, `faces.pets` |
| Launcher | job kind `recognize_pets` |
| UI routes | `#view=settings`, `#view=pets`, `#pet=…` |

## 8. Tests

All run without the real models, with the fake worker
(`core/src/bin/shoebox-fake-recognizer.rs`; its header lists the cues:
`--pets` adds `pets` and `embed-pets`, a red picture is a cat, a blue one a
dog, a grey bottom quarter sets the similarity).

- `core/tests/recognize.rs`: the pets pass and its guard (originals
  untouched), resumable, `--limit` per pass, faces untouched by it and the
  other way round, a model change redoes only its own pass, the worker
  without `--pets` refuses, the Pet check (stats, list, crops under the
  guard, neighbours among pets only).
- `core/tests/people.rs`: pets as people (clusters apart, naming, strict
  thresholds, groups with people, merging, pets never suggested for a
  person's face), a pet drawn by hand (embedded, a reference only through the
  drawn box, one face over a detected pet, deleting), a pet drawn while
  serving is embedded in the background, **searching by pet** (all terms,
  typed words, AND, drawn pets, "not a pet", suggestions and counts).
- `core/tests/multi.rs`: pet terms over several drives.
- `core/tests/launcher.rs`: the Recognize pets job under the guard (counts
  the library's photos, so it holds with the CI fixtures too).
- Unit tests: `pets.rs` (thresholds, `kind_matches`, search words),
  `people.rs` (decisions match faces of their own kind), `db.rs` (the v7
  step re-runs safely).
- `recognizer/test_recognizer.py` (real models): hello and flags, `pets`,
  `embed-pets` incl. "a drawn box around a detected pet matches its
  embedding", the embedder plumbing against a generated DINOv2-shaped model
  on both runtimes. `SHOEBOX_TEST_PET` takes a photo of a cat or dog.
- CI (the manual Linux job): fetches the models, runs the Python tests with a
  real dog photo, and `recognize --pets` on the fixtures.

## 9. What was checked, and how

Done while building (a Linux container, not the real hardware):

- All Rust suites and the Python tests pass; the Rust suites also with
  `SHOEBOX_FIXTURES` set, as CI has it (here without the HEIC and video
  fixtures, which this machine cannot make: no HEVC encoder).
- The real worker (YOLOX, PP-ResNet50) on both runtimes, through the Rust
  core: 12 photos of 4 pets (each flipped, brightened and shrunk) gave one
  pet per photo and exactly 4 clusters of 3; neighbours of the same pet
  scored 0.99–1.00 and other pets 0.70 and below.
- The UI in Chromium against that library: Settings, the Pet check (summary,
  filters, neighbours), Unnamed with the Pets filter, naming four clusters,
  grouping, the people overview with badges, a pet's photos, the viewer and
  info panel, drawing a pet with the checkbox (the real worker embedded it
  in the background and a detected pet in the same photo was then
  suggested), and the search: "kat" offers 🐱 All cats (3), picking gives the
  chip and 3 photos, "hund" gives the dogs, `#pet=…` deep links, Backspace.
- CI found one thing that local runs did not: the launcher test assumed no
  fixtures; fixed.
- Observed once, unrelated: `several_libraries_share_one_server…` (phase 6)
  failed in a full run and passes alone; it looks timing-dependent (the
  second library rewrites a file later, so mtime-based order can differ).

Not done: anything on the real drive, the old Intel MacBook or the iPad;
DINOv2 (see below).

## 10. Checklist

On the drive (Intel MacBook and/or a modern Mac):

- [ ] `recognizer/install.sh <drive>` again (the models, onnxruntime if there
      is a wheel); `shoebox recognize <drive> --pets` (or the launcher's
      "Recognize pets"). Note the time over the whole library (YOLOX at 640
      px plus an embedding per pet, twice with the mirror image) and that it
      resumes after Ctrl-C / Cancel.
- [ ] `shoebox faces stats <drive> --pets`: pets found per photo, widths,
      scores, failures.
- [ ] Settings → Calibration → **Pet check**: crops show whole pets with the
      right species; "≈" lists the same pet first (0.9 and more) and other
      pets far below; false finds (plush toys, statues, pictures of pets)
      marked "not a pet" disappear and come back with "It is a pet".
- [ ] Thresholds: do clusters hold one pet each? Do suggestions at 0.85 and
      "maybe" at 0.75 look right? If not, set `Space::thresholds`.
- [ ] Unnamed → Pets: name a few clusters; the pet appears in the sidebar
      with its icon; put it in a group with people; its page shows its
      photos, suggested and maybe tabs work.
- [ ] A person's face and a pet in one photo: naming one does not touch the
      other.
- [ ] **Search**: "kat", "katze", "cat", "hund", "dog", "pets", "haustier" in
      the search box offer the terms with counts; picking one gives a chip
      and the right photos; combined with a folder, a year folder, a person;
      "Clear all" and Backspace; a bookmarked `#pet=cat` opens again.
- [ ] **Drawn pet**: "+ Add face or pet" on a pet the detector missed (small,
      from behind, partly hidden): tick "This is a pet", name it; it is
      listed as 🐾 pet, has a blue box, and the pet's other photos are
      suggested after a moment. Delete it with the ⋯ menu.
- [ ] Several drives: the common timeline with `#pet=cat`; the suggestion
      counts add up the drives that take part, not a backup.
- [ ] Old Intel MacBook (macOS 12): the recognizer starts with `--pets`
      (OpenCV 4.10 runs YOLOX and PP-ResNet50; with DINOv2 only if OpenCV
      can run it), `Recognize pets` finishes.
- [ ] iPad: the Pet check scrolls and opens photos, Unnamed → Pets, the
      search suggestions are easy to hit, the "+ Add face or pet" box can be
      dragged with a finger (also in [ipad-checklist.md](ipad-checklist.md)).
- [ ] After a scan that moved or renamed photos: pets, names and drawn pets
      are still found (decisions are keyed by content and box).
- [ ] An older `library.db` opens (v7 step) and a library with pets in it is
      not opened by an older shoebox (it says so).

## 11. Open

- [ ] **DINOv2.** Not in `fetch-models.sh`: Hugging Face (where
      `onnx-community/dinov2-small` lives) was unreachable when this was
      built, so the file could not be fetched, checksummed or run. Put its
      `onnx/model.onnx` into `recognizer/models/` as `dinov2_small.onnx`
      (or `.shoebox/recognizer/models/`), run `shoebox recognize --pets`
      again (the model id changes, so all photos are looked at again), check
      the neighbours in the Pet check and set its thresholds. Check whether
      OpenCV's runner (the old Mac) can run it; with onnxruntime it should.
      Then pin its SHA-256 in `fetch-models.sh`.
- [ ] A species choice (cat / dog) for pets drawn by hand; today they are
      "pet" and only found by "any pet".
- [ ] Pet terms in the all-drives suggestions are summed per request; for
      many drives a cache like the people list would be kinder.
- [ ] Text search over photo content (CLIP) is a later phase of its own.
