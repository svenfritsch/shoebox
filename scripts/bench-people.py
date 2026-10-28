#!/usr/bin/env python3
"""Benchmark of the people and face endpoints on a large synthetic library.

    scripts/bench-people.py make <dir> --shoebox <binary> [--files 100000] [--faces-per-photo 3.3]
    scripts/bench-people.py run  <dir> --shoebox <binary> [--port 7799] [--namings 6]
    scripts/bench-people.py probe <dir> --shoebox <binary> [--port 7799] [--move 5000]

`make` writes a library of `--files` index rows (no photo files: the people
endpoints read only the index) with about 300 000 detected faces, people,
groups and decisions into `<dir>/pristine`, then starts `shoebox serve` once so
the neighbour lists and clusters are computed (as on a drive that was
recognised before), and stops it again.

`run` copies `<dir>/pristine` to `<dir>/work` and times, against a running
`shoebox serve`:
  - every endpoint the People and Unnamed pages ask for, alone (idle);
  - naming clusters and single faces as on the Unnamed page, while a second
    client asks for `/api/groups` (a trivial query) every 100 ms, which shows
    how long requests wait for the connection, and while the regrouping
    after each name runs.

`probe` copies `<dir>/pristine` to `<dir>/work` as well and times each of the
requests that held up the rest of the app, right after the server started: the
first `/suggest`, the Face check's stats, every filter of its lists (faces and
pets, as the page asks for them), similar faces, the duplicates and a move of
`--move` photos (made as empty, sparse files that match the index). Two passes
on a fresh copy and a fresh server each, the same requests in the same order:
each request alone, and each while a second client asks for `/api/groups` every
100 ms, whose longest wait is how long the rest of the app was held up. It
prints the table of section 2 of docs/design/responsiveness.md, and the lines
of the slow-request log the server wrote meanwhile.

Needs numpy (only for making the embeddings). Nothing here is an original:
the library has index rows only, under a folder of its own.
"""

import argparse
import hashlib
import json
import os
import shutil
import signal
import sqlite3
import statistics
import subprocess
import sys
import threading
import time
import urllib.request

FACES_MODEL = "yunet-2023mar+sface-2021dec"
PETS_MODEL = "yolox-s+pp-resnet50"
DIM = 128
# Straight to the local server, never through a proxy from the environment.
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def key_of(n):
    return hashlib.blake2b(str(n).encode(), digest_size=32).hexdigest()


