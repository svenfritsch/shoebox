# Recognizer protocol

The Rust core starts the recognizer as a child process and talks to it in
JSON lines: one JSON object per line, UTF-8, `\n`-terminated, on the
worker's stdin (requests) and stdout (replies). Anything a worker wants to
log goes to stderr, which the core passes through to its own. The worker is
stateless: it gets pixels and returns boxes and embeddings. It never opens
the library, the database or any original.

Protocol version: **1**.

## Start

The worker loads its models, then writes one line before reading anything:

```json
{"hello": "shoebox-recognizer", "protocol": 1, "version": "0.1.0",
 "tasks": {"faces": {"model": "yunet-2023mar+sface-2021dec", "dim": 128}}}
```

- `protocol` must equal the core's; otherwise the core stops the worker and
  reports the mismatch.
- `tasks` lists what this worker can do. `model` names the models (stored
  with every result, so results of different models are never mixed);
  `dim` is the embedding length.

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
- A key per requested task; tasks added later (`animals`, …) get their own
  keys.

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
2. `recognizer/recognizer.py` in the library's `.shoebox/` folder, or next to
   the shoebox binary's folder (`.shoebox/bin/../recognizer/`), run with the
   standalone Python in `recognizer/runtime/<platform>/bin/python3` if
   present, else `python3` from `PATH`.

Without a worker, `shoebox recognize` reports that recognition is not
installed; everything else works as before.
