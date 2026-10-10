# Recognizer protocol

The Rust core starts the recognizer as a child process and talks to it in
JSON lines: one JSON object per line, UTF-8, `\n`-terminated, on the
worker's stdin (requests) and stdout (replies). Anything a worker wants to
log goes to stderr, which the core passes through to its own. The worker is
stateless: it gets pixels and returns boxes and embeddings. It never opens
the library, the database or any original.

Protocol version: **2** (2 adds the task `embed`, for faces drawn by hand;
phase 5c-3). Core and worker are updated together: a worker of protocol 1
is refused.

## Start

The worker loads its models, then writes one line before reading anything:

```json
{"hello": "shoebox-recognizer", "protocol": 2, "version": "0.2.0",
 "tasks": {"faces": {"model": "yunet-2023mar+sface-2021dec", "dim": 128},
           "embed": {"model": "yunet-2023mar+sface-2021dec", "dim": 128}}}
```

- `protocol` must equal the core's; otherwise the core stops the worker and
  reports the mismatch.
- `tasks` lists what this worker can do. `model` names the models (stored
  with every result, so results of different models are never mixed);
  `dim` is the embedding length.
- `embed` must name the same model and length as `faces` (drawn faces are
  compared with detected ones); a worker without it still finds faces, and
  drawn faces wait for one that has it.

The core waits up to 2 minutes for the hello (loading Python and the models
from a slow external drive can take a while).

## Requests

```json
{"id": 42, "tasks": ["faces"], "image": "<base64 JPEG>"}
```

- `id`: an integer chosen by the core, echoed in the reply.
- `tasks`: a subset of the hello's `tasks`.
- `image`: the picture as base64 (standard alphabet, padded) JPEG or PNG.
  The core decodes the original itself (all formats, HEIC included, EXIF
  orientation applied) under the guard and sends an upright copy whose
  longer edge is at most 1600 px. The worker never sees the original.
- Instead of `image`, a request may carry `"path": "/some/file.jpg"`. The
  core never sends this; it is for trying the worker by hand.

One request at a time: the core waits for the reply before it sends the
next.

## Replies

```json
{"id": 42, "width": 1600, "height": 1200,
 "faces": [{"bbox": [x, y, w, h], "score": 0.97,
            "landmarks": [[x, y], [x, y], [x, y], [x, y], [x, y]],
            "emb": "<base64 little-endian f32 × dim>"}]}
```

- `width`, `height`: size of the image the worker decoded. Coordinates are
  pixels in that image, origin top left; the core stores them as fractions
  of the upright image.
- `faces`: one entry per detected face (possibly none). `landmarks` are the
  eyes, nose tip and mouth corners (right eye, left eye, nose, right and left
  mouth corner, as YuNet orders them). `emb` is L2-normalised, so the cosine
  similarity of two faces is their dot product.
- A key per requested task; tasks added later (`pets`, …) get their own
  keys.

### `embed`: faces drawn by hand

```json
→ {"id": 43, "tasks": ["embed"], "image": "<base64 JPEG>", "boxes": [[x, y, w, h], …]}
← {"id": 43, "width": 1600, "height": 1200,
   "embed": [{"landmarks": [[x, y], …], "emb": "<base64 little-endian f32 × dim>"}, …]}
```

- `boxes`: the boxes the user drew, in pixels of the image sent (the same
  upright ≤ 1600 px copy as for `faces`). One entry per box in `embed`, in
  the same order.
- The worker looks for a face around each box (YuNet with a low score
  threshold, 0.3, in a region half the box larger on every side, enlarged
  to at least 320 px) and takes the detection that overlaps the box best
  (IoU ≥ 0.3). Found: the face is aligned by its five landmarks and
  embedded as usual, and `landmarks` lists them. Not found: the plain crop
  of the box, resized to 112 × 112, is embedded and `landmarks` is `[]`.
- The core uses only the aligned ones as references for suggestions; a
  plain crop's embedding is too unreliable (on test photos a box off the
  face gave 0.25 against the detected face, an aligned one 0.92–0.96).
- The core stores them in `recog.drawn` (key and box of the drawn face,
  model, aligned). `shoebox recognize` embeds drawn faces at the end of
  every run; `shoebox serve` starts the worker for that right after a face
  is drawn and stops it again.

### `pets`: cats and dogs

An optional task (still protocol 2: a core that does not ask never sees it).
The worker only loads the pet models when it is started with
`--pets`; without the flag the hello has no `pets` and a request for
it is an unknown task. `shoebox recognize --pets` starts it that way.

```json
→ {"id": 44, "tasks": ["pets"], "image": "<base64 JPEG>"}
← {"id": 44, "width": 1600, "height": 1200,
   "pets": [{"species": "cat", "bbox": [x, y, w, h], "score": 0.91,
                "emb": "<base64 little-endian f32 × dim>"}]}
```

- The hello lists `"pets": {"model": "yolox-s-2022nov+ppresnet50-2022jan", "dim": 2048}`.
  `model` names the detector and the embedder (stored with every result, so
  results of another embedder are redone, never mixed); `dim` is the
  embedding length and is **not** the face model's. Pet embeddings are
  never compared with face embeddings.
- `species` is `cat` or `dog` (COCO classes; a box counts only where that is
  the best of all 80 classes, so a teddy bear stays one).
