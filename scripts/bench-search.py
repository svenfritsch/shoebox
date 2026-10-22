#!/usr/bin/env python3
"""Benchmark of the search box on a large synthetic library.

    scripts/bench-search.py make <dir> --shoebox <binary> [--files 100000]
    scripts/bench-search.py run  <dir> --shoebox <binary> [--port 7798] [--word longsea]

`make` builds the library of `scripts/bench-people.py make` (index rows of
100 000 files, about 300 000 faces, 300 people, 8 groups, pets) into
`<dir>/pristine` and gives it tags as a scan and a person would: every file
the tags of its folder's name, a few thousand own tags, some hearts.

`run` copies `<dir>/pristine` to `<dir>/work` and times, against a running
`shoebox serve`:
  - every request the search box sent for one typed text (tags, people,
    pets, dates, groups), the one it sends now (`suggest`) and the
    timeline, alone;
  - ten requests at once of which nine are called off (closing their
    connection, as the browser does), how long the tenth takes;
  - typing a word one letter every 120 ms as the page did before (every
    letter asks for everything at once, nothing is called off) and as it
    does now (one request for the list after a pause of 150 ms, the
    timeline after 300 ms, a
    request that a newer letter replaces is called off by closing its
    connection, as the browser does): how long after the last letter the
    last list and the timeline arrive.

Nothing here is an original: the library has index rows only.
"""

import argparse
import http.client
import importlib.util
import os
import shutil
import socket
import sqlite3
import statistics
import threading
import time
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("bench_people", os.path.join(HERE, "bench-people.py"))
bench_people = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench_people)
Server = bench_people.Server

THEMES = ["Holiday", "Birthday", "Wedding", "Garden", "Mountains", "Beach", "Christmas", "School", "Concert", "Longsea Trip",
          "London", "Lisbon", "Hiking", "Skiing", "Party", "Family", "Friends", "Zoo", "Museum", "Harbour"]
OWN = ["best", "print", "album 2019", "for mum", "longsea", "lovely", "to sort", "wallpaper", "project", "recipe"]


