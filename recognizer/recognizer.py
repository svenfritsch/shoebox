#!/usr/bin/env python3
"""shoebox recognizer: face detection and embeddings for the Rust core.

Speaks the JSON-lines protocol in docs/protocol.md on stdin/stdout. Stateless:
pixels in, boxes and embeddings out. Never opens the library or the database.

    python3 recognizer.py            # what shoebox runs
    python3 recognizer.py --animals  # also loads the cat and dog models
    echo '{"id": 1, "tasks": ["faces"], "path": "photo.jpg"}' | python3 recognizer.py
    echo '{"id": 2, "tasks": ["embed"], "path": "photo.jpg", "boxes": [[100, 80, 60, 70]]}' | python3 recognizer.py

Models (ONNX, from the OpenCV model zoo) are looked up in ./models next to this
file, or in $SHOEBOX_MODELS:
    face_detection_yunet_2023mar.onnx   (YuNet, detection + 5 landmarks)
    face_recognition_sface_2021dec.onnx (SFace, 128-d embedding)
and for --animals:
    object_detection_yolox_2022nov.onnx (YOLOX-S, COCO: cats and dogs)
    dinov2_small.onnx or image_classification_ppresnet50_2022jan.onnx (embedding)
"""

import base64
import json
import os
import sys

PROTOCOL = 2
VERSION = "0.3.0"

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


# Animals (task "animals"): a COCO detector finds cats and dogs, an image
# embedder describes each animal's box. Optional: without the models the
# worker still does faces.
ANIMAL_DETECTOR = "object_detection_yolox_2022nov.onnx"
ANIMAL_DETECTOR_ID = "yolox-s-2022nov"
# COCO class numbers of the animals we keep.
COCO_ANIMALS = {15: "cat", 16: "dog"}
# Objectness times class score; lower finds more animals that are partly
# hidden or far away, higher fewer plush toys.
ANIMAL_SCORE = 0.35
ANIMAL_NMS = 0.45
YOLOX_SIZE = 640
# Animals smaller than this (px, shorter side of the box) give embeddings too
# poor to tell individuals apart.
MIN_ANIMAL = 48
# The box is enlarged by this much on every side before it is embedded, so
# ears and paws that the detector cut off are in.
ANIMAL_PAD = 0.08
# Embedder input: the box is padded to a square and scaled to this.
IMAGENET_MEAN = (0.485, 0.456, 0.406)
IMAGENET_STD = (0.229, 0.224, 0.225)
# The embedders in order of preference; the first whose file is in the models
# folder is used. `output`: the name of the output holding the embedding
# (None: the first output; a sequence of tokens gives its first, the class
# token).
EMBEDDERS = [
    # DINOv2-small as exported by Hugging Face (onnx-community/dinov2-small);
    # best at telling individual animals apart. Not part of fetch-models.sh:
    # see recognizer/README.md.
    {"id": "dinov2-small", "file": "dinov2_small.onnx", "size": 224, "mean": IMAGENET_MEAN, "std": IMAGENET_STD, "output": "pooler_output"},
    # PP-ResNet50 from the OpenCV zoo: its second output is the pooled
    # 2048-d feature before the classifier.
    {
        "id": "ppresnet50-2022jan",
        "file": "image_classification_ppresnet50_2022jan.onnx",
        "size": 224,
        "mean": IMAGENET_MEAN,
        "std": IMAGENET_STD,
        "output": "save_infer_model/scale_1.tmp_0",
    },
]
# Average the embedding of the box and of its mirror image, so an animal
# looking left matches itself looking right.
ANIMAL_FLIP = True


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


