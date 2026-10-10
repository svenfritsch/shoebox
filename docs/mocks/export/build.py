#!/usr/bin/env python3
"""Static mock-ups for the planned Export feature (docs/mocks/export/).

Not the real app: plain HTML that reuses core/web/app.css and the sample pictures
drawn by docs/guide/comic.py. `python3 build.py` writes img/*.jpg and *.html;
`node shoot.mjs` turns every *.html into a PNG next to it.
"""
import sys, os, html
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "guide"))
import comic

SCENES = ["park", "beach", "garden", "sofa", "close", "cat2", "pets:park:1", "pets:garden:2", "grp:park:mia,mia:0", "grp:beach:mia,mia:1", "grp:garden:mia:2", "park"]
def pictures():
    for i, s in enumerate(SCENES):
        p = os.path.join(HERE, "img", "p%02d.jpg" % i)
        if not os.path.exists(p):
            im, _, _ = comic.scene(s)
            im.convert("RGB").resize((640, int(640 * im.height / im.width))).save(p, quality=85)

CSS = """
<style>
body { overflow: hidden; }
.mgrid { display: flex; flex-wrap: wrap; padding: 2px; }
.mgrid .cell { width: 168px; height: 168px; }
.mgrid .cell img { opacity: 1; }
.sech { padding: 12px 14px 4px; font-weight: 600; font-size: 16px; display: flex; align-items: baseline; }
.sech .n { margin-left: 8px; color: var(--muted); font-weight: 400; font-size: 13px; }
.cell .tick { position: absolute; left: 8px; top: 8px; width: 22px; height: 22px; border-radius: 50%; background: var(--accent); color: #fff; font-size: 13px; text-align: center; line-height: 22px; }
.cell .tagbadge { position: absolute; right: 8px; bottom: 8px; background: rgba(0,0,0,.6); color: #fff; font-size: 11px; padding: 1px 7px; border-radius: 99px; }
.btn.export { background: var(--accent); }
.filters .btn { padding: 4px 14px; }
.filters .spacer { flex: 1; }
.dialog.wide { width: min(560px, 100%); }
.dialog .row { display: flex; align-items: center; gap: 8px; margin: 0 0 10px; }
.dialog .row label.lbl { width: 92px; color: var(--muted); font-size: 13px; flex: none; }
.dialog .path { flex: 1; padding: 7px 10px; border-radius: 8px; border: 1px solid var(--line); background: var(--panel); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; font-variant-ligatures: none; }
.dialog .result { margin: 2px 0 12px; padding: 8px 10px; border-radius: 8px; background: var(--panel); font-size: 13px; color: var(--muted); }
.dialog .result b { color: var(--fg); font-weight: 600; }
.dialog .sum { display: flex; gap: 14px; margin: 0 0 12px; flex-wrap: wrap; }
.dialog .sum span { background: var(--panel); border: 1px solid var(--line); border-radius: 99px; padding: 2px 10px; font-size: 13px; }
.dialog .opt { margin: 8px 0 0; }
.dialog .opt .hint { margin: 0 0 0 24px; }
.dialog .check { margin: 8px 0 0; }
.dialog .note { margin: 10px 0 0; padding: 8px 10px; border-radius: 8px; border: 1px solid var(--line); font-size: 13px; }
.dialog .note.good { border-color: #2f8a3b; }
.dialog .note.info { border-color: var(--accent); }
.bar-progress { height: 8px; border-radius: 99px; background: var(--line); overflow: hidden; margin: 10px 0 6px; }
.bar-progress i { display: block; height: 100%; background: var(--accent); }
.picker { padding: 0; width: min(640px, 100%); }
.picker header { padding: 16px 20px 10px; }
.picker h2 { margin: 0; }
.pk { display: flex; border-top: 1px solid var(--line); border-bottom: 1px solid var(--line); height: 330px; }
.pk nav { width: 190px; flex: none; background: var(--panel); border-right: 1px solid var(--line); overflow: hidden; padding: 8px 0; font-size: 14px; }
.pk nav h4 { margin: 8px 14px 3px; font-size: 11px; letter-spacing: .05em; text-transform: uppercase; color: var(--muted); font-weight: 600; }
.pk nav div { padding: 5px 14px; display: flex; gap: 8px; align-items: baseline; }
.pk nav div small { margin-left: auto; color: var(--muted); font-size: 11px; }
.pk nav div.on { background: var(--accent); color: #fff; }
.pk nav div.on small { color: #fff; }
.pk nav div.off { color: var(--muted); }
.pk .main { flex: 1; display: flex; flex-direction: column; min-width: 0; }
.pk .crumbs { padding: 9px 14px; font-size: 13px; color: var(--muted); border-bottom: 1px solid var(--line); white-space: nowrap; overflow: hidden; }
.pk .crumbs b { color: var(--fg); font-weight: 600; }
.pk ul { list-style: none; margin: 0; padding: 4px 0; flex: 1; overflow: hidden; font-size: 14px; }
.pk li { padding: 5px 14px; display: flex; gap: 8px; align-items: baseline; }
.pk li.sel { background: color-mix(in srgb, var(--accent) 18%, transparent); }
.pk li small { margin-left: auto; color: var(--muted); font-size: 12px; }
.pk li .nope { color: var(--muted); font-size: 12px; }
.picker .foot { display: flex; align-items: center; gap: 8px; padding: 12px 20px; }
.picker .foot .grow { flex: 1; color: var(--muted); font-size: 13px; }
.lightbox { display: block; }
.lb-bar .icon { background: none; border: 0; color: #fff; font-size: 20px; padding: 4px 6px; }
.lb-bar .icon svg { display: block; }
.lb-bar .coll.on { color: #6ee7a0; }
.lb-bar .fav-lb { color: #fff; }
.callout { position: absolute; z-index: 9; font: 600 13px -apple-system, sans-serif; color: #fff; background: var(--accent); border-radius: 99px; padding: 2px 10px; }
.toast { position: fixed; left: 50%; transform: translateX(-50%); top: 70px; bottom: auto; }
.set-h { margin-top: 0; }
.lightbox .toast { left: 330px; }
.lightbox .stage { right: 340px; }
.lb-panel { display: flex; }
.lb-panel dl { margin-top: 8px; }
.iconbox { display: inline-flex; align-items: center; justify-content: center; width: 34px; height: 34px; border-radius: 8px; border: 1px solid var(--line); background: var(--panel); vertical-align: middle; }
.kbd { border: 1px solid var(--line); border-radius: 5px; padding: 0 6px; font-size: 12px; background: var(--panel); }
.dialog input[type=text].req { border-color: var(--accent); }
.tree .nm { flex: 1; }
.tree .ct { color: var(--muted); font-size: 12px; }
</style>
"""

