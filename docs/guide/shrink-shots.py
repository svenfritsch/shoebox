#!/usr/bin/env python3
"""Turn the full-size PNG screenshots in assets/ into 1800 px JPEGs (what the
guide needs, and light enough to keep in git)."""
import os
from PIL import Image
base = os.path.join(os.path.dirname(os.path.abspath(__file__)), "assets")
for lang in sorted(os.listdir(base)):
    d = os.path.join(base, lang)
    for f in sorted(os.listdir(d)):
        if not f.endswith(".png"):
            continue
        im = Image.open(os.path.join(d, f)).convert("RGB")
        if im.width > 1800:
            im = im.resize((1800, round(im.height * 1800 / im.width)), Image.LANCZOS)
        im.save(os.path.join(d, f[:-4] + ".jpg"), quality=86, optimize=True)
        os.remove(os.path.join(d, f))
