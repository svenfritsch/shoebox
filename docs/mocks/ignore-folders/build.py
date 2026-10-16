#!/usr/bin/env python3
"""Static mock-ups for phase 15c (docs/mocks/ignore-folders/): the Duplicates screen, tab
"Across drives", in All drives. Not the real app: plain HTML on core/web/app.css.
`python3 build.py` writes *.html, `node shoot.mjs` turns them into PNGs. The sample pictures are
the ones of ../export/img (made by ../export/build.py), not a second copy."""
import os, sys, importlib.util
HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("exp", os.path.join(HERE, "..", "export", "build.py"))
exp = importlib.util.module_from_spec(spec); spec.loader.exec_module(exp)

CSS = """<style>
.ign { display:flex; align-items:center; gap:8px; flex-wrap:wrap; margin:0 0 12px; padding:10px 12px; border:1px solid var(--line); border-radius:10px; background:var(--panel); font-size:13px; }
.ign .lbl { color: var(--muted); }
.ign input { flex:1; min-width:180px; max-width:260px; padding:5px 10px; border-radius:8px; border:1px solid var(--line); background:var(--bg); color:var(--fg); }
.ign .note { width:100%; color:var(--muted); font-size:12px; }
.chipfield { flex:1 1 100%; display:flex; flex-wrap:wrap; align-items:center; gap:6px; min-height:40px; padding:5px 8px; border:1px solid var(--accent); border-radius:10px; background:var(--bg); box-shadow:0 0 0 3px color-mix(in srgb, var(--accent) 22%, transparent); }
.chipfield .caret { width:1px; height:20px; background:var(--fg); margin-left:2px; }
.chipfield .ph2 { display:none; }
.sugwrap { position:relative; flex:1 1 100%; }
.sug { position:absolute; left:0; right:0; top:calc(100% + 4px); z-index:5; background:var(--bg); border:1px solid var(--line); border-radius:10px; box-shadow:0 6px 18px rgba(0,0,0,.18); overflow:hidden; font-size:14px; }
.sug div { display:flex; gap:10px; align-items:baseline; padding:7px 12px; }
.sug div.on { background:color-mix(in srgb, var(--accent) 16%, transparent); }
.sug b { font-weight:700; } .sug small { margin-left:auto; color:var(--muted); font-size:12px; white-space:nowrap; }
.sug .add { border-top:1px solid var(--line); color:var(--accent); }
.kb { margin-left:auto; align-self:flex-start; }
.kbbar { display:flex; align-items:center; gap:10px; margin:0 0 10px; }
.kbbar .grow { flex:1; color:var(--muted); font-size:13px; }
.callout { position:absolute; z-index:9; font:600 13px -apple-system,sans-serif; color:#fff; background:#d9534f; border-radius:99px; padding:2px 10px; }
.drives li .row { display:flex; gap:8px; } .dot { width:8px; height:8px; border-radius:50%; background:#3a9d63; align-self:center; }
.group.cross { margin-bottom:14px; padding-bottom:14px; border-bottom:1px solid var(--line); }
.side-drives { padding:0 14px 8px; font-size:14px; } .side-drives div { padding:4px 0; display:flex; gap:8px; align-items:center; } .side-drives .on { font-weight:600; color:var(--accent); }
</style>"""

def group(i, name, size, copies, kb=False):
    rows = ""
    for disk, folder in copies:
        rows += ('<div class="copy-row"><input type="checkbox"><div class="where"><span class="disk">%s</span><div><button class="folder">%s</button></div></div>'
                 '<span class="state">stays</span></div>') % (disk, folder)
    right = '<button class="btn quiet kb">Keep both</button>' if kb else ""
    return ('<div class="group cross"><a class="thumb"><img src="../export/img/p%02d.jpg"></a><div class="main"><div style="display:flex"><div><div class="name">%s</div>'
            '<div class="meta">on 2 drives · %s</div></div>%s</div>%s</div></div>') % (i, name, size, right, rows)

SIDE = ('<div class="side-drives"><div><span class="dot"></span>Noah (SSD-NOAH)</div><div><span class="dot"></span>Lena (SSD-LENA)</div>'
        '<div class="on"><span class="dot"></span>All drives</div></div>')

