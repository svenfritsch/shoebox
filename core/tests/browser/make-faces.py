#!/usr/bin/env python3
"""A library with faces for the browser tests of the Unnamed page and the
people list (unnamed.mjs, people.mjs). Plain PNG pictures that the fake recognizer
(core/src/bin/shoebox-fake-recognizer.rs) turns into faces: one face in the
middle of each picture, the person given by the picture's colour, and with a
white top and a grey bottom a face of that person with a chosen similarity.

    make-faces.py <folder>

What `shoebox recognize` with the fake makes of it (each folder one cluster
of one person, nobody alike enough to anybody else to be suggested):

    c01 ... c18   clusters of 3 to 6 faces, one person each
    look          3 faces of one person
    lookalike     3 faces 0.575 like `look` (one face, three files): a card of
                  its own, suggested for whoever `look` is named as
    singles       10 faces of nobody in particular (clusters of one)
    big           100 faces of one person: the largest card there can be
                  (`clusters::MAX_CLUSTER`; one more and it is split)

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


KS = list(range(2, 23))  # 18 clusters, then look, the singles' person and big
LOOK, SINGLES, BIG = KS[18:]
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


def plain(folder, k):
    """A face of person k, 100 px wide."""
    save(folder, Image.new("RGB", (200, 120), colour(k)))


def variant(folder, k, grey):
    """A face of person k, round(40 grey / 255) / 40 alike (the fake's white-top cue)."""
    im = Image.new("RGB", (200, 200), (255, 255, 255))
    im.paste(colour(k), (0, 50, 200, 150))
    im.paste((grey, grey, grey), (0, 150, 200, 200))
    save(folder, im)


for c, k in enumerate(KS[:18]):
    for _ in range(3 + c % 4):
        plain(f"c{c + 1:02d}", k)
for _ in range(3):
    plain("look", LOOK)
for _ in range(3):
    variant("lookalike", LOOK, 147)
# Ten greys from 0.15 to 0.375 like the person SINGLES (whom no plain picture
# shows): at most about 0.25 alike to each other, so each face is a cluster of
# its own.
for steps in range(6, 16):
    variant("singles", SINGLES, round(steps * 255 / 40))
for _ in range(100):
    plain("big", BIG)