def make(args):
    import numpy as np

    rng = np.random.default_rng(20261019)
    out = os.path.abspath(args.dir)
    root = os.path.join(out, "pristine")
    if os.path.exists(root):
        shutil.rmtree(root)
    os.makedirs(root)
    # An empty scan makes library.db with the current schema, the photo app
    # recognition.db.
    subprocess.run([args.shoebox, "scan", root, "--no-thumbs"], check=True, stdout=subprocess.DEVNULL)
    with Server(args.shoebox, root, args.port):
        pass
    lib = sqlite3.connect(os.path.join(root, ".shoebox", "library.db"))
    lib.execute("ATTACH DATABASE ? AS recog", (os.path.join(root, ".shoebox", "recognition.db"),))
    now = int(time.time())
    t0 = time.time()

    # Folders of 200 files, files: jpeg, heic, png, video; some missing, some
    # duplicates (the same content twice).
    n = args.files
    folders = (n + 199) // 200
    lib.execute("INSERT OR IGNORE INTO folders (id, parent_id, path, path_nfc, name, last_seen) VALUES (1, NULL, '', '', '', 1)")
    lib.executemany(
        "INSERT INTO folders (id, parent_id, path, path_nfc, name, last_seen) VALUES (?, 1, ?, ?, ?, 1)",
        [(i + 2, f"{2000 + i // 50}-{i % 12 + 1:02d} Event {i}", f"{2000 + i // 50}-{i % 12 + 1:02d} Event {i}", f"Event {i}") for i in range(folders)],
    )
    kinds = rng.choice(["jpeg", "heic", "png", "video"], size=n, p=[0.85, 0.08, 0.02, 0.05])
    missing = rng.random(n) < 0.015
    dup = rng.random(n) < 0.03
    rows = []
    keys = []
    for i in range(n):
        key = key_of(i - 1 if dup[i] and i > 0 else i)
        keys.append(key)
        folder = i // 200
        name = f"IMG_{i:06d}.{'MOV' if kinds[i] == 'video' else kinds[i].upper()}"
        path = f"{2000 + folder // 50}-{folder % 12 + 1:02d} Event {folder}/{name}"
        rows.append((i + 1, folder + 2, path, path, name, str(kinds[i]), 2_000_000 + i, now * 10**9, key,
                     f"20{10 + folder // 50 % 15}-{folder % 12 + 1:02d}-15T12:00:00", 4032, 3024, now,
                     now if missing[i] else None))
    lib.executemany(
        "INSERT INTO files (id, folder_id, path, path_nfc, name, kind, size, mtime_ns, quick_hash, taken, width, height, added_at, missing_since)"
        " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        rows,
    )
    images = sorted({keys[i] for i in range(n) if kinds[i] != "video"})

    # Identities: 400 who come back often (family and friends, Zipf), many
    # strangers. An embedding is its identity's centre plus noise.
    family = 400
    strangers = 40_000
    centres = rng.normal(size=(family + strangers, DIM)).astype(np.float32)
    centres /= np.linalg.norm(centres, axis=1, keepdims=True)
    weights = 1.0 / np.arange(1, family + 1) ** 0.9
    weights /= weights.sum()

    faces = []
    looked = []
    emb_rows = []
    fid = 0
    counts = rng.poisson(args.faces_per_photo, size=len(images))
    counts[rng.random(len(images)) < 0.3] = 0
    for k, key in enumerate(images):
        c = int(counts[k])
        looked.append((key, "faces", FACES_MODEL, 1600, 1200, c, now))
        if c == 0:
            continue
        who = np.where(rng.random(c) < 0.62, rng.choice(family, size=c, p=weights), family + rng.integers(0, strangers, size=c))
        noise = rng.normal(scale=rng.uniform(0.04, 0.075, size=(c, 1)), size=(c, DIM)).astype(np.float32)
        e = centres[who] + noise
        e /= np.linalg.norm(e, axis=1, keepdims=True)
        for j in range(c):
            fid += 1
            w = float(rng.uniform(0.015, 0.2))
            x = (j % 5) * 0.19 + 0.01
            y = 0.1 + (j // 5) * 0.4
            faces.append((fid, key, FACES_MODEL, x, y, w, w * 1.3, float(rng.uniform(0.6, 0.99)), e[j].tobytes(), None, int(who[j])))
        if len(faces) >= 50_000:
            lib.executemany("INSERT INTO recog.faces (id, key, model, x, y, w, h, score, emb, species) VALUES (?,?,?,?,?,?,?,?,?,?)", [f[:10] for f in faces])
            emb_rows.extend((f[0], f[1], f[3], f[4], f[5], f[6], f[10]) for f in faces)
            faces = []
    # Pets: a few cats and dogs in their own space.
    pet_keys = images[::60]
    pet_centres = rng.normal(size=(30, DIM)).astype(np.float32)
    pet_centres /= np.linalg.norm(pet_centres, axis=1, keepdims=True)
    pets = []
    for key in pet_keys:
        looked.append((key, "pets", PETS_MODEL, 1600, 1200, 1, now))
        who = int(rng.integers(0, 30))
        e = pet_centres[who] + rng.normal(scale=0.02, size=DIM).astype(np.float32)
        e /= np.linalg.norm(e)
        fid += 1
        pets.append((fid, key, PETS_MODEL, 0.5, 0.5, 0.3, 0.3, 0.9, e.astype(np.float32).tobytes(), "cat" if who < 15 else "dog"))
    lib.executemany("INSERT INTO recog.faces (id, key, model, x, y, w, h, score, emb, species) VALUES (?,?,?,?,?,?,?,?,?,?)", [f[:10] for f in faces] + pets)
    emb_rows.extend((f[0], f[1], f[3], f[4], f[5], f[6], f[10]) for f in faces)
    lib.executemany("INSERT INTO recog.looked (key, task, model, width, height, found, done_at) VALUES (?,?,?,?,?,?,?)", looked)

    # People: the 300 most frequent identities have names, 70 % of their
    # faces confirmed; groups of five; some hidden. Strangers ignored, false
    # finds, rejections, faces drawn by hand.
    groups = 8
    lib.executemany("INSERT INTO groups (id, name, position) VALUES (?, ?, ?)", [(g + 1, f"Group {g + 1}", g + 1) for g in range(groups)])
    people = 300
    lib.executemany(
        "INSERT INTO people (id, name, group_id, hidden) VALUES (?, ?, ?, ?)",
        [(p + 1, f"Person {p + 1:03d}", (p // 5 + 1) if p < groups * 5 else None, 1 if p % 37 == 36 else 0) for p in range(people)],
    )
    decisions = []
    for (face, key, x, y, w, h, who) in emb_rows:
        r = rng.random()
        if who < people and r < 0.70:
            decisions.append((key, x, y, w, h, who + 1, "confirmed", 0, now - int(rng.integers(0, 10**7))))
        elif who < people and r < 0.71:
            decisions.append((key, x, y, w, h, int((who + 1) % people + 1), "rejected", 0, now))
        elif who >= family and r < 0.06:
            decisions.append((key, x, y, w, h, None, "ignored", 0, now))
        elif r < 0.005:
            decisions.append((key, x, y, w, h, None, "not_face", 0, now))
    for d in range(150):
        key = images[(d * 613) % len(images)]
        decisions.append((key, 0.8, 0.8, 0.1, 0.1, d % people + 1, "confirmed", 1, now))
    lib.executemany(
        "INSERT INTO face_decisions (key, x, y, w, h, person_id, decision, manual, at) VALUES (?,?,?,?,?,?,?,?,?)", decisions
    )
    lib.commit()
    total_faces = lib.execute("SELECT count(*) FROM recog.faces").fetchone()[0]
    print(f"made {n} files, {total_faces} faces, {len(decisions)} decisions, {people} people in {time.time() - t0:.0f} s")
    lib.close()

    # Neighbours and clusters, once, as `recognize` would leave them.
    with Server(args.shoebox, root, args.port) as s:
        t = time.time()
        s.wait_clusters(timeout=3600)
        print(f"first clustering (all neighbour lists): {time.time() - t:.0f} s")


class Server:
    def __init__(self, shoebox, root, port):
        self.base = f"http://127.0.0.1:{port}"
        self.log = open(os.path.join(os.path.dirname(root), f"serve-{os.path.basename(root)}.log"), "w")
        self.proc = subprocess.Popen([shoebox, "serve", root, "--port", str(port)], stdout=self.log, stderr=subprocess.STDOUT)
        started = time.time()
        self.lib = None
        while True:
            try:
                self.lib = "/api/lib/" + json.loads(self.request("/api/libraries")[1])[0]["id"]
                self.get("/api/info")
                break
            except Exception:
                if self.proc.poll() is not None or time.time() - started > 600:
                    raise SystemExit(f"shoebox serve did not start (see {self.log.name})")
                time.sleep(0.2)
        self.open_seconds = time.time() - started

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.proc.send_signal(signal.SIGINT)
        try:
            self.proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        self.log.close()

    def request(self, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        # The library's own routes are under /api/lib/<id>/.
        if self.lib and path != "/api/libraries":
            path = self.lib + path[len("/api"):]
        req = urllib.request.Request(self.base + path, data=data, headers={"Content-Type": "application/json", "X-Shoebox": "1"} if data else {})
        t = time.perf_counter()
        try:
            with OPENER.open(req, timeout=600) as r:
                raw = r.read()
                status = r.status
        except urllib.error.HTTPError as e:
            raw = e.read()
            status = e.code
        return status, raw, time.perf_counter() - t

    def get(self, path):
        status, raw, _ = self.request(path)
        if status != 200:
            raise RuntimeError(f"{path}: {status} {raw[:200]!r}")
        return json.loads(raw)

    def wait_clusters(self, timeout=600):
        started = time.time()
        time.sleep(0.5)
        while True:
            c = self.get("/api/info")["clusters"]
            if not c["stale"] and not c["running"] and not c.get("pending"):
                return time.time() - started
            if time.time() - started > timeout:
                raise SystemExit("clustering did not finish")
            time.sleep(0.2)


def timed(s, path, repeat=3):
    times = []
    size = 0
    for _ in range(repeat):
        status, raw, t = s.request(path)
        if status != 200:
            raise RuntimeError(f"{path}: {status} {raw[:200]!r}")
        times.append(t)
        size = len(raw)
    return times, size


class Prober(threading.Thread):
    """Asks for a cheap endpoint every 100 ms and keeps how long each took."""

    def __init__(self, s, path="/api/groups"):
        super().__init__(daemon=True)
        self.s, self.path, self.times, self.stop = s, path, [], threading.Event()

    def run(self):
        while not self.stop.is_set():
            _, _, t = self.s.request(self.path)
            self.times.append(t)
            self.stop.wait(0.1)


def summary(times):
    if not times:
        return "-"
    times = sorted(times)
    p95 = times[min(len(times) - 1, int(len(times) * 0.95))]
    return f"n={len(times)} median {statistics.median(times) * 1000:.0f} ms, p95 {p95 * 1000:.0f} ms, max {times[-1] * 1000:.0f} ms"


def run(args):
    out = os.path.abspath(args.dir)
    pristine = os.path.join(out, "pristine")
    root = os.path.join(out, "work")
    if os.path.exists(root):
        shutil.rmtree(root)
    shutil.copytree(pristine, root)
    results = {}
    with Server(args.shoebox, root, args.port) as s:
        print(f"server answered after {s.open_seconds:.1f} s")
        t = s.wait_clusters()
        print(f"clustering at start: {t:.1f} s")
        people = s.get("/api/people")
        person = max(people, key=lambda p: p["faces"])["id"]
        large = s.get("/api/clusters?size=large&offset=0&limit=30&samples=8&kind=faces")
        c = large["clusters"][0]
        paths = [
            "/api/info",
            "/api/groups",
            "/api/people",
            "/api/people?hidden=1",
            f"/api/people/{person}",
            f"/api/people/{person}/faces?state=confirmed&limit=300",
            f"/api/people/{person}/faces?state=suggested&limit=300",
            "/api/clusters?size=large&offset=0&limit=30&samples=8&kind=faces",
            "/api/clusters?size=small&offset=0&limit=60&samples=2&kind=faces",
            f"/api/clusters/{c['id']}/faces?generation={c['generation']}",
        ]
        print("\nidle (3 requests each; first, then the best of the other two)")
        for p in paths:
            times, size = timed(s, p)
            results[p] = times
            print(f"  {p:70s} {times[0] * 1000:7.0f} ms  {min(times[1:]) * 1000:7.0f} ms  {size / 1024:8.0f} KiB")

        print(f"\nnaming {args.namings} clusters and {args.namings} single faces, /api/groups asked every 100 ms meanwhile")
        probe = Prober(s)
        probe.start()
        names, clusters_after, singles = [], [], []
        for k in range(args.namings):
            large = s.get("/api/clusters?size=large&offset=0&limit=30&samples=8&kind=faces")
            c = large["clusters"][0]
            status, raw, t = s.request(f"/api/clusters/{c['id']}/name", {"generation": c["generation"], "name": f"Bench {k}"})
            if status != 200:
                print(f"  name cluster: {status} {raw[:200]!r}")
            names.append(t)
            # The page asks for both lists again (refreshUnnamed).
            for p in ("/api/clusters?size=large&offset=0&limit=30&samples=8&kind=faces",
                      "/api/clusters?size=small&offset=0&limit=60&samples=2&kind=faces"):
                _, _, t = s.request(p)
                clusters_after.append(t)
            small = s.get("/api/clusters?size=small&offset=0&limit=60&samples=2&kind=faces")
            face = small["clusters"][0]["faces"][0]["id"]
            status, raw, t = s.request("/api/faces/assign", {"faces": [face], "person_id": person, "open": True})
            if status != 200:
                print(f"  assign: {status} {raw[:200]!r}")
            singles.append(t)
        t = s.wait_clusters()
        probe.stop.set()
        probe.join()
        print(f"  name a cluster          {summary(names)}")
        print(f"  assign one face         {summary(singles)}")
        print(f"  /api/clusters after     {summary(clusters_after)}")
        print(f"  /api/groups meanwhile   {summary(probe.times)}")
        print(f"  regrouping finished {t:.1f} s after the last name")

        print("\nwhile a regrouping runs (/api/groups and /api/people every 100 ms)")
        probe = Prober(s)
        probe2 = Prober(s, "/api/people")
        status, _, _ = s.request("/api/faces/assign", {"faces": [face], "person_id": person})
        probe.start()
        probe2.start()
        time.sleep(0.4)
        t = s.wait_clusters()
        probe.stop.set()
        probe2.stop.set()
        probe.join()
        probe2.join()
        print(f"  regrouping took about {t + 0.4:.1f} s")
        print(f"  /api/groups             {summary(probe.times)}")
        print(f"  /api/people             {summary(probe2.times)}")


def make_files(root, n):
    """Sparse files for `n` present index rows, with the size and modified time
    the index has for them (all a move checks), so they can be moved."""
    lib = sqlite3.connect(os.path.join(root, ".shoebox", "library.db"))
    rows = lib.execute(
        "SELECT id, path, size, mtime_ns FROM files WHERE missing_since IS NULL AND kind != 'video' ORDER BY id LIMIT ?", (n,)
    ).fetchall()
    lib.close()
    for _, path, size, mtime_ns in rows:
        p = os.path.join(root, path)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.truncate(size)
        os.utime(p, ns=(mtime_ns, mtime_ns))
    return [r[0] for r in rows]


def under_probe(s, what, call, probing=True):
    """`call()`, with /api/groups asked every 100 ms meanwhile if `probing`.
    Prints how long it took and, if probing, how long the probe waited at most;
    returns (took, worst wait of the probe or None)."""
    probe = Prober(s) if probing else None
    if probe:
        probe.start()
        time.sleep(0.3)
    t = time.perf_counter()
    status = call()
    took = time.perf_counter() - t
    worst = None
    if probe:
        time.sleep(0.3)
        probe.stop.set()
        probe.join()
        worst = max(probe.times) if probe.times else 0
    print(f"  {what:60s} {status}  {took * 1000:7.0f} ms" + (f"   probe: {summary(probe.times)}" if probe else ""))
    return took, worst


def check_requests(min_px, pet_min_px, face):
    """The requests of the Face check page (`moreFaces`, `/faces/stats`, `similar`)
    in the order of one visit: the stats, then each filter of the faces and of
    the pets as `FACE_FILTERS` and `PET_FILTERS` in app.js, newest first page."""
    page = "sort=size&offset=0&limit=300"
    faces = [
        ("all", ""), ("small", f"&max_px={min_px}"), ("large", f"&min_px={min_px}"), ("rotated", "&rotated=true"),
        ("not a face", "&not_face=true"), ("pet face", "&pet_face=true"), ("not matched yet", "&unmatched=true"),
    ]
    pets = [("all", ""), ("small", f"&max_px={pet_min_px}"), ("large", f"&min_px={pet_min_px}"), ("not a pet", "&not_face=true"), ("not matched yet", "&unmatched=true")]
    reqs = [("/faces/stats", "/api/faces/stats"), ("/faces/stats?kind=pets", "/api/faces/stats?kind=pets")]
    reqs += [(f"/faces, {name}", f"/api/faces?{page}{extra}") for name, extra in faces]
    reqs += [(f"/faces?kind=pets, {name}", f"/api/faces?kind=pets&{page}{extra}") for name, extra in pets]
    reqs.append(("/faces/{id}/similar?limit=24", f"/api/faces/{face}/similar?limit=24"))
    return reqs


def pass_of(args, probing):
    """One pass on a fresh copy of the library and a fresh server; returns
    {request: (took, probe's longest wait or None)} in order, and the slow lines
    the server logged."""
    out = os.path.abspath(args.dir)
    root = os.path.join(out, "work")
    if os.path.exists(root):
        shutil.rmtree(root)
    shutil.copytree(os.path.join(out, "pristine"), root)
    ids = make_files(root, args.move)
    results = {}
    with Server(args.shoebox, root, args.port) as s:
        print(f"server answered after {s.open_seconds:.1f} s")
        print(f"clustering at start: {s.wait_clusters():.1f} s")

        replies = {}

        def timed_get(label, path):
            def call():
                status, raw, _ = s.request(path)
                replies[label] = raw
                return status

            results[label] = under_probe(s, label, call, probing)

        timed_get("/suggest?q=a (first after the start)", "/api/suggest?q=a")
        timed_get("/suggest?q=an", "/api/suggest?q=an")
        # The Face check's first request is its stats; the lists use what it says.
        timed_get("/faces/stats", "/api/faces/stats")
        timed_get("/faces/stats?kind=pets", "/api/faces/stats?kind=pets")
        # The page takes each kind's own value (`faceState.minPx`).
        min_px = json.loads(replies["/faces/stats"])["min_cluster_px"]
        pet_min_px = json.loads(replies["/faces/stats?kind=pets"])["min_cluster_px"]
        face = s.get("/api/faces?sort=size&offset=0&limit=1")["faces"][0]["id"]
        for label, path in check_requests(min_px, pet_min_px, face):
            if label not in ("/faces/stats", "/faces/stats?kind=pets"):
                timed_get(label, path)
        timed_get("/duplicates", "/api/duplicates")

        def move():
            status, raw, _ = s.request("/api/move", {"ids": ids, "folder": "Bench moved"})
            moved = len(json.loads(raw).get("files", [])) if status == 200 else raw[:200]
            return f"{status} ({moved} moved)"

        label = f"/move of {len(ids)} photos"
        results[label] = under_probe(s, label, move, probing)
        log = s.log.name
    with open(log) as f:
        slow = [line.rstrip() for line in f if line.startswith("slow request:")]
    return results, slow


def ms(t):
    return "-" if t is None else f"{t * 1000:.0f} ms" if t < 1 else f"{t:.2f} s"


def probe(args):
    print("pass 1: each request alone")
    alone, slow_alone = pass_of(args, probing=False)
    print("\npass 2: each request under the probe (/api/groups every 100 ms)")
    probed, slow_probed = pass_of(args, probing=True)
    print("\n| Request | Alone | Under the probe | Probe's longest wait |\n|---|---|---|---|")
    for label, (took, _) in alone.items():
        took_probed, worst = probed[label]
        print(f"| `{label}` | {ms(took)} | {ms(took_probed)} | {ms(worst)} |")
    for name, lines in (("alone", slow_alone), ("under the probe", slow_probed)):
        print(f"\nslow-request log, {name}: {len(lines)} lines")
        for line in lines:
            print(f"  {line}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("command", choices=["make", "run", "probe"])
    ap.add_argument("dir")
    ap.add_argument("--shoebox", required=True)
    ap.add_argument("--files", type=int, default=100_000)
    ap.add_argument("--faces-per-photo", type=float, default=4.6)
    ap.add_argument("--port", type=int, default=7799)
    ap.add_argument("--namings", type=int, default=6)
    ap.add_argument("--move", type=int, default=5000)
    args = ap.parse_args()
    {"make": make, "run": run, "probe": probe}[args.command](args)


if __name__ == "__main__":
    main()
