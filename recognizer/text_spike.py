#!/usr/bin/env python3
"""Throw-away measurement for phase 9 (text in photos). Not part of the app.

Reads a folder of COPIES of photos, runs PP-OCR on each (through the
`rapidocr_onnxruntime` package, which carries the PP-OCRv4 models) and writes a
report: seconds per photo, the text found, hits and misses against
`expected.txt`, and false text in photos that have none. It only reads the
folder; the report goes to the current directory.

    python3 -m venv spike-venv
    spike-venv/bin/pip install rapidocr-onnxruntime pillow     # pillow-heif for HEIC
    spike-venv/bin/python recognizer/text_spike.py ~/spike-photos --make-template
    # fill in ~/spike-photos/expected.txt, then:
    spike-venv/bin/python recognizer/text_spike.py ~/spike-photos

expected.txt, one photo per line, `file name: word, word`:

    # lines starting with # are ignored
    letter-stadtwerke.jpg: Rechnung, Stadtwerke
    street-sign.jpg: Hauptstraße
    menu.jpg: Schnitzel, Pommes

A photo that is not listed, or listed with nothing after the colon, is a photo
WITHOUT readable text: any text found there counts as false text.
"""
import argparse
import os
import platform
import sys
import time
import unicodedata

EXTS = {".jpg", ".jpeg", ".png", ".heic", ".heif", ".tif", ".tiff", ".webp", ".bmp"}
THRESHOLDS = (0.5, 0.6, 0.7, 0.8, 0.9)
LONG_EDGE = 1600  # what the core would send the worker


import re
# Only German and English characters are kept (see docs/plan.md, phase 9).
_NOT_ALLOWED = re.compile("[^A-Za-z0-9ÄÖÜäöüß\\s.,;:!?'\"„“”‘’«»()\\[\\]{}<>/\\\\|@#&%*+=~^_€§°²³µ$£–—·•-]")


def keep_line(text):
    """The line without characters outside the allow-list, or None if fewer than
    two letters or digits are left."""
    t = _NOT_ALLOWED.sub("", text).strip()
    return t if len(re.findall("[A-Za-z0-9ÄÖÜäöüß]", t)) >= 2 else None


def fold(s):
    """Lower case, no accents, ß as ss, only letters and digits (as the index will do)."""
    s = unicodedata.normalize("NFC", s).casefold().replace("ß", "ss")
    s = unicodedata.normalize("NFD", s)
    s = "".join(c for c in s if not unicodedata.combining(c))
    return "".join(c if c.isalnum() else " " for c in s)


def squash(s):
    return "".join(fold(s).split())


def read_expected(path):
    out = {}
    if not os.path.exists(path):
        return out
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#") or ":" not in line:
                continue
            name, words = line.split(":", 1)
            out[unicodedata.normalize("NFC", name.strip())] = [w.strip() for w in words.split(",") if w.strip()]
    return out


def photos(folder):
    names = [n for n in sorted(os.listdir(folder))
             if os.path.splitext(n)[1].lower() in EXTS and not n.startswith("._")]
    return names


def load(path):
    from PIL import Image, ImageOps
    if path.lower().endswith((".heic", ".heif")):
        try:
            import pillow_heif
            pillow_heif.register_heif_opener()
        except ImportError:
            raise RuntimeError("HEIC needs: pip install pillow-heif (or convert the copy to JPEG)")
    im = Image.open(path)
    im = ImageOps.exif_transpose(im).convert("RGB")
    if max(im.size) > LONG_EDGE:
        k = LONG_EDGE / max(im.size)
        im = im.resize((round(im.width * k), round(im.height * k)), Image.LANCZOS)
    return im


