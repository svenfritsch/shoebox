#!/usr/bin/env python3
"""Generate a small sample library (synthetic landscapes, no real people) for
the guide's screenshots.  usage: make-sample-library.py OUTDIR [en|de] [annotations.json]"""
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

# Comic pictures (a girl, Mia, and a cat, Whiskers) for the people and pets
# screenshots, and the annotations the mock recognizer answers from.
import json
import numpy as np
from comic import scene
COMIC = {
    "2025-05 Garden party": [("park", "2025:05:17 15:20:00"), ("garden", "2025:05:17 16:05:00"), ("close", "2025:05:17 17:40:00")],
    "2025-07 Beach": [("beach", "2025:07:12 11:30:00"), ("sofa", "2025:07:20 19:10:00"), ("cat2", "2025:07:21 09:00:00")],
}
# More comic people for the face-tagging screenshots: a family, friends and
# colleagues (three each), a dog with the cat, two strangers and a framed
# picture that only looks like a face. They are older than the others so the
# timeline's first photos stay the same.
COMIC.update({
    "2023-06 Dog walk": [("pets:park:0", "2023:06:10 10:00:00"), ("pets:garden:1", "2023:06:10 11:30:00"), ("pets:beach:2", "2023:06:11 16:00:00")],
    "2023-08 Family picnic": [("grp:garden:mia,rosa,ben:0", "2023:08:05 13:00:00"), ("grp:park:mia,rosa,ben:1", "2023:08:05 15:10:00"), ("grp:indoor:mia,rosa,ben:2", "2023:08:06 18:20:00")],
    "2023-09 Friends weekend": [("grp:park:lena,jonas,sam:0", "2023:09:16 12:00:00"), ("grp:beach:lena,jonas,sam:1", "2023:09:16 15:30:00"), ("grp:garden:lena,jonas,sam:2", "2023:09:17 11:15:00")],
    "2023-10 Team day": [("grp:indoor:priya,marco,chen:0", "2023:10:12 09:30:00"), ("grp:park:priya,marco,chen:1", "2023:10:12 13:00:00"), ("grp:garden:priya,marco,chen:2", "2023:10:12 16:45:00")],
    "2023-11 City walk": [("strangers:park", "2023:11:04 11:00:00"), ("strangers:beach", "2023:11:04 14:20:00"), ("poster", "2023:11:04 17:00:00")],
})
DE_COMIC = {"2025-05 Garden party": "2025-05 Gartenfest", "2025-07 Beach": "2025-07 Strand",
            "2023-06 Dog walk": "2023-06 Hundespaziergang", "2023-08 Family picnic": "2023-08 Familienpicknick",
            "2023-09 Friends weekend": "2023-09 Freundewochenende", "2023-10 Team day": "2023-10 Teamtag",
            "2023-11 City walk": "2023-11 Stadtbummel"}
annotations = []
for folder, shots in COMIC.items():
    folder = DE_COMIC[folder] if lang == "de" else folder
    os.makedirs(os.path.join(out, folder), exist_ok=True)
    for name, date in shots:
        n += 1
        im, faces, pets = scene(name)
        ex = Image.Exif()
        ex[0x0110] = "Sample Camera"
        ex.get_ifd(0x8769)[0x9003] = date
        ex.get_ifd(0x8769)[0x9004] = date
        ex[0x0132] = date
        path = os.path.join(out, folder, f"IMG_{1000 + n}.jpg")
        im.save(path, quality=88, exif=ex)
        fp = np.asarray(Image.open(path).convert("L").resize((16, 16), Image.BILINEAR), dtype=float).ravel()
        annotations.append({"fp": fp.tolist(),
                            "faces": [{"who": w, "bbox": b} for w, b, _ in faces],
                            "pets": [{"who": w, "species": "dog" if w == "buddy" else "cat", "bbox": b} for w, b, _ in pets]})
if len(sys.argv) > 3:
    json.dump(annotations, open(sys.argv[3], "w"))

# Duplicates for the guide: an exact copy and a messenger-sized copy.
src = os.path.join(out, first, "IMG_1001.jpg")
os.makedirs(os.path.join(out, "Messenger"), exist_ok=True)
import shutil
shutil.copy2(src, os.path.join(out, "Messenger", "IMG_1001 copy.jpg"))
small = os.path.join(out, "Messenger", "WhatsApp Image.jpg")
small_im = Image.open(src).resize((640, 427))
small_im.save(small, quality=70, exif=Image.open(src).getexif())
import datetime
t = datetime.datetime(2024, 6, 14, 6, 40).timestamp()
os.utime(small, (t, t))
print(n, "photos in", out)