class Embedder:
    """An ONNX image embedder: behind onnxruntime when that is installed,
    else behind OpenCV's own runner (the oldest Intel Macs have no
    onnxruntime wheel). SHOEBOX_ANIMAL_BACKEND = auto | onnxruntime | opencv."""

    def __init__(self, cv2, np, spec, path):
        self.cv2 = cv2
        self.np = np
        self.spec = spec
        self.session = None
        self.net = None
        backend = os.environ.get("SHOEBOX_ANIMAL_BACKEND", "auto")
        if backend not in ("auto", "onnxruntime", "opencv"):
            raise SystemExit(f"recognizer: SHOEBOX_ANIMAL_BACKEND must be auto, onnxruntime or opencv, not {backend!r}")
        if backend in ("auto", "onnxruntime"):
            try:
                import onnxruntime

                options = onnxruntime.SessionOptions()
                options.log_severity_level = 3
                self.session = onnxruntime.InferenceSession(path, options, providers=["CPUExecutionProvider"])
            except ImportError:
                if backend == "onnxruntime":
                    raise SystemExit("recognizer: onnxruntime is not installed (pip install onnxruntime)")
        if self.session is None:
            self.net = cv2.dnn.readNetFromONNX(path)
        self.backend = "onnxruntime" if self.session is not None else "opencv"
        self.dim = len(self.vector(np.zeros((spec["size"], spec["size"], 3), dtype=np.uint8)))

    def vector(self, square):
        """The raw embedding of a square BGR picture of `size` px."""
        np = self.np
        spec = self.spec
        rgb = square[:, :, ::-1].astype(np.float32) / 255.0
        rgb = (rgb - np.array(spec["mean"], dtype=np.float32)) / np.array(spec["std"], dtype=np.float32)
        blob = np.ascontiguousarray(rgb.transpose(2, 0, 1)[None])
        wanted = spec["output"]
        if self.session is not None:
            names = [o.name for o in self.session.get_outputs()]
            outs = self.session.run(None, {self.session.get_inputs()[0].name: blob})
            out = outs[names.index(wanted)] if wanted in names else outs[0]
        else:
            self.net.setInput(blob)
            names = list(self.net.getUnconnectedOutLayersNames())
            if wanted in names:
                out = self.net.forward(names)[names.index(wanted)]
            else:
                out = self.net.forward()
        out = np.asarray(out, dtype=np.float32)
        if out.ndim == 3:  # a sequence of tokens: the first is the class token
            out = out[:, 0]
        return out.reshape(-1)

    def __call__(self, square):
        """The L2-normalised embedding of a square BGR picture; with
        ANIMAL_FLIP the mean of it and its mirror image."""
        np = self.np
        views = [square, square[:, ::-1]] if ANIMAL_FLIP else [square]
        total = None
        for view in views:
            v = self.vector(np.ascontiguousarray(view))
            n = float(np.linalg.norm(v))
            if not np.isfinite(n) or n == 0.0:
                return None
            v = v / n
            total = v if total is None else total + v
        n = float(np.linalg.norm(total))
        if not np.isfinite(n) or n == 0.0:
            return None
        return base64.b64encode((total / n).astype("<f4").tobytes()).decode("ascii")