def make_template(folder):
    path = os.path.join(folder, "expected.txt")
    if os.path.exists(path):
        sys.exit(f"{path} exists already; not overwritten")
    with open(path, "w", encoding="utf-8") as f:
        f.write("# file name: one or two words you can really read in the photo, separated by commas.\n"
                "# Leave nothing after the colon (or delete the line) for a photo WITHOUT text.\n\n")
        for n in photos(folder):
            f.write(f"{n}:\n")
    print(f"wrote {path}: fill in the words for the photos that have text")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("folder")
    ap.add_argument("--expected", help="default: <folder>/expected.txt")
    ap.add_argument("--report", default="text_spike_report.txt")
    ap.add_argument("--make-template", action="store_true", help="write expected.txt with every file name, then stop")
    a = ap.parse_args()
    folder = os.path.abspath(a.folder)
    if a.make_template:
        return make_template(folder)

    try:
        import numpy as np
        from rapidocr_onnxruntime import RapidOCR
        import onnxruntime
    except ImportError as e:
        sys.exit(f"missing package ({e}); pip install rapidocr-onnxruntime pillow")

    expected = read_expected(a.expected or os.path.join(folder, "expected.txt"))
    names = photos(folder)
    if not names:
        sys.exit("no photos in the folder")
    unknown = [n for n in expected if unicodedata.normalize("NFC", n) not in {unicodedata.normalize("NFC", x) for x in names}]
    engine = RapidOCR()
    # warm-up so the first photo does not carry the model loading time
    engine(np.full((64, 64, 3), 255, np.uint8))

    rows = []  # (name, seconds, [(text, score)], error)
    for n in names:
        try:
            im = load(os.path.join(folder, n))
        except Exception as e:  # keep going: one bad file must not end the run
            rows.append((n, 0.0, [], str(e)))
            continue
        t0 = time.perf_counter()
        result, _ = engine(np.asarray(im)[:, :, ::-1])  # RapidOCR takes BGR
        dt = time.perf_counter() - t0
        lines = [(k, float(r[2])) for r in (result or []) for k in [keep_line(r[1])] if k]
        rows.append((n, dt, lines, None))

    out = []
    p = out.append
    p(f"machine: {platform.platform()} {platform.machine()} / python {platform.python_version()} / onnxruntime {onnxruntime.__version__}")
    p(f"photos: {len(names)}, with expected text: {sum(1 for n in names if expected.get(unicodedata.normalize('NFC', n)))}, long edge {LONG_EDGE}px")
    if unknown:
        p(f"WARNING: expected.txt names files that are not in the folder: {', '.join(unknown)}")
    p("")
    p("== per photo (seconds, lines found, text at confidence >= 0.5) ==")
    for n, dt, lines, err in rows:
        if err:
            p(f"{n}: ERROR {err}")
            continue
        want = expected.get(unicodedata.normalize("NFC", n), [])
        shown = " | ".join(f"{t} ({s:.2f})" for t, s in lines if s >= 0.5)
        p(f"{n}: {dt:.2f}s, {len(lines)} lines{', expects ' + ', '.join(want) if want else ''}\n    {shown}")

    ok = [r for r in rows if not r[3]]
    times = sorted(r[1] for r in ok)
    p("")
    p("== speed ==")
    p(f"mean {sum(times) / len(times):.2f}s, median {times[len(times) // 2]:.2f}s, slowest {times[-1]:.2f}s per photo "
      f"(estimate for 50,000 photos: {sum(times) / len(times) * 50000 / 3600:.1f} h)")
    with_text = [r for r in ok if expected.get(unicodedata.normalize("NFC", r[0]))]
    without = [r for r in ok if not expected.get(unicodedata.normalize("NFC", r[0]))]
    if with_text:
        p(f"photos with text: mean {sum(r[1] for r in with_text) / len(with_text):.2f}s")
    if without:
        p(f"photos without text: mean {sum(r[1] for r in without) / len(without):.2f}s")

    p("")
    p("== quality by confidence threshold ==")
    p("threshold | words found / expected | photos without text that still show text (lines)")
    for thr in THRESHOLDS:
        found = total = 0
        for n, _, lines, _ in with_text:
            blob = squash("".join(t for t, s in lines if s >= thr))
            for w in expected[unicodedata.normalize("NFC", n)]:
                total += 1
                found += squash(w) in blob
        fp_photos = sum(1 for r in without if any(s >= thr for _, s in r[2]))
        fp_lines = sum(1 for r in without for _, s in r[2] if s >= thr)
        p(f"{thr:.1f}       | {found}/{total}" + (f" ({100 * found / total:.0f}%)" if total else "")
          + f"                 | {fp_photos}/{len(without)} photos ({fp_lines} lines)")

    p("")
    p("== words not found at 0.5 (check the text above: wrong letters, or really not read?) ==")
    for n, _, lines, _ in with_text:
        blob = squash("".join(t for t, s in lines if s >= 0.5))
        miss = [w for w in expected[unicodedata.normalize("NFC", n)] if squash(w) not in blob]
        if miss:
            p(f"{n}: {', '.join(miss)}")

    p("")
    p("== what it would store (rows) ==")
    kept = [(t, s) for r in ok for t, s in r[2] if s >= 0.7]
    chars = sum(len(t) for t, _ in kept)
    p(f"{len(kept)} lines at >= 0.7 over {len(ok)} photos, {chars} characters; "
      f"about {(len(kept) * 110 + chars * 4) / 1024:.0f} KB here, scaled to 50,000 photos about "
      f"{(len(kept) * 110 + chars * 4) * 50000 / len(ok) / 1048576:.0f} MB")

    text = "\n".join(out) + "\n"
    with open(a.report, "w", encoding="utf-8") as f:
        f.write(text)
    print(text)
    print(f"report written to {os.path.abspath(a.report)}")


if __name__ == "__main__":
    main()
