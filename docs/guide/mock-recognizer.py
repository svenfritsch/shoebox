#!/usr/bin/env python3
"""A stand-in recognizer for the guide's screenshots (docs/protocol.md).
It does not look at faces: it recognises the comic sample pictures by a small
fingerprint and answers with the boxes and identities that make-sample-library.py
wrote to the file named by $SHOEBOX_MOCK_ANN. Any other picture has no faces."""
import base64, io, json, os, sys, zlib
import numpy as np
from PIL import Image

pets_on = "--pets" in sys.argv
ann = json.load(open(os.environ["SHOEBOX_MOCK_ANN"]))
FACE_DIM, PET_DIM = 128, 64


def fingerprint(im):
    return np.asarray(im.convert("L").resize((16, 16), Image.BILINEAR), dtype=float).ravel()


def vec(who, dim, salt, spread=0.3):
    base = np.random.default_rng(zlib.crc32(who.encode())).normal(size=dim)
    base /= np.linalg.norm(base)
    noise = np.random.default_rng(salt).normal(size=dim) / np.sqrt(dim)
    v = base + spread * noise
    return base64.b64encode((v / np.linalg.norm(v)).astype("<f4").tobytes()).decode()


def match(im):
    fp = fingerprint(im)
    best = min(ann, key=lambda a: np.abs(np.array(a["fp"]) - fp).mean())
    return best if np.abs(np.array(best["fp"]) - fp).mean() < 6 else None


hello = {"hello": "shoebox-recognizer", "protocol": 2, "version": "mock",
         "tasks": {"faces": {"model": "mock-faces-1", "dim": FACE_DIM}}}
if pets_on:
    hello["tasks"]["pets"] = {"model": "mock-pets-1", "dim": PET_DIM}
print(json.dumps(hello), flush=True)

for line in sys.stdin:
    req = json.loads(line)
    im = Image.open(io.BytesIO(base64.b64decode(req["image"])))
    a = match(im)
    salt = zlib.crc32(np.asarray(im.resize((8, 8))).tobytes())
    reply = {"id": req["id"], "width": im.width, "height": im.height}
    if "faces" in req["tasks"]:
        reply["faces"] = []
        for f in (a or {}).get("faces", []):
            x, y, w, h = f["bbox"]
            lm = [[x + .3 * w, y + .4 * h], [x + .7 * w, y + .4 * h], [x + .5 * w, y + .6 * h], [x + .35 * w, y + .78 * h], [x + .65 * w, y + .78 * h]]
            reply["faces"].append({"bbox": f["bbox"], "score": 0.96, "landmarks": lm, "emb": vec(f["who"], FACE_DIM, salt)})
    if "pets" in req["tasks"]:
        reply["pets"] = [{"species": p["species"], "bbox": p["bbox"], "score": 0.93, "emb": vec(p["who"], PET_DIM, salt, 0.12)}
                         for p in (a or {}).get("pets", [])]
    print(json.dumps(reply), flush=True)