HEART = '<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 21s-7.5-4.6-9.6-9.2C1 8.6 2.8 5 6.2 5c2 0 3.4 1.1 4.3 2.6h3C14.4 6.100 15.800 5 17.800 5c3.400 0 5.200 3.600 3.800 6.800C19.500 16.400 12 21 12 21z" transform="translate(0 0)"/></svg>'
def coll(on):
    fill = "currentColor" if on else "none"
    plus = '' if on else '<path d="M12 8v6M9 11h6"/>'
    check = '<path d="M9 11.500l2.200 2.200L15.200 9" stroke="#111"/>' if on else ''
    return ('<svg width="22" height="22" viewBox="0 0 24 24" fill="%s" stroke="currentColor" stroke-width="2" stroke-linejoin="round">'
            '<path d="M6 3h12v18l-6-4.500L6 21z"/>%s%s</svg>') % (fill, plus, check)
ROT = '↺'

def tile(i, sel=False, badge=None, tick="✓"):
    return ('<a class="cell%s"><div class="ph"><img class="ok" src="img/p%02d.jpg"></div>%s%s</a>'
            % (" sel" if sel else "", i, '<span class="tick">%s</span>' % tick if sel else "",
               '<span class="tagbadge">%s</span>' % badge if badge else ""))

def shell(body, *, title="Photos", month="July 2025", tags=("Highlights", "print-march", "album-italy"), active=None, chips="", extra="", selbar="", modal="", select_on=False):
    side_tags = "".join('<li><div class="row"><span class="tw"></span><span class="nm">%s%s</span><span class="ct">%s</span></div></li>'
                        % ("# ", t, c) for t, c in zip(tags, (3, 42, 17)))
    return """<!doctype html><html lang="en"><head><meta charset="utf-8"><title>mock</title>
<link rel="stylesheet" href="../../../core/web/app.css">%s</head><body class="%s">
<header class="bar">
  <button class="icon">☰</button><h1>%s</h1><span class="month">%s</span>
  <div class="search"><input type="search" placeholder="Search people, pets, tags, folders" value=""></div>
  <button class="fav-btn">♡</button>
  <div class="types"><button class="types-btn">Type ▾</button></div>
  <select><option>Year</option></select>
  <button class="icon" style="%s">☑︎</button>
  <select class="lang-pick"><option>English</option></select>
</header>
<div class="layout">
  <nav class="sidebar">
    <div class="sidebar-head"><button class="link">All photos</button> <button class="link">Duplicates</button> <button class="link">Trash</button> <button class="link">Settings</button></div>
    <div class="side-tags"><h3><span>Tags</span></h3><ul class="tree">%s</ul></div>
    <h3 class="side-head"><span>Folders</span></h3>
    <ul class="tree"><li><div class="row"><span class="nm">2024-06 Summer trip</span><span class="ct">6</span></div></li><li><div class="row"><span class="nm">2025-05 Garden party</span><span class="ct">3</span></div></li><li><div class="row"><span class="nm">2025-07 Beach</span><span class="ct">3</span></div></li><li><div class="row"><span class="nm">Family</span><span class="ct">3</span></div></li></ul>
    <footer class="status">1,240 photos · 31 videos · last scan 10/9/2026</footer>
  </nav>
  <main class="scroller">%s%s</main>
</div>%s%s</body></html>""" % (CSS, "selecting" if select_on else "", title, month,
                                "color:var(--accent)" if select_on else "", side_tags, chips, body, selbar, modal)