def make(args):
    bench_people.make(args)
    root = os.path.join(os.path.abspath(args.dir), "pristine")
    lib = sqlite3.connect(os.path.join(root, ".shoebox", "library.db"))
    t0 = time.time()
    names = sorted({f"Event {i}" for i in range((args.files + 199) // 200)} | set(THEMES) | set(OWN) | {"favorite"})
    lib.executemany("INSERT OR IGNORE INTO tags (name) VALUES (?)", [(n,) for n in names])
    tag = dict(lib.execute("SELECT name, id FROM tags"))
    rows = []
    now = int(time.time())
    for (fid, folder_id) in lib.execute("SELECT id, folder_id FROM files").fetchall():
        folder = folder_id - 2
        rows.append((fid, tag[f"Event {folder}"], "folder", None))
        rows.append((fid, tag[THEMES[folder % len(THEMES)]], "folder", None))
        if fid % 23 == 0:
            rows.append((fid, tag[OWN[fid % len(OWN)]], "user", now - fid))
        if fid % 97 == 0:
            rows.append((fid, tag["favorite"], "user", now - fid))
    lib.executemany("INSERT OR IGNORE INTO file_tags (file_id, tag_id, source, added_at) VALUES (?, ?, ?, ?)", rows)
    lib.commit()
    lib.close()
    print(f"added {len(rows)} tags of {len(names)} names in {time.time() - t0:.0f} s")


def suggestion_paths(lib, text):
    q = urllib.parse.quote(text)
    return [
        f"{lib}/tags?limit=8&own=1&q={q}",
        f"{lib}/people/search?limit=6&q={q}",
        f"{lib}/pets/search?q={q}",
        f"{lib}/dates?q={q}",
        f"{lib}/groups/search?limit=4&q={q}",
    ]


def suggest_path(lib, text):
    return f"{lib}/suggest?q={urllib.parse.quote(text)}"


def timeline_path(lib, text):
    return f"{lib}/timeline?q={urllib.parse.quote(text)}"


class Request(threading.Thread):
    """One GET on a connection of its own; `abort` closes it as a browser
    does when a page calls a fetch off."""

    def __init__(self, port, path):
        super().__init__(daemon=True)
        self.port, self.path = port, path
        self.conn = None
        self.done = None
        self.status = None
        self.aborted = False
        self.lock = threading.Lock()

    def run(self):
        try:
            with self.lock:
                if self.aborted:
                    return
                self.conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=600)
            self.conn.request("GET", self.path)
            r = self.conn.getresponse()
            r.read()
            self.status = r.status
            self.done = time.perf_counter()
        except (OSError, http.client.HTTPException):
            pass

    def abort(self):
        with self.lock:
            self.aborted = True
            sock = self.conn.sock if self.conn else None
            if sock:
                try:
                    sock.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
                sock.close()


def send(port, path):
    r = Request(port, path)
    r.start()
    return r


def typing(port, lib, word, gap, new):
    """Types `word` one letter every `gap` seconds; returns the seconds from
    the last letter until its list and its timeline arrived."""
    pending = []  # the current letter's requests (new behaviour: called off)
    timers = []
    final = {"sugg": [], "tl": []}
    lock = threading.Lock()

    def ask_suggestions(text, last):
        with lock:
            reqs = [send(port, p) for p in ([suggest_path(lib, text)] if new else suggestion_paths(lib, text))]
            pending.extend(reqs)
            if last:
                final["sugg"] = reqs

    def ask_timeline(text, last):
        with lock:
            if new:
                for r in pending:
                    if "/timeline" in r.path:
                        r.abort()
            r = send(port, timeline_path(lib, text))
            pending.append(r)
            if last:
                final["tl"] = [r]

    for i in range(1, len(word) + 1):
        text, last = word[:i], i == len(word)
        if new:
            for t in timers:
                t.cancel()
            with lock:
                for r in pending:
                    if "/timeline" not in r.path:
                        r.abort()
            timers = [threading.Timer(0.15, ask_suggestions, (text, last)), threading.Timer(0.3, ask_timeline, (text, last))]
            for t in timers:
                t.start()
        else:
            ask_suggestions(text, last)
            # The timeline came 300 ms after the last letter already then.
            if last:
                threading.Timer(0.3, ask_timeline, (text, last)).start()
        if not last:
            time.sleep(gap)
    typed = time.perf_counter()
    time.sleep(0.35)
    with lock:
        wanted = final["sugg"] + final["tl"]
    for r in wanted:
        r.join()
    with lock:
        everything = list(pending)
    for r in everything:
        r.join(timeout=600)
    sugg = max(r.done for r in final["sugg"]) - typed
    tl = final["tl"][0].done - typed
    return sugg, tl


def run(args):
    out = os.path.abspath(args.dir)
    work = os.path.join(out, "work")
    if os.path.exists(work):
        shutil.rmtree(work)
    shutil.copytree(os.path.join(out, "pristine"), work)
    with Server(args.shoebox, work, args.port) as s:
        s.wait_clusters(timeout=3600)
        lib = s.lib
        # The first after the server started (or the index changed): nothing kept yet.
        print("first, nothing kept yet (ms):")
        for path in suggestion_paths(lib, "l") + [suggest_path(lib, "l"), timeline_path(lib, "l")]:
            _, _, t = s.request("/api" + path[len(lib):])
            print(f"  {t * 1000:8.1f}  {path[len(lib):]}")
        print("alone, median of 3 (ms):")
        for text in ["l", "lon", args.word]:
            for path in suggestion_paths(lib, text) + [suggest_path(lib, text), timeline_path(lib, text)]:
                times = []
                for _ in range(3):
                    c = http.client.HTTPConnection("127.0.0.1", args.port, timeout=600)
                    t = time.perf_counter()
                    c.request("GET", path)
                    r = c.getresponse()
                    r.read()
                    times.append(time.perf_counter() - t)
                    c.close()
                print(f"  {statistics.median(times) * 1000:8.1f}  {path[len(lib):]}")
        # Ten requests nobody kept the answer of (each with another filter,
        # a parameter the server does not read), nine called off after 50 ms
        # as the page does when a newer letter comes: how long the tenth takes.
        for rnd in range(3):
            reqs = [send(args.port, f"{lib}/people/search?limit=6&q=a&unused={rnd}-{i}") for i in range(10)]
            t = time.perf_counter()
            time.sleep(0.05)
            for r in reqs[:-1]:
                r.abort()
            reqs[-1].join()
            print(f"10 uncached people searches, 9 called off: the last after {(reqs[-1].done - t) * 1000:.0f} ms")
        for label, new in [("before (every letter, nothing called off)", False), ("now (pause, called off)", True)]:
            results = [typing(args.port, lib, args.word, args.gap, new) for _ in range(args.rounds)]
            sugg = statistics.median(r[0] for r in results)
            tl = statistics.median(r[1] for r in results)
            print(f"typing '{args.word}', {label}: list {sugg * 1000:.0f} ms, timeline {tl * 1000:.0f} ms after the last letter (median of {args.rounds})")
            time.sleep(1)


def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    m = sub.add_parser("make")
    m.add_argument("dir")
    m.add_argument("--shoebox", required=True)
    m.add_argument("--files", type=int, default=100_000)
    m.add_argument("--faces-per-photo", type=float, default=3.3)
    m.add_argument("--port", type=int, default=7798)
    r = sub.add_parser("run")
    r.add_argument("dir")
    r.add_argument("--shoebox", required=True)
    r.add_argument("--port", type=int, default=7798)
    r.add_argument("--word", default="longsea")
    r.add_argument("--gap", type=float, default=0.12)
    r.add_argument("--rounds", type=int, default=3)
    args = p.parse_args()
    make(args) if args.cmd == "make" else run(args)


if __name__ == "__main__":
    main()
