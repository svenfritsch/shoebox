#!/usr/bin/env python3
"""Turn the full-size PNG screenshots in SRC into the 1800 px JPEGs the guide
shows, in DST (other files, such as the dot boxes ui-overview.json, are copied
as they are).  usage: shrink-shots.py SRC DST"""
import os
import shutil
import sys
from concurrent.futures import ProcessPoolExecutor
from PIL import Image


def shrink(src, dst):
    im = Image.open(src).convert("RGB")
    if im.width > 1800:
        im = im.resize((1800, round(im.height * 1800 / im.width)), Image.LANCZOS)
    im.save(dst, quality=86, optimize=True)


def main():
    src, dst = sys.argv[1], sys.argv[2]
    os.makedirs(dst, exist_ok=True)
    jobs = []
    with ProcessPoolExecutor() as pool:
        for f in sorted(os.listdir(src)):
            if f.endswith(".png"):
                jobs.append(pool.submit(shrink, os.path.join(src, f), os.path.join(dst, f[:-4] + ".jpg")))
            else:
                shutil.copy(os.path.join(src, f), os.path.join(dst, f))
        for j in jobs:
            j.result()  # re-raises a failure


if __name__ == "__main__":
    main()
