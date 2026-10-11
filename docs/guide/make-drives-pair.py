#!/usr/bin/env python3
"""A backup of the sample library that has drifted from it, for the screenshots
of Manage drives and the comparison views.  usage: make-drives-pair.py ORIGINAL BACKUP

BACKUP starts as a copy of ORIGINAL (dates kept), then life goes on: a few
photos are new on the original (not on the backup yet), a few were deleted
there (only on the backup) and two were edited on the original's side, so the
backup holds other content at the same path (different on the backup)."""
import os, shutil, sys
from PIL import Image, ImageEnhance

orig, bak = sys.argv[1], sys.argv[2]
shutil.copytree(orig, bak, dirs_exist_ok=True, ignore=shutil.ignore_patterns(".shoebox"))
files = sorted(os.path.relpath(os.path.join(d, f), orig) for d, _, fs in os.walk(orig) if ".shoebox" not in d
               for f in fs if f.lower().endswith((".jpg", ".jpeg")))
pick = lambda *ix: [files[i % len(files)] for i in ix]
for rel in pick(4, 11, 17, 23):      # new on the original, not copied yet
    os.remove(os.path.join(bak, rel))
for rel in pick(7, 14, 29):          # deleted from the original since the backup
    os.remove(os.path.join(orig, rel))
for rel in pick(2, 19):              # edited on the original: other content at the same path
    path = os.path.join(orig, rel)
    st = os.stat(path)
    im = Image.open(path)
    exif = im.info.get("exif")
    edited = ImageEnhance.Color(ImageEnhance.Contrast(im.convert("RGB")).enhance(1.25)).enhance(1.6)
    edited.save(path, quality=88, **({"exif": exif} if exif else {}))
    os.utime(path, (st.st_atime, st.st_mtime))
print(f"{len(files)} photos; 4 not on the backup yet, 3 only on the backup, 2 different")
