# Phase 7: pets

Dogs and cats are recognised and handled like people: they appear in the
**Faces** section, are named, put in groups, found in the timeline and
searched like anyone else. Individual pets ("Spooky") are told apart by the
look of the whole animal.

## Decisions

| Topic | Decision |
|---|---|
| Species | Cats and dogs only (COCO classes). The label is stored, so another species is a small change |
| Detector | YOLOX-S from the OpenCV zoo (COCO, 80 classes), run by OpenCV. A box counts only where cat or dog is the best of all 80 classes, so a teddy bear stays one |
| Box | The **whole animal**, not just its face. COCO has no pet-face class; an embedding of the whole animal is also more reliable for telling individuals apart (coat, size, markings) |
| Embedder | The first of `dinov2-small` (preferred: best at individuals) and `ppresnet50-2022jan` (PP-ResNet50 from the OpenCV zoo, its pooled 2048-d feature) whose file is in the models folder; the model id (`yolox-s-2022nov+<embedder>`) is stored with every result, so changing it redoes the animals, never mixes them |
| Runtime | onnxruntime if installed, else OpenCV's own runner, so the old Intel Mac (macOS 12, OpenCV 4.10) still works and modern computers use the faster path. `install.sh` tries to install onnxruntime and goes on without it |
| Storage | Rows of `recog.faces` with `species` (`recognition.db` v5). Everything downstream (viewer boxes, people, decisions, crops, face check) works unchanged. Faces and animals never share a pass: `recognize` leaves animal rows alone and `--animals` leaves faces alone |
| Spaces | Embeddings of different models are never compared. Faces and animals are neighbours, clusters and suggestions each in a space of their own (`clusters.rs`), with their own thresholds and minimum size (`animals.rs`, `Space`). Cluster numbers continue (faces first) |
| People | A pet is an ordinary person. Which kind of person is derived (most confirmed faces are animals: `species` in `/api/people`); there are no restrictions, so groups mix people and pets |
| Decisions | `face_decisions.species` (`library.db` v7): a decision only ever matches a detected face of its own kind, so a person's face and an animal with the same box in one photo are never mixed up. NULL = a person's face (everything decided before) |
| Calibration | Settings → Calibration holds the **Face check** and the new **Animal check** (same page, `view=animals`), replacing the Face check link in the sidebar |
| Drawn boxes | Boxes drawn by hand are faces. Drawing a missed pet is not supported yet |

## What was built

- **Worker** (`recognizer/recognizer.py` 0.3.0, [protocol.md](protocol.md)):
  optional task `animals`, loaded only with `--animals`; `Embedder` (either
  runtime; reads `pooler_output` or the class token of a token sequence),
  `Animals` (detect, pad to a square, embed the box and its mirror image
  and average). `fetch-models.sh` fetches YOLOX and PP-ResNet50 (checksummed;
  `SHOEBOX_NO_ANIMALS=1` skips them, ~140 MB).
- **Core**: `shoebox recognize --animals` (a pass of its own, task `animals`,
  resumable like the others, `--limit` per pass, `--retry-failed`),
  `shoebox faces stats --animals`, `recognize::Worker::animals`,
  `animals.rs`, per-space `clusters.rs`, species on decisions, `kind=animals`
  on `/api/clusters`, `/api/faces` and `/api/faces/stats`.
- **Launcher**: a **Recognize pets** button (job `recognize_animals`: the
  faces pass, which is quick when done, then the animals pass).
- **UI**: Settings page; Animal check; species badge (🐱 🐶) on avatars,
  names, cards and the viewer's boxes (blue); "People and pets / People /
  Pets" filter on Unnamed; pet progress in the status line.
- **Tests**: `core/tests/recognize.rs` (the pass, its guard, model changes
  redo only their own pass, the check page), `core/tests/people.rs` (pets
  as people: clusters apart, naming, thresholds, groups, merging),
  `core/tests/launcher.rs`, `recognizer/test_recognizer.py` (real models;
  `SHOEBOX_TEST_ANIMAL` for a photo; the embedder plumbing against a
  generated DINOv2-shaped model on both runtimes). The fake worker with
  `--animals` has its own cues (see its header).

## Thresholds (a first guess: set them from the Animal check)

Similarity is the cosine of the embeddings. Measured with PP-ResNet50 on
four animals: one animal after brightness, blur, crop, size and mirror
changes scores 0.95–1.00; different animals 0.50–0.68 (different photos of
one pet in other poses and light will lie in between). Faces: 0.60 / 0.55 /
0.35, calibrated on the real drive.

| | cluster | suggest | maybe | smallest box |
|---|---|---|---|---|
| faces (SFace) | 0.60 | 0.55 | 0.35 | 30 px |
| animals, PP-ResNet50 (and any unknown embedder) | 0.90 | 0.85 | 0.75 | 64 px |
| animals, DINOv2-small (a guess, never run) | 0.70 | 0.60 | 0.45 | 64 px |

`animals.rs`, `Space::thresholds`. A different embedder changes the whole
scale, so an unknown one starts strict.

## Open

- [ ] **DINOv2.** Not in `fetch-models.sh`: Hugging Face (where
      `onnx-community/dinov2-small` lives) was unreachable when this was
      built, so the file could not be fetched, checksummed or run. Put its
      `onnx/model.onnx` into `recognizer/models/` as `dinov2_small.onnx`
      (or `.shoebox/recognizer/models/`), run `shoebox recognize --animals`
      again (the model id changes, so all photos are looked at again), check
      the neighbours in the Animal check and set its thresholds. Check
      whether OpenCV's runner (the old Mac) can run it; with onnxruntime it
      should. Then pin its SHA-256 in `fetch-models.sh`.
- [ ] Real-hardware run on the drive: time of the animals pass over the
      library (YOLOX at 640 px plus one embedding per animal, twice with
      the mirror image), the Animal check, the neighbours of a few pets,
      thresholds, false finds (plush toys, statues) marked "not an animal".
- [ ] The old Intel MacBook: `recognizer/install.sh` again (onnxruntime is
      optional there), then `shoebox recognize --animals`.
- [ ] Boxes drawn by hand for pets (a species choice on the drawing tool, an
      `embed` task for animals).
- [ ] Text search (CLIP) is a later phase of its own; it needs no change here.
