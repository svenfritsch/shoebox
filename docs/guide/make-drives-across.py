#!/usr/bin/env python3
"""A second drive for the screenshots of the duplicates across drives.
usage: make-drives-across.py ORIGINAL TRAVEL [en|de]

Only in CI, from screenshots.sh. ORIGINAL is the sample library; TRAVEL gets, with the
dates kept: the hike folder under the same name (a folder that is on both drives
on purpose), the summer trip under another name, two birthday photos in a
Downloads folder, and photos of its own (so the drive is not taken for a
backup)."""
import os, platform, random, shutil, sys
import numpy as np
from PIL import Image, ImageFilter

# It writes into the folder it is given: only ever the sample drive of screenshots.sh, only in the pipeline.
if os.environ.get("GITHUB_ACTIONS") != "true" or platform.system() == "Darwin":
    sys.exit("make-drives-across.py runs only in CI, from screenshots.sh")
orig, travel = sys.argv[1], sys.argv[2]
de = len(sys.argv) > 3 and sys.argv[3] == "de"
HIKE, SUMMER, BIRTHDAY = ("2024-09 Wanderung", "2024-06 Sommerurlaub", "Familie/Geburtstag") if de else ("2024-09 Hike", "2024-06 Summer trip", "Family/Birthday")
GREECE, ALPS = ("Griechenland 2024", "2025-08 Alpen") if de else ("Greece 2024", "2025-08 Alps")

def copy(rel, to):
    dst = os.path.join(travel, to)
    os.makedirs(dst, exist_ok=True)
    shutil.copy2(os.path.join(orig, rel), os.path.join(dst, os.path.basename(rel)))

def photos(folder):
    return sorted(f for f in os.listdir(os.path.join(orig, folder)) if f.lower().endswith(".jpg"))

for f in photos(HIKE):
    copy(os.path.join(HIKE, f), HIKE)
for f in photos(SUMMER)[:4]:
    copy(os.path.join(SUMMER, f), GREECE)
for f in photos(BIRTHDAY)[:2]:
    copy(os.path.join(BIRTHDAY, f), "Downloads")

# Photos only this drive has: soft hills in a few colours, dated in August 2025.
os.makedirs(os.path.join(travel, ALPS), exist_ok=True)
xx = np.linspace(0, 1, 1200)[None, :]
yy = np.linspace(0, 1, 800)[:, None]
for i in range(12):
    rnd = random.Random(500 + i)
    sky = np.array([(150 + 7 * i) % 256, 190, 235 - 6 * i], float)
    img = np.broadcast_to(sky * (1 - .5 * yy[..., None]), (800, 1200, 3)).copy()
    for k in range(3):
        ridge = .45 + .12 * k + .06 * np.sin(xx * rnd.uniform(2, 6) * np.pi + rnd.uniform(0, 6))
        img = np.where((yy > ridge)[..., None], np.array([70 - 15 * k, 110 - 20 * k, 70 - 10 * k], float), img)
    im = Image.fromarray(np.clip(img + np.random.default_rng(i).normal(0, 3, img.shape), 0, 255).astype("uint8")).filter(ImageFilter.GaussianBlur(.8))
    date = f"2025:08:{3 + i:02d} 11:{10 + i}:00"
    ex = Image.Exif()
    ex[0x0110] = "Sample Camera"
    ex.get_ifd(0x8769)[0x9003] = date
    ex.get_ifd(0x8769)[0x9004] = date
    ex[0x0132] = date
    path = os.path.join(travel, ALPS, f"IMG_{2000 + i}.jpg")
    im.save(path, quality=88, exif=ex)
    t = (2025 - 1970) * 365.25 * 86400 + (7 * 30 + 3 + i) * 86400 + 11 * 3600
    os.utime(path, (t, t))
print("5 + 4 + 2 photos also on the original, 12 of its own")
