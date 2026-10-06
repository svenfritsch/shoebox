#!/usr/bin/env python3
"""Generate a small sample library (synthetic landscapes, no real people) for
the guide's screenshots.  usage: make-sample-library.py OUTDIR [en|de]"""
import math, os, random, sys
import numpy as np
from PIL import Image, ImageFilter

out = sys.argv[1]
lang = sys.argv[2] if len(sys.argv) > 2 else "en"
DE = {"Summer trip": "Sommerurlaub", "Hike": "Wanderung", "Winter": "Winter", "Family/Birthday": "Familie/Geburtstag", "Messenger": "Messenger"}
W, H = 1600, 1067

PALETTES = {
    "dawn":   [(255, 183, 120), (240, 120, 120), (80, 70, 130)],
    "noon":   [(110, 180, 240), (190, 225, 250), (240, 245, 250)],
    "dusk":   [(40, 40, 110), (210, 100, 110), (250, 180, 110)],
    "mist":   [(170, 190, 200), (210, 220, 225), (235, 238, 238)],
    "storm":  [(70, 80, 100), (120, 130, 150), (180, 185, 195)],
}

def scene(seed, pal, ground, sun=True, portrait=False):
    rnd = random.Random(seed)
    w, h = (H, W) if portrait else (W, H)
    y = np.linspace(0, 1, h)[:, None, None]
    p = [np.array(c, float) for c in PALETTES[pal]]
    sky = np.where(y < .5, p[0] + (p[1] - p[0]) * (y / .5), p[1] + (p[2] - p[1]) * ((y - .5) / .5))
    img = np.broadcast_to(sky, (h, w, 3)).copy()
    xx = np.arange(w)[None, :]
    yy = np.arange(h)[:, None]
    if sun:
        cx, cy, r = rnd.uniform(.2, .8) * w, rnd.uniform(.2, .35) * h, h * .06
        d = np.sqrt((xx - cx) ** 2 + (yy - cy) ** 2)
        glow = np.clip(1 - d / (r * 6), 0, 1) ** 2
        img += glow[..., None] * 90
        img[d < r] = (255, 245, 215)
    # layered hills
    for i in range(4):
        base = h * (.5 + .1 * i)
        f1, f2 = rnd.uniform(1, 3), rnd.uniform(4, 9)
        ph1, ph2 = rnd.uniform(0, 6), rnd.uniform(0, 6)
        prof = base + h * .07 * np.sin(xx / w * f1 * math.tau + ph1) + h * .02 * np.sin(xx / w * f2 * math.tau + ph2)
        mask = yy > prof
        shade = 1 - .22 * (3 - i) / 3
        col = np.array(ground[i % len(ground)], float) * (.55 + .15 * i)
        img = np.where(mask[..., None], col * shade, img)
    img += np.random.default_rng(seed).normal(0, 4, img.shape)
    im = Image.fromarray(np.clip(img, 0, 255).astype("uint8"))
    return im.filter(ImageFilter.GaussianBlur(.8))

GREEN = [(70, 120, 60), (50, 100, 55), (40, 85, 50), (30, 70, 45)]
SAND = [(214, 180, 120), (190, 150, 100), (160, 125, 85), (120, 95, 70)]
ROCK = [(120, 120, 130), (95, 95, 110), (75, 75, 90), (55, 55, 70)]

LIB = {  # keys: "YYYY-MM Name" or a path
    "2024-06 Summer trip": [("dawn", GREEN, "2024:06:14 06:40:00"), ("noon", SAND, "2024:06:14 12:15:00"),
        ("dusk", SAND, "2024:06:15 20:30:00"), ("noon", GREEN, "2024:06:16 11:05:00"),
        ("mist", ROCK, "2024:06:17 09:45:00"), ("dusk", GREEN, "2024:06:18 21:10:00")],
    "2024-09 Hike": [("mist", ROCK, "2024:09:07 08:20:00"), ("noon", ROCK, "2024:09:07 11:40:00"),
        ("storm", ROCK, "2024:09:07 15:10:00"), ("dawn", ROCK, "2024:09:08 07:05:00"), ("noon", GREEN, "2024:09:08 10:30:00")],
    "2025-01 Winter": [("mist", ROCK, "2025:01:03 10:00:00"), ("storm", GREEN, "2025:01:04 14:20:00"),
        ("dusk", ROCK, "2025:01:05 17:00:00"), ("noon", ROCK, "2025:01:06 12:00:00")],
    "Family/Birthday": [("noon", GREEN, "2025:03:22 14:00:00"), ("dawn", SAND, "2025:03:22 16:30:00"),
        ("dusk", GREEN, "2025:03:22 19:00:00")],
}

n = 0
first = None
for folder, shots in LIB.items():
    if lang == "de":
        head, _, name = folder.partition(" ")
        folder = DE[folder] if name == "" else head + " " + DE[name]
    first = first or folder
    os.makedirs(os.path.join(out, folder), exist_ok=True)
    for i, (pal, ground, date) in enumerate(shots):
        n += 1
        im = scene(n, pal, ground, sun=pal not in ("storm", "mist"), portrait=(n % 7 == 0))
        ex = Image.Exif()
        ex[0x0110] = "Sample Camera"
        ex.get_ifd(0x8769)[0x9003] = date
        ex.get_ifd(0x8769)[0x9004] = date
        ex[0x0132] = date
        im.save(os.path.join(out, folder, f"IMG_{1000 + n}.jpg"), quality=88, exif=ex)

# Duplicates for the guide: an exact copy and a messenger-sized copy.
src = os.path.join(out, first, "IMG_1001.jpg")
os.makedirs(os.path.join(out, "Messenger"), exist_ok=True)
import shutil
shutil.copy2(src, os.path.join(out, "Messenger", "IMG_1001 copy.jpg"))
small = os.path.join(out, "Messenger", "WhatsApp Image.jpg")
Image.open(src).resize((640, 427)).save(small, quality=70)  # no EXIF, as after a messenger
import datetime
t = datetime.datetime(2024, 6, 14, 6, 40).timestamp()
os.utime(small, (t, t))
print(n, "photos in", out)
