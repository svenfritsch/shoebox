#!/usr/bin/env python3
"""Times every read endpoint of the photo app on the large synthetic library
of bench-people.py, and what a request the browser gave up on costs.

    scripts/bench-people.py make <dir> --shoebox <binary>     (once)
    scripts/bench-endpoints.py <dir> --shoebox <binary> [--port 7798]

Prints, per endpoint, the first and the repeated time (the first one reads
what the server keeps afterwards), the size of the answer, and marks every
one that takes more than --slow seconds: those need a spinner on the page and
must be called off when it is left (serve/cancel.rs, `pageApi` in app.js).

Then "called off": a slow request is started and the client closes the
connection after 0.3 s; a cheap request (/api/groups) asked right after must
not wait for the work of the first (`serve/cancel.rs`).
"""

import argparse
import importlib.util
import os
import shutil
import socket
import sys
import time

here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("bench_people", os.path.join(here, "bench-people.py"))
bp = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bp)


def abandon(s, path, after):
    """Ask for `path` and close the connection `after` seconds later."""
    host, port = s.base.replace("http://", "").split(":")
    sock = socket.create_connection((host, int(port)))
    sock.sendall(f"GET {s.lib + path[len('/api'):]} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n".encode())
    time.sleep(after)
    sock.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--shoebox", required=True)
    ap.add_argument("--port", type=int, default=7798)
    ap.add_argument("--slow", type=float, default=0.5)
    args = ap.parse_args()
    out = os.path.abspath(args.dir)
    root = os.path.join(out, "work")
    if os.path.exists(root):
        shutil.rmtree(root)
    shutil.copytree(os.path.join(out, "pristine"), root)
    with bp.Server(args.shoebox, root, args.port) as s:
        s.wait_clusters()
        people = s.get("/api/people")
        person = max(people, key=lambda p: p["faces"])["id"]
        large = s.get("/api/clusters?size=large&offset=0&limit=30&samples=8&kind=faces")
        cluster = large["clusters"][0]
        face = s.get("/api/faces?limit=1")["faces"][0]["id"]
        paths = [
            "/api/info", "/api/folders", "/api/tags?own=1&limit=500", "/api/timeline", "/api/count",
            "/api/suggest?q=a", "/api/duplicates", "/api/trash", "/api/geo/points", "/api/places", "/api/dates?q=2020",
            "/api/text/stats", "/api/text/check", "/api/text/limits", "/api/recognition/forget",
            "/api/faces/stats", "/api/faces/stats?kind=pets",
            "/api/faces?sort=size&offset=0&limit=300",
            "/api/faces?sort=size&offset=0&limit=300&unmatched=true",
            "/api/faces?sort=score&desc=true&offset=0&limit=300&kind=pets",
            "/api/faces?sort=size&offset=0&limit=300&not_face=true",
            "/api/faces?sort=size&offset=0&limit=300&pet_face=true",
            "/api/faces?sort=size&offset=0&limit=300&rotated=true",
            f"/api/faces/{face}/similar?limit=24",
            "/api/groups", "/api/people", "/api/people?hidden=1", f"/api/people/{person}",
            f"/api/people/{person}/faces?state=confirmed&limit=300", f"/api/people/{person}/faces?state=suggested&limit=300",
            "/api/people/search?q=a", "/api/groups/search?q=a",
            "/api/clusters?size=large&offset=0&limit=30&samples=8&kind=faces",
            "/api/clusters?size=small&offset=0&limit=60&samples=2&kind=faces",
            f"/api/clusters/{cluster['id']}/faces?generation={cluster['generation']}",
        ]
        slow = []
        print(f"{'endpoint':<75} {'first':>8} {'again':>8} {'bytes':>9}")
        for path in paths:
            status, raw, first = s.request(path)
            if status != 200:
                print(f"{path:<75} HTTP {status} {raw[:80]!r}")
                continue
            times, size = bp.timed(s, path, repeat=2)
            flag = "  SLOW" if max(first, *times) > args.slow else ""
            if flag:
                slow.append(path)
            print(f"{path:<75} {first * 1000:6.0f}ms {min(times) * 1000:6.0f}ms {size:9d}{flag}")
        print(f"\n{len(slow)} slow (> {args.slow} s)")
        if slow:
            worst = max(slow, key=lambda p: s.request(p)[2])
            print(f"\ncalled off: {worst} abandoned after 0.3 s, then /api/groups")
            for round_ in range(3):
                abandon(s, worst, 0.3)
                _, _, t = s.request("/api/groups")
                print(f"  /api/groups right after took {t * 1000:.0f} ms")


if __name__ == "__main__":
    sys.exit(main())
