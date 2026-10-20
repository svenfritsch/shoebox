#!/usr/bin/env python3
"""A library with faces for the browser tests against the real server
(faces-e2e.mjs). Plain PNG pictures that the fake recognizer
(core/src/bin/shoebox-fake-recognizer.rs) turns into faces: one face in the
middle of each picture, the person given by the picture's colour.

    make-faces.py <folder>

What `shoebox recognize` with the fake makes of it: seven folders `c01` ...
`c07`, each one cluster of 3 to 6 faces of one person, nobody alike enough to
anybody else to be suggested. The mocked tests (faces-mocked.mjs) need no
library.

Needs Pillow.
"""
import itertools
import math
import os
import sys

from PIL import Image

out = sys.argv[1]


# The fake's person of a colour: each channel rounded to steps of 32, then
# k = q0 + 2 q1 + 3 q2 + 1, and the face is the vector sin((i + 1) k) of 128
# numbers. These k are at most 0.22 alike pairwise (below the 0.35 that makes
# even a "maybe"), so each one is somebody else.
def emb(k):
    v = [math.sin((i + 1) * k) for i in range(128)]
    n = math.sqrt(sum(x * x for x in v))
    return [x / n for x in v]


def colour(k):
    """A colour of person k: multiples of 32 up to 224, so never one of the
    fake's cues (dark, pure red/green/blue, cyan or yellow edges)."""
    for q2 in range(7, -1, -1):
        for q1 in range(7, -1, -1):
            q0 = k - 1 - 2 * q1 - 3 * q2
            if 1 <= q0 <= 7 or (q0 == 0 and q1 + q2 > 0):
                return (q0 * 32, q1 * 32, q2 * 32)
    raise ValueError(k)


KS = list(range(2, 9))  # one person per folder
for a, b in itertools.combinations(KS, 2):
    assert abs(sum(x * y for x, y in zip(emb(a), emb(b)))) < 0.25, (a, b)
assert len({colour(k) for k in KS}) == len(KS)

n = 0


def save(folder, im):
    """Saved with one pixel of its own, so no two files are the same."""
    global n
    n += 1
    im.putpixel((im.width // 2, im.height // 2), (n % 256, n // 256, 7))
    path = os.path.join(out, folder)
    os.makedirs(path, exist_ok=True)
    im.save(os.path.join(path, f"IMG_{n:04d}.png"))


for c, k in enumerate(KS):
    for _ in range(3 + c % 4):
        save(f"c{c + 1:02d}", Image.new("RGB", (200, 120), colour(k)))  # a face 100 px wide
