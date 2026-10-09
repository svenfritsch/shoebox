#!/usr/bin/env python3
"""Write THIRD-PARTY-LICENSES.txt: the notices for everything that ends up in
the shoebox binary or the archive.

  python3 scripts/third-party-licenses.py            # rewrite the file
  python3 scripts/third-party-licenses.py --check    # fail if it is out of date

Sources: the Rust crates the binary is built from (cargo metadata, normal
dependencies only; the licence files come from the downloaded crates, so run
`cargo fetch` first), the web libraries in core/web/vendor/, and the two
native libraries built by scripts/build-deps.sh (libheif, libde265; their
LGPL text is taken from the downloaded sources when a build exists, else from
the committed file).
"""
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "THIRD-PARTY-LICENSES.txt"
LGPL = ROOT / "scripts" / "licenses" / "LGPL-3.0.txt"


TARGETS = ["x86_64-apple-darwin", "aarch64-apple-darwin"]  # what the release ships


def crates():
    """The crates built into the macOS binaries (normal dependencies only)."""
    found = {}
    for target in TARGETS:
        meta = json.loads(subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target,
             "--manifest-path", str(ROOT / "core" / "Cargo.toml")],
            check=True, capture_output=True, text=True).stdout)
        by_id = {p["id"]: p for p in meta["packages"]}
        nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
        root = meta["resolve"]["root"]
        seen, todo = set(), [root]
        while todo:
            i = todo.pop()
            if i in seen:
                continue
            seen.add(i)
            for d in nodes[i]["deps"]:
                if any(k["kind"] is None for k in d["dep_kinds"]):
                    todo.append(d["pkg"])
        seen.discard(root)
        for i in seen:
            found[i] = by_id[i]
    return sorted(found.values(), key=lambda p: (p["name"], p["version"]))


def license_files(pkg):
    d = Path(pkg["manifest_path"]).parent
    names = [f for f in sorted(os.listdir(d)) if re.match(r"(LICENSE|LICENCE|COPYING|UNLICENSE|NOTICE)", f, re.I) and (d / f).is_file()]
    return [(f, (d / f).read_text(encoding="utf-8", errors="replace").strip()) for f in names]


def main():
    check = "--check" in sys.argv
    groups = {}  # text -> [(label, filename)]
    for p in crates():
        files = license_files(p)
        label = f'{p["name"]} {p["version"]} ({p["license"] or "see its repository"})'
        if not files:
            groups.setdefault(f"(no licence file in the crate; licence: {p['license']}, source: {p['repository']})", []).append(label)
        for name, text in files:
            groups.setdefault(text, []).append(label)

    out = []
    w = out.append
    w("THIRD-PARTY LICENSES\n====================\n")
    w("shoebox (source: github.com/svenfritsch/shoebox) contains and uses\n"
      "the software below, each under its own licence. Where a crate offers a choice of\n"
      "licences (\"MIT OR Apache-2.0\"), you may use it under either; the texts that\n"
      "the authors ship are reproduced in part 3.\n")
    w("\n1. Web libraries (embedded in the photo app, core/web/vendor/)\n" + "-" * 62 + "\n")
    for name, ver, lic, path in [("Leaflet", "1.9.4", "BSD-2-Clause", "core/web/vendor/leaflet/LICENSE"),
                                 ("Leaflet.markercluster", "1.5.3", "MIT", "core/web/vendor/markercluster/LICENSE")]:
        w(f"{name} {ver}, {lic}\n\n{(ROOT / path).read_text().strip()}\n\n")
    w("Map pictures shown by the optional maps are OpenStreetMap tiles. Map data (c) OpenStreetMap\n"
      "contributors, available under the Open Database Licence (https://www.openstreetmap.org/copyright).\n"
      "shoebox does not store or ship any of it.\n")
    w("\n2. Native libraries (linked statically into the binary)\n" + "-" * 56 + "\n")
    versions = dict(re.findall(r"^(LIBDE265|LIBHEIF)_VERSION=(\S+)", (ROOT / "scripts" / "build-deps.sh").read_text(), re.M))
    w(f"libheif {versions['LIBHEIF']} (https://github.com/strukturag/libheif) and\n"
      f"libde265 {versions['LIBDE265']} (https://github.com/strukturag/libde265) read HEIC photos.\n"
      "Both are licensed under the GNU Lesser General Public License, version 3 (text below).\n"
      "They are built from the unmodified upstream source releases by scripts/build-deps.sh and\n"
      "linked statically. As the LGPL requires, you can replace them with another version:\n"
      "the complete source of shoebox and that script are at github.com/svenfritsch/shoebox;\n"
      "change the versions in the script and run scripts/build.sh to get a binary with your own\n"
      "build of the libraries.\n"
      "Copyright (c) 2013-2026 Struktur AG and Dirk Farin and the other contributors of the projects.\n\n")
    w(LGPL.read_text().strip() + "\n")
    w("\n3. Rust crates (linked into the binary)\n" + "-" * 40 + "\n")
    w("The Rust standard library and compiler runtime are used under MIT OR Apache-2.0.\n\n")
    for text, labels in sorted(groups.items(), key=lambda kv: kv[1][0]):
        w("=" * 78 + "\n" + "\n".join(labels) + "\n" + "-" * 78 + "\n" + text + "\n\n")
    w("\n4. Downloaded on request, not part of the archive\n" + "-" * 51 + "\n")
    w("The optional recognizer (recognizer/install.sh, fetch-models.sh) downloads Python\n"
      "(python-build-standalone), OpenCV and the models from the OpenCV Model Zoo\n"
      "(github.com/opencv/opencv_zoo) onto your drive. Each comes under its own licence,\n"
      "which you find with the download (the model folders of opencv_zoo carry a LICENSE).\n")
    text = "\n".join(out).replace("\r\n", "\n")
    text = re.sub(r"\n{4,}", "\n\n\n", text) + "\n"
    if check:
        if not OUT.exists() or OUT.read_text() != text:
            sys.exit("THIRD-PARTY-LICENSES.txt is out of date: python3 scripts/third-party-licenses.py")
        print("licenses ok")
        return
    OUT.write_text(text)
    print(f"wrote {OUT} ({len(text) // 1024} KB, {sum(len(v) for v in groups.values())} licence texts for {len(crates())} crates)")


main()