def dialog(h, inner, actions, cls="wide"):
    return '<div class="modal"><div class="dialog %s" role="dialog"><h2>%s</h2>%s<div class="actions">%s</div></div></div>' % (cls, h, inner, actions)

def selbar(n, export=True):
    return ('<div class="selbar"><span>%s</span><button class="btn">Move…</button><button class="btn">Add tag…</button>'
            '<button class="btn">Remove tag…</button>%s<button class="btn quiet">Done</button></div>'
            % (n, '<button class="btn export">Export…</button>' if export else ""))

def grid(sel=(), extra_badge=None):
    rows = [("July 2025", 3, [0, 1, 2]), ("May 2025", 5, [3, 4, 5, 6, 7]), ("March 2025", 3, [8, 9, 10])]
    out = ""
    for name, n, idx in rows:
        out += '<div class="sech">%s<span class="n">%d</span></div><div class="mgrid">%s</div>' % (name, n, "".join(tile(i, i in sel) for i in idx))
    return out

BTN_C = '<button class="btn quiet">Cancel</button>'

def opts(heic=2, raw=3, vid=1, prefix=True):
    s = ""
    if heic:
        s += '<label class="check"><input type="checkbox" checked> Convert HEIC to JPEG (%d photos)</label><p class="hint opt">For print shops and apps that cannot read HEIC. The JPEG keeps the capture date and the original stays as it is.</p>' % heic
    if raw:
        s += '<label class="check"><input type="checkbox"> Also export the RAW files (%d photos have one)</label><p class="hint opt">Each RAW file keeps the number and name of its picture.</p>' % raw
    if vid:
        s += '<p class="hint" style="margin-top:10px">%d video is included and copied as it is.</p>' % vid
    return s

