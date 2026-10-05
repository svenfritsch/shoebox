#!/usr/bin/env python3
"""shoebox recognizer: face detection and embeddings for the Rust core.

Speaks the JSON-lines protocol in docs/protocol.md on stdin/stdout. Stateless:
pixels in, boxes and embeddings out. Never opens the library or the database.

    python3 recognizer.py            # what shoebox runs
    echo '{"id": 1, "tasks": ["faces"], "path": "photo.jpg"}' | python3 recognizer.py
    echo '{"id": 2, "tasks": ["embed"], "path": "photo.jpg", "boxes": [[100, 80, 60, 70]]}' | python3 recognizer.py

Models (ONNX, from the OpenCV model zoo) are looked up in ./models next to this
file, or in $SHOEBOX_MODELS:
    face_detection_yunet_2023mar.onnx   (YuNet, detection + 5 landmarks)
    face_recognition_sface_2021dec.onnx (SFace, 128-d embedding)
"""

import base64
import json
import os
import sys

PROTOCOL = 2
VERSION = "0.2.0"

DETECTOR = "face_detection_yunet_2023mar.onnx"
EMBEDDER = "face_recognition_sface_2021dec.onnx"
FACES_MODEL = "yunet-2023mar+sface-2021dec"
FACES_DIM = 128

# YuNet's own demo uses 0.9; a little lower finds more turned and partly
# hidden faces, and the core keeps the score for later filtering.
SCORE_THRESHOLD = 0.85
NMS_THRESHOLD = 0.3
TOP_K = 5000
# Faces smaller than this (px, shorter side of the box) give embeddings too
# poor to match anyone.
MIN_FACE = 24
# The core sends at most this; pictures read from a path are shrunk to it.
MAX_EDGE = 1600
# Faces drawn by hand (task "embed"): landmarks are looked for around the
# box with this lower score threshold (the user says there is a face), in a
# region this much larger than the box on each side, enlarged to at least
# EMBED_REGION px so small faces are seen.
EMBED_SCORE = 0.3
EMBED_PAD = 0.5
EMBED_REGION = 320
# A detection belongs to the drawn box when they overlap at least this much.
EMBED_IOU = 0.3


def models_dir():
    return os.environ.get("SHOEBOX_MODELS") or os.path.join(os.path.dirname(os.path.abspath(__file__)), "models")


class Faces:
    def __init__(self, cv2, np):
        self.cv2 = cv2
        self.np = np
        d = models_dir()
        detector, embedder = os.path.join(d, DETECTOR), os.path.join(d, EMBEDDER)
        for path in (detector, embedder):
            if not os.path.isfile(path):
                raise SystemExit(f"recognizer: model missing: {path} (run recognizer/fetch-models.sh)")
        self.detector = cv2.FaceDetectorYN.create(detector, "", (320, 320), SCORE_THRESHOLD, NMS_THRESHOLD, TOP_K)
        self.embedder = cv2.FaceRecognizerSF.create(embedder, "")

    def __call__(self, img, req=None):
        h, w = img.shape[:2]
        self.detector.setInputSize((w, h))
        _, found = self.detector.detect(img)
        faces = []
        for row in found if found is not None else []:
            x, y, bw, bh = (float(v) for v in row[:4])
            if min(bw, bh) < MIN_FACE:
                continue
            emb = self.embed_row(img, row)
            if emb is None:
                continue
            faces.append(
                {
                    "bbox": [round(x, 2), round(y, 2), round(bw, 2), round(bh, 2)],
                    "score": round(float(row[14]), 4),
                    "landmarks": landmarks(row),
                    "emb": emb,
                }
            )
        return faces

    def embed_row(self, img, row):
        """The embedding of a detected face, aligned by its landmarks."""
        return self.encode(self.embedder.feature(self.embedder.alignCrop(img, row)))

    def encode(self, feature):
        emb = feature.astype(self.np.float32).reshape(-1)
        norm = float(self.np.linalg.norm(emb))
        if not self.np.isfinite(norm) or norm == 0.0:
            return None
        return base64.b64encode((emb / norm).astype("<f4").tobytes()).decode("ascii")


def landmarks(row):
    return [[round(float(row[i]), 2), round(float(row[i + 1]), 2)] for i in range(4, 14, 2)]


def iou(a, b):
    ax, ay, aw, ah = a
    bx, by, bw, bh = b
    iw = max(0.0, min(ax + aw, bx + bw) - max(ax, bx))
    ih = max(0.0, min(ay + ah, by + bh) - max(ay, by))
    inter = iw * ih
    union = aw * ah + bw * bh - inter
    return inter / union if union > 0 else 0.0


