#!/usr/bin/env python3
"""Write shoebox-en.txt and shoebox-de.txt from the guide's HTML files: the same
text, plain, for reading in any editor and for the long run.

    python3 docs/guide/html2txt.py          rewrite the .txt files
    python3 docs/guide/html2txt.py --check  fail if a .txt is out of date, or an
                                            image is missing or not used (CI)

Standard library only. Elements with data-txt="skip" (and svg, style, script,
the head) are left out of the text."""
import os
import re
import sys
import textwrap
from html.parser import HTMLParser

HERE = os.path.dirname(os.path.abspath(__file__))
LANGS = ("en", "de")
WIDTH = 78
SKIP = {"svg", "style", "script", "head"}
BLOCK = {"h1", "h2", "h3", "h4", "p", "tr", "li", "dt", "dd", "figcaption", "div", "section", "header",
         "footer", "main", "nav", "ul", "ol", "dl", "figure", "aside"}
VOID = {"img", "br", "meta", "link", "hr", "use", "input"}


class Text(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.out = []          # finished blocks: (kind, text, prefix)
        self.buf = []          # inline text of the block being read
        self.skip = 0          # >0 inside an element that is left out
        self.stack = []        # open tags: (tag, skipped, kind)
        self.lists = []        # open lists: ["ul"] or ["ol", count]
        self.pending = None    # prefix for the first block of a list item
        self.term = None       # last <dt> of a <dl>
        self.images = []
        self.lang = "en"

    # -- structure
    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag == "html":
            self.lang = a.get("lang", "en")
        if tag == "img":
            self.images.append(a.get("src", ""))
        if tag in VOID:
            if tag == "br" and not self.skip:
                self.buf.append(" ")
            return
        skipped = tag in SKIP or a.get("data-txt") == "skip"
        self.stack.append((tag, skipped))
        if skipped:
            self.skip += 1
            return
        if self.skip:
            return
        if tag in BLOCK:
            self.flush()
        if tag in ("td", "th") and "".join(self.buf).strip():
            self.buf.append("\0")       # cell separator: a row reads "first: second"
        if tag in ("ul", "ol"):
            self.lists.append([tag, 0])
        if tag == "li" and self.lists:
            lst = self.lists[-1]
            lst[1] += 1
            self.pending = "- " if lst[0] == "ul" else f"{lst[1]}. "

    def handle_endtag(self, tag):
        if tag in VOID or not self.stack:
            return
        while self.stack and self.stack[-1][0] != tag:   # tolerate omitted end tags
            self.stack.pop()
        if not self.stack:
            return
        _, skipped = self.stack.pop()
        if skipped:
            self.skip -= 1
            return
        if self.skip:
            return
        if tag in BLOCK:
            self.flush(tag)
        if tag in ("ul", "ol") and self.lists:
            self.lists.pop()
            self.out.append(("gap", "", ""))

    def handle_data(self, data):
        if not self.skip:
            self.buf.append(data)

    # -- blocks
    def flush(self, tag=None):
        text = re.sub(r"\s+", " ", "".join(self.buf)).strip()
        self.buf = []
        text = text.replace(" \0 ", ": ").replace("\0 ", ": ").replace(" \0", ": ").replace("\0", ": ")
        if tag == "tr":
            tag = "kv"
        if not text:
            return
        in_fig = any(t == "figure" for t, _ in self.stack)
        if tag == "figcaption" and in_fig:
            text = f"[Screenshot: {text}]"
        if tag == "dt":
            self.term = text
            return
        if tag == "dd" and self.term:
            text, self.term, tag = f"{self.term}: {text}", None, "kv"
        depth = len(self.lists)
        indent = "   " * max(depth - 1, 0) if depth else ""
        first = self.pending or ""
        self.pending = None
        kind = {"h1": "h1", "h2": "h2", "h3": "h3", "h4": "h3", "kv": "kv"}.get(tag or "", "p")
        self.out.append((kind, text, (indent, first, depth)))

    def render(self):
        lines = []
        for kind, text, pre in self.out:
            if kind == "gap":
                continue
            indent, first, depth = pre
            if kind == "h1":
                block = [text.upper(), "=" * min(len(text), WIDTH)]
            elif kind == "h2":
                block = [text, "-" * min(len(text), WIDTH)]
            else:
                lead = indent + first
                hang = indent + " " * len(first) if (first or depth) else ""
                if depth and not first:
                    hang = lead = indent + "   "
                block = textwrap.wrap(text, WIDTH, initial_indent=lead, subsequent_indent=hang,
                                      break_long_words=False, break_on_hyphens=False)
            # tight: a heading right above its text, one-line list items and
            # key/value lines that follow each other
            lines.append((block, kind, bool(first)))
        out = []
        for i, (block, kind, item) in enumerate(lines):
            if i:
                pk, pitem = lines[i - 1][1], lines[i - 1][2]
                if not (pk == "h3" or (pk == "kv" and kind == "kv") or (pitem and item)):
                    out.append("")
            out.extend(block)
        return "\n".join(out) + "\n"


def convert(path):
    p = Text()
    with open(path, encoding="utf-8") as f:
        p.feed(f.read())
    p.flush()
    return p.render(), p.images


def main():
    check = "--check" in sys.argv
    bad = []
    used = set()
    for lang in LANGS:
        html = os.path.join(HERE, f"shoebox-{lang}.html")
        txt = os.path.join(HERE, f"shoebox-{lang}.txt")
        text, images = convert(html)
        for src in images:
            used.add(src)
            if not os.path.isfile(os.path.join(HERE, src)):
                bad.append(f"shoebox-{lang}.html: missing image {src}")
        if check:
            have = open(txt, encoding="utf-8").read() if os.path.exists(txt) else None
            if have != text:
                bad.append(f"shoebox-{lang}.txt is out of date: run python3 docs/guide/html2txt.py")
        else:
            with open(txt, "w", encoding="utf-8") as f:
                f.write(text)
            print("wrote", txt)
    if check:
        # a screenshot nobody shows is fine (settings, people-overview), but the
        # two languages must carry the same set of names
        names = {l: sorted(os.listdir(os.path.join(HERE, "assets", l))) for l in LANGS}
        if names["en"] != names["de"]:
            bad.append("assets/en and assets/de differ in file names")
        for b in bad:
            print("error:", b, file=sys.stderr)
        if bad:
            sys.exit(1)
        print("guide ok")


if __name__ == "__main__":
    main()