def place(path, btn="Choose…"):
    return '<div class="row"><label class="lbl">Save in</label><div class="path">%s</div><button class="btn quiet">%s</button></div>' % (path, btn)


def export_dialog(photos, videos, size, parent, name, heic, raw, exists=None, button="Export", disabled=False):
    """ONE dialog for both entry points. From a tag the name is pre-filled with the tag name;
    from a selection it is empty and required."""
    chips = '<div class="sum"><span>%d photos</span>%s<span>%s</span></div>' % (photos, '<span>%d video%s</span>' % (videos, "" if videos == 1 else "s") if videos else "", size)
    if name:
        field = '<input type="text" value="%s" style="flex:1">' % name
        result = 'Creates <b>%s/%s/</b>' % (parent, name)
    else:
        field = '<input type="text" class="req" placeholder="Name of the new folder (required)" style="flex:1">'
        result = 'The new folder is created inside the folder above.'
    number = '<br>Files are numbered in capture order, like <b>001_IMG_3457.jpg</b>. Change this in <b>Settings → Export</b>.'
    inner = chips + place(parent) + '<div class="row"><label class="lbl">Folder name</label>%s</div>' % field
    if exists:
        inner += '<div class="note info"><b>This folder already exists</b> with %d photos.<br>Photos that are already there (same name without the number, same size) are skipped. <b>%d new</b> photos are added. The numbers of all files are updated to the new order.</div>' % exists
        inner += '<div class="result" style="margin-top:10px">%s</div>' % number[4:]
    else:
        inner += '<div class="result">%s%s</div>' % (result, number)
    inner += opts(heic=heic, raw=raw, vid=videos)
    ok = '<button class="btn"%s>%s</button>' % (" disabled" if disabled else "", button)
    return dialog("Export %d photos%s" % (photos, " and %d video%s" % (videos, "" if videos == 1 else "s") if videos else ""), inner, BTN_C + ok)

