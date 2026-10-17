#!/usr/bin/env python3
"""A small library for the browser tests: plain coloured JPEGs, each with its
number on it (all different, so no duplicates), with capture dates over three
months, so the timeline has several sections and rows.

    make-library.py <folder>

Needs Pillow.
"""
import colorsys
import datetime
import os
import sys

from PIL import Image, ImageDraw

out = sys.argv[1]
start = datetime.datetime(2024, 3, 1, 10, 0)
for n in range(60):
    when = start + datetime.timedelta(days=n * 1.5, minutes=n)
    folder = os.path.join(out, when.strftime("%Y-%m"))
    os.makedirs(folder, exist_ok=True)
    r, g, b = colorsys.hsv_to_rgb(n / 60, 0.5, 0.85)
    im = Image.new("RGB", (480, 360), (int(r * 255), int(g * 255), int(b * 255)))
    ImageDraw.Draw(im).text((20, 20), str(n), fill=(0, 0, 0))
    date = when.strftime("%Y:%m:%d %H:%M:%S")
    ex = Image.Exif()
    ex.get_ifd(0x8769)[0x9003] = date
    ex.get_ifd(0x8769)[0x9004] = date
    path = os.path.join(folder, f"IMG_{1000 + n}.jpg")
    im.save(path, quality=85, exif=ex)
    t = when.timestamp()
    os.utime(path, (t, t))