class Embed:
    """Faces drawn by hand (protocol 2): one embedding per box in
    req["boxes"] ([x, y, w, h] in pixels of the picture). Landmarks are looked
    for around the box with a low threshold and the face aligned as usual;
    without landmarks the plain crop is embedded (less reliable: the core
    does not use those for suggestions)."""

    def __init__(self, faces, cv2, np):
        self.faces = faces
        self.cv2 = cv2
        self.np = np
        d = models_dir()
        self.detector = cv2.FaceDetectorYN.create(os.path.join(d, DETECTOR), "", (320, 320), EMBED_SCORE, NMS_THRESHOLD, TOP_K)

    def __call__(self, img, req):
        boxes = req.get("boxes")
        if not isinstance(boxes, list):
            raise ValueError("embed needs boxes")
        return [self.one(img, b) for b in boxes]

    def one(self, img, b):
        if not (isinstance(b, list) and len(b) == 4):
            raise ValueError("a box is [x, y, w, h]")
        h, w = img.shape[:2]
        x, y, bw, bh = (float(v) for v in b)
        x0, y0 = max(0, int(x)), max(0, int(y))
        x1, y1 = min(w, int(round(x + bw))), min(h, int(round(y + bh)))
        if x1 - x0 < 2 or y1 - y0 < 2:
            raise ValueError("box outside the picture")
        row = self.find(img, (x, y, bw, bh))
        if row is not None:
            emb = self.faces.embed_row(img, row)
            if emb is not None:
                return {"landmarks": landmarks(row), "emb": emb}
        crop = self.cv2.resize(img[y0:y1, x0:x1], (112, 112), interpolation=self.cv2.INTER_AREA)
        emb = self.faces.encode(self.faces.embedder.feature(crop))
        if emb is None:
            raise ValueError("cannot embed the box")
        return {"landmarks": [], "emb": emb}

    def find(self, img, box):
        """The detection (low threshold) around the box that overlaps it
        best, as a row in the picture's coordinates, or None."""
        h, w = img.shape[:2]
        x, y, bw, bh = box
        pad = EMBED_PAD * max(bw, bh)
        rx0, ry0 = max(0, int(x - pad)), max(0, int(y - pad))
        rx1, ry1 = min(w, int(x + bw + pad)), min(h, int(y + bh + pad))
        region = img[ry0:ry1, rx0:rx1]
        rh, rw = region.shape[:2]
        scale = max(1.0, EMBED_REGION / max(rw, rh))
        if scale > 1.0:
            region = self.cv2.resize(region, (round(rw * scale), round(rh * scale)), interpolation=self.cv2.INTER_CUBIC)
        self.detector.setInputSize((region.shape[1], region.shape[0]))
        _, found = self.detector.detect(region)
        best, best_iou = None, EMBED_IOU
        for row in found if found is not None else []:
            row = row.copy()
            row[:14] /= scale
            row[0] += rx0
            row[1] += ry0
            row[4:14:2] += rx0
            row[5:14:2] += ry0
            overlap = iou(tuple(float(v) for v in row[:4]), box)
            if overlap >= best_iou:
                best, best_iou = row, overlap
        return best


def decode(cv2, np, req):
    if "image" in req:
        data = np.frombuffer(base64.b64decode(req["image"], validate=True), dtype=np.uint8)
        img = cv2.imdecode(data, cv2.IMREAD_COLOR)
    elif "path" in req:
        img = cv2.imread(req["path"], cv2.IMREAD_COLOR)
    else:
        raise ValueError("request has neither image nor path")
    if img is None:
        raise ValueError("cannot decode image")
    h, w = img.shape[:2]
    if max(w, h) > MAX_EDGE:
        scale = MAX_EDGE / max(w, h)
        img = cv2.resize(img, (max(1, round(w * scale)), max(1, round(h * scale))), interpolation=cv2.INTER_AREA)
    return img


def handle(cv2, np, tasks, req):
    img = decode(cv2, np, req)
    h, w = img.shape[:2]
    reply = {"id": req["id"], "width": w, "height": h}
    for task in req.get("tasks", []):
        if task not in tasks:
            raise ValueError(f"unknown task: {task}")
        reply[task] = tasks[task](img, req)
    return reply


def main():
    # The protocol owns stdout; anything printed by us or a library goes to stderr.
    out = sys.stdout
    sys.stdout = sys.stderr
    try:
        import cv2
        import numpy as np
    except ImportError as e:
        raise SystemExit(f"recognizer: {e} (pip install opencv-python-headless numpy)")

    try:  # OpenCV 5 warns about every network it loads
        cv2.utils.logging.setLogLevel(cv2.utils.logging.LOG_LEVEL_ERROR)
    except AttributeError:
        pass
    faces = Faces(cv2, np)
    tasks = {"faces": faces, "embed": Embed(faces, cv2, np)}
    hello = {
        "hello": "shoebox-recognizer",
        "protocol": PROTOCOL,
        "version": VERSION,
        # Drawn faces are embedded with the same models, so they compare with
        # detected ones.
        "tasks": {
            "faces": {"model": FACES_MODEL, "dim": FACES_DIM},
            "embed": {"model": FACES_MODEL, "dim": FACES_DIM},
        },
    }
    out.write(json.dumps(hello) + "\n")
    out.flush()

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except ValueError as e:
            print(f"recognizer: not JSON: {e}", file=sys.stderr)
            continue
        if not isinstance(req, dict) or "id" not in req:
            print("recognizer: request without id", file=sys.stderr)
            continue
        try:
            reply = handle(cv2, np, tasks, req)
        except Exception as e:  # one bad picture must not stop the worker
            reply = {"id": req["id"], "error": f"{type(e).__name__}: {e}" if not isinstance(e, ValueError) else str(e)}
        out.write(json.dumps(reply) + "\n")
        out.flush()


if __name__ == "__main__":
    main()