def mock_files():
    M = {}
    SEL = (3, 4, 5, 6, 7)
    # 01 selection bar
    M["01-selection-bar"] = shell(grid(sel=SEL), select_on=True).replace("</body>", selbar("5 photos") + "</body>")
    # 02 export dialog from a selection: the name is required
    M["02-export-dialog-selection"] = shell(grid(sel=SEL), select_on=True,
        modal=export_dialog(4, 1, "38.2 MB", "/Users/anna/Downloads", "", heic=2, raw=3, disabled=True)).replace("</body>", selbar("5 photos") + "</body>")
    # 03 folder picker, macOS and Windows
    def picker(system):
        if system == "mac":
            places = [("Downloads", True), ("Desktop", False), ("Documents", False), ("Pictures", False), ("anna (home)", False)]
            drives = [("USB-STICK", "29 GB free", False), ("Holiday backup", "shoebox", True), ("Photos", "shoebox", True)]
            crumbs = "<b>/</b> Users › anna › <b>Downloads</b>"
        else:
            places = [("Downloads", True), ("Desktop", False), ("Documents", False), ("Pictures", False), ("Anna (home)", False)]
            drives = [("USB-STICK (E:)", "29 GB free", False), ("Holiday backup (F:)", "shoebox", True), ("Photos (D:)", "shoebox", True)]
            crumbs = "<b>This PC</b> › C: › Users › Anna › <b>Downloads</b>"
        lst = [("📁 Anna wedding", ""), ("📁 invoices", ""), ("📁 print-march", "42 photos"), ("📁 Tickets", "")]
        nav = '<h4>Places</h4>' + "".join('<div class="%s">%s</div>' % ("on" if p[1] else "", p[0]) for p in places)
        nav += '<h4>Drives</h4>' + "".join('<div class="%s">%s<small>%s</small></div>' % ("off" if d[2] else "", d[0], d[1]) for d in drives)
        ul = "".join('<li><span>%s</span><small>%s</small></li>' % (n, c) for n, c in lst)
        body = ('<header><h2>Choose where to save</h2><p class="hint" style="margin:4px 0 0">Drives with a shoebox library are greyed out: copies saved there would be found by the next scan.</p></header>'
                '<div class="pk"><nav>%s</nav><div class="main"><div class="crumbs">%s</div><ul>%s</ul></div></div>'
                '<div class="foot"><span class="grow">Selected: Downloads</span><button class="btn quiet">Cancel</button><button class="btn">Choose this folder</button></div>') % (nav, crumbs, ul)
        return '<div class="modal"><div class="dialog picker" role="dialog">%s</div></div>' % body
    M["03a-folder-picker-mac"] = shell(grid(sel=SEL), select_on=True, modal=picker("mac")).replace("</body>", selbar("5 photos") + "</body>")
    M["03b-folder-picker-windows"] = shell(grid(sel=SEL), select_on=True, modal=picker("win")).replace("</body>", selbar("5 photos") + "</body>")
    # 04 tag chip with the Export button (a normal button, plain label)
    chips = ('<div class="filters"><span class="chip">#&nbsp;print-march <button>✕</button></span><span class="spacer"></span>'
             '<button class="btn" title="Export the 42 photos with this tag">Export…</button></div>')
    tagged = ('<div class="sech">March 2026<span class="n">20</span></div><div class="mgrid">%s</div>'
              '<div class="sech">July 2025<span class="n">22</span></div><div class="mgrid">%s</div>') % (
        "".join(tile(i) for i in (0, 1, 2, 3, 4)), "".join(tile(i) for i in (5, 6, 7, 8, 9)))
    M["04-tag-active-export-button"] = shell(tagged, chips=chips)
    # 05 the same dialog, name pre-filled with the tag
    M["05-export-dialog-tag"] = shell(tagged, chips=chips,
        modal=export_dialog(40, 2, "312 MB", "/Users/anna/Downloads", "print-march", heic=5, raw=0))
    # 06 the folder already exists
    M["06-export-dialog-folder-exists"] = shell(tagged, chips=chips,
        modal=export_dialog(40, 2, "312 MB", "/Users/anna/Downloads", "print-march", heic=5, raw=0, exists=(30, 12), button="Add 12 photos"))
    # 07 progress
    inner = ('<p style="margin:0">Copying <b>17</b> of 42 · IMG_2231.heic → <b>018_IMG_2231.jpg</b></p><div class="bar-progress"><i style="width:40%"></i></div>'
             '<p class="hint" style="margin:0">Every original is read and checked before and after, so this takes a little longer than a plain copy. Your photos are not changed. Files already copied stay in the folder if you stop.</p>')
    M["07-export-progress"] = shell(tagged, chips=chips, modal=dialog("Exporting…", inner, '<button class="btn quiet">Stop</button>'))
    # 08 done
    inner = ('<div class="note good"><b>42 files are in /Users/anna/Downloads/print-march</b><br>Checked against the originals: all identical. The originals on the drive were not changed.</div>'
             '<p class="hint" style="margin:10px 0 0">5 HEIC photos were converted to JPEG · 12 were new · 30 were already there.</p>')
    M["08-export-done"] = shell(tagged, chips=chips, modal=dialog("Export finished", inner, '<button class="btn quiet">Close</button><button class="btn">Show in Finder</button>'))
    # 09 settings
    bm = '<span class="iconbox">%s</span>' % coll(False)
    badge = '<span style="background:var(--accent);color:#fff;border-radius:99px;font-size:11px;padding:1px 8px;vertical-align:middle">new</span>'
    settings = ('<div class="page"><h2>Settings</h2>'
        '<section class="settings-section"><h3>Moving to the trash</h3><label class="check-row"><input type="checkbox"> Allow move to trash</label><p class="hint">Off by default, so shoebox never touches your originals.</p></section>'
        '<section class="settings-section"><h3>Collection %s</h3>'
        '<p class="hint" style="margin-top:0">A quick way to gather photos for something: a print order, an album, a video for a birthday. Type a tag name here and this button %s appears in the photo view (shortcut <span class="kbd">C</span>). One click tags the photo with it, another click removes the tag. Then search for the tag and use <b>Export…</b> to get all of them in one folder. Leave it empty to hide the button.</p>'
        '<div style="display:flex;gap:10px;align-items:center">%s<input type="text" value="print-march" placeholder="e.g. album-italy" style="padding:7px 10px;border-radius:8px;border:1px solid var(--line);background:var(--panel)"> <span class="hint">42 photos have this tag</span></div></section>'
        '<section class="settings-section"><h3>Export %s</h3>'
        '<label class="check-row"><input type="checkbox" checked> Number exported files</label>'
        '<p class="hint">Puts the order of the photos in front of the name: <b>001_IMG_3457.jpg</b>, <b>002_IMG_3501.jpg</b>… The numbers follow the capture date, so a layout program or a print shop sees the same order as the timeline. Off: files keep their name only.</p></section>'
        '<section class="settings-section"><h3>Maps</h3><label class="check-row"><input type="checkbox"> Show maps (OpenStreetMap)</label></section></div>') % (badge, bm, bm, badge)
    M["09-settings"] = shell(settings)
    # 10 photo view with the info panel open: the bookmark and the tag go together
    def lightbox(on, toast):
        t = ('<div class="toast">%s</div>' % toast) if toast else ""
        own = '<span class="own"><button>print-march</button><button class="x">✕</button></span>' if on else ""
        panel = ('<aside class="lb-panel"><dl><dt>Date</dt><dd>Jul 14, 2025, 3:12 PM</dd><dt>Drive</dt><dd>Photos</dd><dt>Path</dt><dd>/2025-07 Beach/IMG_3457.jpg</dd>'
                 '<dt>Size</dt><dd>1600 × 1067 · 133.1 KB</dd><dt>Camera</dt><dd>Sample Camera</dd>'
                 '<dt>Tags</dt><dd><div class="tags"><button>📁 2025-07 Beach</button>%s<button class="add">+ Tag</button></div></dd></dl></aside>') % own
        return ('<div class="lightbox"><div class="stage"><img src="img/p00.jpg" style="max-width:100%%;max-height:100%%"></div>'
                '<div class="lb-bar"><button class="icon">✕</button><span class="lb-title">Jul 14, 2025, 3:12 PM · IMG_3457.jpg</span>'
                '<button class="icon fav-lb">%s</button>'
                '<button class="icon coll%s" title="Add to collection “print-march” (c)">%s</button>'
                '<button class="icon">↺</button><button class="icon">⤓</button><button class="icon">ⓘ</button></div>'
                '<button class="nav prev">‹</button>%s%s</div>') % (HEART, " on" if on else "", coll(on), panel, t)
    for name, on, toast in (("10a-photo-view-collection-off", False, None), ("10b-photo-view-collection-on", True, "Added to “print-march” · 43 photos")):
        M[name] = ("<!doctype html><html><head><meta charset='utf-8'><title>mock</title><link rel='stylesheet' href='../../../core/web/app.css'>%s</head><body>%s</body></html>"
                   % (CSS, lightbox(on, toast)))
    return M

if __name__ == "__main__":
    pictures()
    for k, v in mock_files().items():
        open(os.path.join(HERE, k + ".html"), "w").write(v)
    print("ok")