def page(inner):
    body = ('<div id="page" style="padding:16px 18px"><h2 style="margin:0 0 8px">Duplicates</h2>'
            '<div class="tabs underline"><button class="on">Across drives</button></div>%s</div>') % inner
    h = exp.shell(body, title="All drives", month="")
    h = h.replace('<div class="side-tags">', SIDE + '<div class="side-tags" style="display:none">', 1)
    h = h.replace("1,240 photos · 31 videos · last scan 10/9/2026", "2 drives online · 4,812 photos")
    return h.replace("</head>", CSS + "</head>", 1)

def dupbar(n=0):
    return ('<div class="dup-bar"><span>%d marked</span><span class="meta">Mark all copies on</span><select><option>SSD-LENA</option></select>'
            '<button class="btn quiet">Mark</button><button class="btn quiet">Clear</button><button class="btn danger" disabled>Move to trash</button></div>') % n

def mocks():
    M = {}
    # 1. "Keep both": where it would sit (NOT planned)
    sub = '<p class="sub">312 photos exist on more than one drive (SSD-NOAH, SSD-LENA). Move the copy you do not need to the trash of its drive.</p>'
    kbbar = '<div class="kbbar"><span class="grow">Intentional copies?</span><button class="btn quiet">Keep both for all 312 shown</button></div>'
    g = (group(0, "IMG_4021.jpg", "3.4 MB", [("SSD-NOAH", "2025-07 Kroatien"), ("SSD-LENA", "2025-07 Kroatien")], kb=True)
         + group(1, "IMG_4022.jpg", "2.9 MB", [("SSD-NOAH", "2025-07 Kroatien"), ("SSD-LENA", "2025-07 Kroatien")], kb=True)
         + group(2, "IMG_4040.jpg", "4.1 MB", [("SSD-NOAH", "2025-07 Kroatien"), ("SSD-LENA", "2025-07 Kroatien")], kb=True))
    M["1-keep-both-placement"] = page(sub + kbbar + g + dupbar())
    # 2. proposed: Ignore folders
    ign = ('<div class="ign"><span class="lbl">Ignore folders</span>'
           '<div class="chipfield"><span class="chip">2025-07 Kroatien<button>✕</button></span><span class="chip">2025-12 Weihnachten<button>✕</button></span>'
           '<span class="chip">Festival<button>✕</button></span><span class="caret"></span><span class="ph2">Folder name, then Enter</span></div>'
           '<span class="note">Photos with a copy in a folder with one of these names are left out, whatever the other copies are called. Nothing is deleted or changed. 312 hidden.</span></div>')
    sub2 = '<p class="sub">3 photos exist on more than one drive (SSD-NOAH, SSD-LENA). Move the copy you do not need to the trash of its drive.</p>'
    g2 = (group(3, "DSC_0187.jpg", "5.2 MB", [("SSD-NOAH", "Noah/Festival 2025"), ("SSD-LENA", "Gemeinsam/Festival")])
          + group(4, "IMG_5310.jpg", "3.0 MB", [("SSD-NOAH", "Downloads"), ("SSD-LENA", "Lena/Handy 2025")]))
    M["2-ignore-folders"] = page(ign + sub2 + g2 + dupbar())
    sug = ('<div class="ign"><span class="lbl">Ignore folders</span><div class="sugwrap">'
           '<div class="chipfield"><span class="chip">2025-12 Weihnachten<button>✕</button></span><span class="chip">Festival<button>✕</button></span>'
           '<span class="chip">Noah privat<button>✕</button></span><span class="chip">Lena privat<button>✕</button></span>'
           '<span class="chip">Handy-Import<button>✕</button></span><span class="chip">Gemeinsam<button>✕</button></span>'
           '<span class="chip">Silvester 2024<button>✕</button></span><span style="padding:0 2px">Kro</span><span class="caret"></span></div>'
           '<div class="sug">'
           '<div class="on"><span><b>Kro</b>atien Highlights</span><small>58 photos · SSD-NOAH, SSD-LENA</small></div>'
           '<div><span><b>Kro</b>atien Fotos</span><small>41 photos · SSD-LENA</small></div>'
           '<div><span>2025-07 <b>Kro</b>atien</span><small>312 photos · SSD-NOAH</small></div>'
           '<div><span>Urlaub <b>Kro</b>atien</span><small>312 photos · SSD-LENA</small></div>'
           '<div class="add"><span>Add “Kro”</span></div></div></div></div>')
    M["3-ignore-suggestions"] = page(sug + '<div style="height:260px"></div>' + dupbar())
    return M

if __name__ == "__main__":
    for k, v in mocks().items():
        open(os.path.join(HERE, k + ".html"), "w").write(v)
    print("ok")