class Animals:
    """Cats and dogs (protocol 2, optional task "animals"): YOLOX finds the
    animals (COCO classes cat and dog), the embedder describes each box.
    Boxes hold the whole animal, not just its face."""

    def __init__(self, cv2, np):
        self.cv2 = cv2
        self.np = np
        d = models_dir()
        detector = os.path.join(d, ANIMAL_DETECTOR)
        if not os.path.isfile(detector):
            raise SystemExit(f"recognizer: model missing: {detector} (run recognizer/fetch-models.sh)")
        forced = os.environ.get("SHOEBOX_ANIMAL_EMBEDDER")
        spec = next((s for s in EMBEDDERS if (forced or s["id"]) == s["id"] and os.path.isfile(os.path.join(d, s["file"]))), None)
        if spec is None:
            raise SystemExit(f"recognizer: no animal embedder in {d} (run recognizer/fetch-models.sh)")
        self.net = cv2.dnn.readNetFromONNX(detector)
        self.embedder = Embedder(cv2, np, spec, os.path.join(d, spec["file"]))
        self.model = f"{ANIMAL_DETECTOR_ID}+{spec['id']}"
        self.dim = self.embedder.dim
        grids, strides = [], []
        for s in (8, 16, 32):
            n = YOLOX_SIZE // s
            gx, gy = np.meshgrid(np.arange(n), np.arange(n))
            grids.append(np.stack((gx, gy), 2).reshape(-1, 2))
            strides.append(np.full((n * n, 1), s))
        self.grids = np.concatenate(grids).astype(np.float32)
        self.strides = np.concatenate(strides).astype(np.float32)

    def __call__(self, img, req=None):
        np = self.np
        h, w = img.shape[:2]
        animals = []
        for species, score, (x, y, bw, bh) in self.detect(img):
            if min(bw, bh) < MIN_ANIMAL:
                continue
            emb = self.embedder(self.square(img, x, y, bw, bh))
            if emb is None:
                continue
            x0, y0 = max(0.0, x), max(0.0, y)
            x1, y1 = min(float(w), x + bw), min(float(h), y + bh)
            animals.append(
                {
                    "species": species,
                    "bbox": [round(x0, 2), round(y0, 2), round(x1 - x0, 2), round(y1 - y0, 2)],
                    "score": round(score, 4),
                    "emb": emb,
                }
            )
        animals.sort(key=lambda a: -a["score"])
        return animals

    def detect(self, img):
        """[(species, score, (x, y, w, h))] in pixels of `img`, best first."""
        np, cv2 = self.np, self.cv2
        h, w = img.shape[:2]
        r = min(YOLOX_SIZE / h, YOLOX_SIZE / w)
        nh, nw = max(1, int(h * r)), max(1, int(w * r))
        canvas = np.full((YOLOX_SIZE, YOLOX_SIZE, 3), 114, dtype=np.uint8)
        canvas[:nh, :nw] = cv2.resize(img, (nw, nh), interpolation=cv2.INTER_LINEAR)
        self.net.setInput(np.ascontiguousarray(canvas.astype(np.float32).transpose(2, 0, 1)[None]))
        out = self.net.forward()[0]
        xy = (out[:, :2] + self.grids) * self.strides
        wh = np.exp(out[:, 2:4]) * self.strides
        scores = out[:, 5:] * out[:, 4:5]
        # A cat or a dog only where that is the best of all 80 classes: a
        # teddy bear that scores a little as a cat stays a teddy bear.
        best = scores.argmax(1)
        ours = np.isin(best, list(COCO_ANIMALS))
        score = np.where(ours, scores[np.arange(len(best)), best], 0.0)
        keep = score >= ANIMAL_SCORE
        if not keep.any():
            return []
        boxes = np.concatenate([xy[keep] - wh[keep] / 2, wh[keep]], 1) / r
        kept_score, kept_class = score[keep], best[keep]
        picked = cv2.dnn.NMSBoxes(boxes.tolist(), kept_score.tolist(), ANIMAL_SCORE, ANIMAL_NMS)
        found = []
        for i in np.array(picked).reshape(-1):
            x, y, bw, bh = (float(v) for v in boxes[i])
            found.append((COCO_ANIMALS[int(kept_class[i])], float(kept_score[i]), (x, y, bw, bh)))
        return found

    def square(self, img, x, y, bw, bh):
        """The box, a little enlarged, padded to a square with grey and
        scaled to the embedder's input size."""
        np, cv2 = self.np, self.cv2
        h, w = img.shape[:2]
        pad = ANIMAL_PAD * max(bw, bh)
        x0, y0 = max(0, int(x - pad)), max(0, int(y - pad))
        x1, y1 = min(w, int(round(x + bw + pad))), min(h, int(round(y + bh + pad)))
        crop = img[y0:y1, x0:x1]
        ch, cw = crop.shape[:2]
        side = max(ch, cw)
        canvas = np.full((side, side, 3), 114, dtype=np.uint8)
        top, left = (side - ch) // 2, (side - cw) // 2
        canvas[top : top + ch, left : left + cw] = crop
        size = self.embedder.spec["size"]
        return cv2.resize(canvas, (size, size), interpolation=cv2.INTER_AREA if side > size else cv2.INTER_CUBIC)


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
    # The animal models are big and the drive may be slow: only loaded when
    # the core asks for animals (`recognizer.py --animals`).
    animals = Animals(cv2, np) if "--animals" in sys.argv[1:] else None
    if animals is not None:
        tasks["animals"] = animals
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
    if animals is not None:
        hello["tasks"]["animals"] = {"model": animals.model, "dim": animals.dim}
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