- `bbox` holds the **whole pet**, in pixels of the image sent, like a
  face's. The embedding is of that box, slightly enlarged, padded to a
  square and scaled to the embedder's input; it is the mean of the box and
  its mirror image, so a pet looking left matches itself looking right.
  `emb` is L2-normalised.
- Boxes with a shorter side under 48 px are left out.
- The embedder is the first of `dinov2-small`, `ppresnet50-2022jan` whose
  file is in the models folder (`SHOEBOX_PET_EMBEDDER` forces one). It
  runs on onnxruntime if that is installed, else on OpenCV's own runner
  (`SHOEBOX_PET_BACKEND=onnxruntime|opencv` forces one).
- Scale: with PP-ResNet50 two photos of one pet score ≥ 0.95 after
  brightness, blur, crop and size changes, and different pets 0.50–0.68,
  so the core uses far stricter thresholds than for faces
  (`core/src/pets.rs`).

### `text`: words in the picture

An optional task (still protocol 2: a core that does not ask never sees it).
The worker only loads the text models when it is started with `--text`;
without the flag the hello has no `text` and a request for it is an unknown
task. With the text models alone (no face models installed) the worker does
text and says nothing of faces, like with `--pets` alone; `--no-faces` leaves the
face models out even where they are installed (a run that only reads text starts
faster that way).

```json
→ {"id": 46, "tasks": ["text"], "image": "<base64 JPEG>"}
← {"id": 46, "width": 1600, "height": 1200,
   "text": [{"bbox": [x, y, w, h], "score": 0.97, "text": "Rechnung Nr. 2024-0412"}]}
```

- The hello lists `"text": {"model": "ppocr-v4-ch-en", "dim": 0}`. `model` is
  stored with every result, so a different model redoes the text, never mixes
  it; `dim` is 0 (there is no embedding).
- One entry per **line** of text, top to bottom, then left to right; `bbox`
  is the axis-aligned box around the line in pixels of the image sent (tilted
  text gets its enclosing box), clamped to the picture; `score` is the
  recogniser's confidence (0 to 1).
- The worker reports **every** line it read, including junk, symbols and
  characters of other scripts: the **core** filters (confidence, size, only
  German and English characters, `ß` for the `β` the model prints), so the
  rules live in one place and a change needs no new run of the models.
- Engine: PP-OCRv4 (detection, angle classifier, recognition) from the
  `rapidocr-onnxruntime` package on onnxruntime. The three model files
  (`ch_PP-OCRv4_det_infer.onnx`, `ch_PP-OCRv4_rec_infer.onnx`,
  `ch_ppocr_mobile_v2.0_cls_infer.onnx`) are in the models folder (copied there
  by `install.sh --text`); the package must be importable by the worker's
  Python. Missing files or package end the worker with a message on stderr.
- The model reads Latin text well but prints `ä ö ü` without their marks and
  `ß` as `β` or `B` (`docs/plan.md`, phase 9, spike results).

### `embed-pets`: pets drawn by hand

With `--pets` the worker also does `embed-pets`, the counterpart of `embed` for a
box the user drew around a pet the detector missed:

```json
→ {"id": 45, "tasks": ["embed-pets"], "image": "<base64 JPEG>", "boxes": [[x, y, w, h], …]}
← {"id": 45, "width": 1600, "height": 1200, "embed-pets": [{"emb": "<base64 little-endian f32 × dim>"}, …]}
```

The hello lists it with the same model and `dim` as `pets`. Each box is embedded
exactly like a detected pet's (enlarged, padded to a square, mirrored and
averaged); the detector is not asked, the user says there is a pet. A box
outside the picture, or one that is not `[x, y, w, h]`, is an error for the
whole request. The core stores the embedding in `recog.drawn` with the pets
model (`shoebox recognize --pets` does it at the end of every run, and
`shoebox serve` in the background when a pet is drawn, starting the worker
with `--pets` only because one is waiting).

If the worker cannot handle one picture, it answers with an error and keeps
running:

```json
{"id": 42, "error": "cannot decode image"}
```

The core records the error for that content and does not ask again (until
`shoebox recognize --retry-failed`).

## End

The core closes the worker's stdin; the worker exits. A worker that has not
exited 5 seconds later is killed.

## Supervision (core side)

- A reply must arrive within 2 minutes. Otherwise the worker is killed and
  started again.
- Lines on stdout that are not JSON objects, or carry another `id`, are
  logged and skipped.
- If the worker exits or hangs while handling a picture, it is restarted and
  the picture is tried once more. If that fails as well, the picture is
  recorded as failed (`recognizer crashed`), so one bad file cannot stop a
  run.
- After 5 crashes in a row without a single reply in between, the run stops
  with an error (the worker is broken, not the pictures).
- A worker that does not start (missing, no hello, wrong protocol) stops the
  run at once.

## Where the core finds the worker

1. `--recognizer <program>` or the `SHOEBOX_RECOGNIZER` environment variable:
   any executable that speaks this protocol (`shoebox-fake-recognizer` in
   tests).
2. `recognizer/recognizer.py` next to the shoebox binary (the downloaded
   folder), else in the library's `.shoebox/recognizer/` (also what an older
   `.shoebox/bin/` layout means), run with the standalone Python in `recognizer/runtime/<platform>/bin/python3` if
   present, else `python3` from `PATH`.

Without a worker, `shoebox recognize` reports that recognition is not
installed; everything else works as before.
