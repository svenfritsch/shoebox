#!/usr/bin/env python3
"""shoebox recognizer: face detection and embeddings for the Rust core.

Speaks the JSON-lines protocol in docs/protocol.md on stdin/stdout. Stateless:
pixels in, boxes and embeddings out. Never opens the library or the database.

    python3 recognizer.py            # what shoebox runs
    echo '{"id": 1, "tasks": ["faces"], "path": "photo.jpg"}' | python3 recognizer.py

Models (ONNX, from the OpenCV model zoo) are looked up in ./models next to this
file, or in $SHOEBOX_MODELS:
    face_detection_yunet_2023mar.onnx   (YuNet, detection + 5 landmarks)
    face_recognition_sface_2021dec.onnx (SFace, 128-d embedding)
"""

import base64
import json
import os
import sys

PROTOCOL = 1
VERSION = "0.1.0"

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

    def __call__(self, img):
        h, w = img.shape[:2]
        self.detector.setInputSize((w, h))
        _, found = self.detector.detect(img)
        faces = []
        for row in found if found is not None else []:
            x, y, bw, bh = (float(v) for v in row[:4])
            if min(bw, bh) < MIN_FACE:
                continue
            aligned = self.embedder.alignCrop(img, row)
            emb = self.embedder.feature(aligned).astype(self.np.float32).reshape(-1)
            norm = float(self.np.linalg.norm(emb))
            if not self.np.isfinite(norm) or norm == 0.0:
                continue
            emb = (emb / norm).astype("<f4")
            faces.append(
                {
                    "bbox": [round(x, 2), round(y, 2), round(bw, 2), round(bh, 2)],
                    "score": round(float(row[14]), 4),
                    "landmarks": [[round(float(row[i]), 2), round(float(row[i + 1]), 2)] for i in range(4, 14, 2)],
                    "emb": base64.b64encode(emb.tobytes()).decode("ascii"),
                }
            )
        return faces


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
        reply[task] = tasks[task](img)
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
    tasks = {"faces": Faces(cv2, np)}
    hello = {
        "hello": "shoebox-recognizer",
        "protocol": PROTOCOL,
        "version": VERSION,
        "tasks": {"faces": {"model": FACES_MODEL, "dim": FACES_DIM}},
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
