#!/usr/bin/env python3
"""Checks for the guide (CI runs them in .github/workflows/guide.yml).

Always (no screenshots needed):
  - shoebox-en.html and shoebox-de.html have the same sections, show the same
    screenshots (by name, each from its own language folder) and the same
    numbered dots.
With the screenshots made (docs/guide/screenshots.sh):
  - every image the HTML shows exists, and assets/en and assets/de hold the
    same names;
  - every numbered dot of "The screen at a glance" sits on its control:
    screenshots.mjs records where each control is on ui-overview
    (assets/<lang>/ui-overview.json, in the order of the legend).

    python3 docs/guide/check.py

Standard library only."""
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
LANGS = ("en", "de")
# How far (in CSS pixels of the 1280 px wide screenshot) a dot's centre may be
# from its control's box: the dots sit next to small controls, not on them.
SLACK = 28


def parse(lang):
    html = open(os.path.join(HERE, f"shoebox-{lang}.html"), encoding="utf-8").read()
    sections = re.findall(r'<section[^>]*\bid="([^"]+)"', html)
    images = re.findall(r'<img[^>]*\bsrc="([^"]+)"', html)
    glance = re.search(r'<section[^>]*id="glance".*?</section>', html, re.S)
    dots = [(float(x), float(y)) for x, y in
            re.findall(r'class="dot"[^>]*style="left:([\d.]+)%;\s*top:([\d.]+)%"', glance.group(0) if glance else "")]
    return sections, images, dots


def main():
    bad = []
    # A merge that was committed with its conflict markers still in the file.
    for lang in LANGS:
        name = f"shoebox-{lang}.html"
        for n, line in enumerate(open(os.path.join(HERE, name), encoding="utf-8"), 1):
            if re.match(r"(<<<<<<<|=======$|>>>>>>>)", line):
                bad.append(f"{name}:{n}: merge-conflict marker left in the file")
    parsed = {lang: parse(lang) for lang in LANGS}
    (en_sec, en_img, en_dots), (de_sec, de_img, de_dots) = parsed["en"], parsed["de"]
    if en_sec != de_sec:
        bad.append(f"the sections differ: en {en_sec} / de {de_sec}")
    for lang, (_, images, _) in parsed.items():
        for src in images:
            if not src.startswith(f"assets/{lang}/"):
                bad.append(f"shoebox-{lang}.html shows {src}, not from assets/{lang}/")
    names = {lang: [os.path.basename(s) for s in parsed[lang][1]] for lang in LANGS}
    if names["en"] != names["de"]:
        only = sorted(set(names["en"]) ^ set(names["de"]))
        bad.append("the two languages show different screenshots (or in another order)"
                   + (f": {', '.join(only)}" if only else ""))
    if en_dots != de_dots:
        bad.append("the numbered dots of 'The screen at a glance' differ between en and de")

    assets = os.path.join(HERE, "assets")
    made = os.path.isdir(assets) and any(f.endswith(".jpg") for _, _, fs in os.walk(assets) for f in fs)
    if made:
        for lang, (_, images, dots) in parsed.items():
            for src in dict.fromkeys(images):
                if not os.path.isfile(os.path.join(HERE, src)):
                    bad.append(f"shoebox-{lang}.html: missing image {src} (no step in screenshots.mjs makes it?)")
            boxes = os.path.join(assets, lang, "ui-overview.json")
            if os.path.isfile(boxes):
                bad += check_dots(lang, dots, json.load(open(boxes, encoding="utf-8")))
        have = {lang: sorted(f for f in os.listdir(os.path.join(assets, lang)) if f.endswith(".jpg"))
                for lang in LANGS if os.path.isdir(os.path.join(assets, lang))}
        if len(have) == 2 and have["en"] != have["de"]:
            bad.append("assets/en and assets/de differ in file names")
    else:
        print("no assets/ (screenshots not made): checked text and structure only")

    for b in bad:
        print("error:", b, file=sys.stderr)
    if bad:
        sys.exit(1)
    print("guide ok")


def check_dots(lang, dots, rec):
    """Each dot's centre (in % of the screenshot) against its control's box."""
    bad = []
    w, h, boxes = rec["width"], rec["height"], rec["boxes"]
    if len(dots) != len(boxes):
        return [f"{lang}: {len(dots)} dots on ui-overview, but screenshots.mjs records {len(boxes)} controls"]
    for n, ((dx, dy), b) in enumerate(zip(dots, boxes), 1):
        if b is None:
            bad.append(f"{lang}: dot {n}: its control ({rec['names'][n - 1]}) is not on the screen")
            continue
        x, y = dx / 100 * w, dy / 100 * h
        gap_x = max(b["x"] - x, 0, x - (b["x"] + b["w"]))
        gap_y = max(b["y"] - y, 0, y - (b["y"] + b["h"]))
        if max(gap_x, gap_y) > SLACK:
            cx, cy = (b["x"] + b["w"] / 2) / w * 100, (b["y"] + b["h"] / 2) / h * 100
            bad.append(f"{lang}: dot {n} ({rec['names'][n - 1]}) at left:{dx}%;top:{dy}% is "
                       f"{max(gap_x, gap_y):.0f}px off its control; the control's centre is "
                       f"left:{cx:.1f}%;top:{cy:.1f}%")
    return bad


if __name__ == "__main__":
    main()
