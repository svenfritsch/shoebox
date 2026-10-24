#!/usr/bin/env python3
"""Static mock-ups for phase 20 (hidden photos), docs/mocks/hidden/. Not the real app: plain HTML on
core/web/app.css, with the icon paths read from core/web/app.js. `python3 build.py`, then `node shoot.mjs`."""
import os, re, html
HERE = os.path.dirname(os.path.abspath(__file__))
JS = open(os.path.join(HERE, "..", "..", "..", "core", "web", "app.js"), encoding="utf-8").read()

def line_icon(paths, fill=False, sw=2):
    return ('<svg viewBox="0 0 24 24" width="1em" height="1em" style="fill:%s" stroke="currentColor" stroke-width="%s" '
            'stroke-linecap="round" stroke-linejoin="round">%s</svg>') % ("currentColor" if fill else "none", sw, paths)
def js_icon(name):
    m = re.search(r"\b%s: lineIcon\('(.*?)'\)" % name, JS) or re.search(r"var %s = lineIcon\('(.*?)'\)" % name, JS)
    return line_icon(m.group(1))
HEART = line_icon(re.search(r"var HEART_PATH = '(.*?)';", JS).group(1), True)
HEART_OFF = line_icon(re.search(r"var HEART_PATH = '(.*?)';", JS).group(1))
PENCIL = re.search(r"pencil: '(.*?)',\n", JS).group(1)
FOLDER = js_icon("FOLDER_OPEN")
TRASH = js_icon("TRASH_ICON")
# The icon of the face "Ignore" (IGNORE_ICON): the action "hide" and the state "hidden". The same eye without
# its slash is the action "show again".
EYE = js_icon("IGNORE_ICON")
EYE_PLAIN = EYE.replace('<path d="M4.5 20L19.5 4"/>', "")
SEARCH_TEXT = re.search(r"scan: '(.*?)',\n", JS).group(1)

CSS = """<style>
html, body { margin: 0; background: var(--bg); }
body { font: 14px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color: var(--fg); padding: 14px; }
.cap { color: var(--muted); font-size: 12px; margin: 0 0 6px; }
.row2 { display: flex; gap: 10px; flex-wrap: wrap; align-items: center; margin: 0 0 16px; }
.ic { display: inline-flex; width: 1.2em; height: 1.2em; margin-right: 8px; vertical-align: middle; }
.ic svg { width: 100%; height: 100%; }
.suggest { position: static; width: 360px; }
.chip svg { width: 1.1em; height: 1.1em; }
.chip .ficon, .chip .ic { display: inline-flex; width: 1.1em; height: 1.1em; margin: 0; }
.chip .ficon svg { width: 100%; height: 100%; }
.chip button { display: inline-flex; align-items: center; font-size: 13px; cursor: default; }
.chip .edit svg, .chip .eye svg { width: 1.05em; height: 1.05em; }
.chip .edit { color: var(--accent); }
.chip .eye { color: var(--muted); }
.chip .eye.on { color: var(--accent); }
.chip.hid { border-color: var(--accent); }
.chip.hid .lead { color: var(--accent); }
.tree { width: 300px; border: 1px solid var(--line); border-radius: 10px; background: var(--panel); padding: 6px 0; }
.tree .row { padding: 2px 10px 2px 8px; }
.tree .name { flex: 1; text-align: left; background: none; border: 0; color: var(--fg); font: inherit; padding: 4px 8px; border-radius: 6px; }
.tree .count { color: var(--muted); font-size: 12px; font-variant-numeric: tabular-nums; }
.tree .hiddenrow .name { opacity: .5; }
.tree .hiddenrow .count { opacity: .5; }
.tree .hiddenrow .name::after { content: ""; }
.tree .ico { width: 15px; height: 15px; margin-left: 2px; color: var(--muted); opacity: .85; display: inline-flex; }
.tree .ico svg { width: 100%; height: 100%; }
.tree .toggle { width: 22px; }
.tree .name.rc { background: var(--line); }
.menu { position: absolute; }
.tipx { position: absolute; z-index: 60; padding: 4px 10px; border-radius: 8px; background: var(--fg); color: var(--bg); font-size: 13px; font-weight: 500; white-space: nowrap; }
.shot { position: relative; }
.lightbox { position: relative; inset: auto; height: 100%; }
.lb-bar { position: relative; inset: auto; }
.lb-bar .icon { background: none; border: 0; color: #fff; font-size: 20px; padding: 4px 6px; }
.lb-bar .icon svg { display: block; width: 22px; height: 22px; }
.lb-bar .fav-lb.on { color: #ff4d6a; }
.lb-bar .hid-lb.on { color: var(--accent); }
.dark { background: #000; color: #eee; border-radius: 12px; overflow: hidden; }
.lb-panel { position: static; width: auto; background: rgba(24,24,24,.94); padding: 14px 18px 16px; }
.selbar { position: static; transform: none; display: inline-flex; }
.tags .own input, .lb-panel input { background: rgba(255,255,255,.1); color: #eee; border: 1px solid #666; border-radius: 99px; padding: 2px 10px; font: inherit; font-size: 13px; }
.toast { position: absolute; top: 56px; z-index: 40; }
.chip.acts { gap: 2px; }
.chip.acts button { padding: 0 3px; }
.chip.acts.tablet { gap: 4px; }
.chip.acts.tablet button { padding: 0 6px; }
.shotbox { display: inline-block; position: relative; padding-bottom: 36px; }
</style>"""

def page(body, w, h):
    return ('<!doctype html><html><head><meta charset="utf-8"><title>mock</title><meta name="w" content="%d"><meta name="h" content="%d">'
            '<link rel="stylesheet" href="../../../core/web/app.css">%s</head><body><div class="shot">%s</div></body></html>') % (w, h, CSS, body)

def ic(svg): return '<span class="ic">%s</span>' % svg
def item(icon, label, count, at=False):
    return '<div class="item%s"><span class="label">%s<span class="text">%s</span></span><span class="count">%s</span></div>' % (" at" if at else "", icon, label, count)

L = {
 "en": dict(typed="hid", head="Hidden", item="Hidden", rtext="Recognized text", shortcut="Shortcut: type text:hid",
            hide_tip="Hide from timeline (h)", show_tip="Show in timeline (h)", sel_tip="Hide from timeline",
            menu_hide="Hide folder from timeline", menu_show="Show folder in timeline", folder="Folders",
            chip="Hidden", fchip_tip="Hide folder from timeline", ftip_on="Show folder in timeline",
            note="Hidden from the timeline. Marked through the eye button, not a tag.", tag="Add tag…", typed_tag="versteckt",
            fnote="Favorite set. Favorites show as a heart, not as a tag."),
 "de": dict(typed="ausg", head="Ausgeblendet", item="Ausgeblendet", rtext="Erkannter Text", shortcut="Tastenkürzel: text:ausg eingeben",
            hide_tip="Aus der Zeitleiste ausblenden (h)", show_tip="In der Zeitleiste anzeigen (h)", sel_tip="Aus der Zeitleiste ausblenden",
            menu_hide="Ordner aus der Zeitleiste ausblenden", menu_show="Ordner in der Zeitleiste anzeigen", folder="Ordner",
            chip="Ausgeblendet", fchip_tip="Ordner aus der Zeitleiste ausblenden", ftip_on="Ordner in der Zeitleiste anzeigen",
            note="Aus der Zeitleiste ausgeblendet. Erkennbar durch das Auge, kein Tag.", tag="Tag hinzufügen…", typed_tag="versteckt",
            fnote="Favorit gesetzt. Favoriten erscheinen als Herz, nicht als Tag."),
}

def mock_search(t):
    s = '<div class="search" style="width:360px"><input type="search" value="%s" style="width:100%%;box-sizing:border-box;padding:8px 12px;border-radius:12px;border:2px solid var(--accent);background:var(--bg);color:var(--fg);font-size:18px"></div>' % t["typed"]
    d = ('<div class="suggest" style="margin-top:8px"><div class="head">%s</div>%s<div class="head">%s</div>%s'
         '<div style="border-top:1px solid var(--line);margin-top:4px;padding:8px 12px;color:var(--muted);font-size:14px">%s</div></div>') % (
        t["head"], item(ic(EYE), t["item"], 12, True), t["rtext"], item('<span class="ic" style="color:var(--accent)">%s</span>' % SEARCH_TEXT, "“%s”" % t["typed"], 1), t["shortcut"])
    return page(s + d, 392, 300)

def tree(t, menu=None):
    rows = [("08. Freunde", 54, False), ("08.07. Tanzkurs", 70, False), ("08.07.Geburtstag bei Marie", 167, False), ("Privat", 31, True), ("2025-12 Weihnachten", 190, False)]
    out = '<div class="cap">%s</div><ul class="tree" style="margin:0">' % t["folder"].upper()
    for name, n, hid in rows:
        rc = " rc" if menu and name == menu[0] else ""
        out += '<li><div class="row%s"><span class="toggle"></span><button class="name%s">%s</button>%s<span class="count">%s</span></div></li>' % (
            " hiddenrow" if hid else "", rc, name, '<span class="ico">%s</span>' % EYE if hid else "", n)
    out += "</ul>"
    if menu:
        out += '<div class="menu" style="left:%dpx;top:%dpx"><button>%s</button></div>' % (menu[2], menu[3], menu[1])
    return out

def mock_menu(t, hidden):
    if hidden: return page(tree(t, ("Privat", t["menu_show"], 150, 160)), 520, 250)
    return page(tree(t, ("08.07. Tanzkurs", t["menu_hide"], 150, 108)), 520, 250)

def chip(label, icon, extra="", cls=""):
    return '<span class="chip%s">%s%s%s<button>✕</button></span>' % (cls, icon, label, extra)
def mock_chips(t, tablet=False):
    en = t is L["en"]
    foldic = '<span class="ficon">%s</span>' % FOLDER
    pen = '<button class="edit">%s</button>' % PENCIL
    cl = " acts tablet" if tablet else " acts"
    a = chip("08. Freunde", foldic, '<button class="eye">%s</button>%s' % (EYE, pen), cl)
    b = chip("Privat", foldic, '<button class="eye on">%s</button>%s' % (EYE_PLAIN, pen), cl)
    c = '<span class="chip hid"><span class="ic lead" style="margin:0">%s</span>%s<button>✕</button></span>' % (EYE, t["chip"])
    fav = '<span class="chip"><span class="ic" style="margin:0">%s</span>%s<button>✕</button></span>' % (HEART, "Favorites" if en else "Favoriten")
    cap = lambda s: '<div class="cap">%s</div>' % s
    body = (cap(("Tablet spacing. " if tablet else "Desktop spacing. ") + ("Folder opened (normal): the eye with a slash hides it" if en else "Ordner geöffnet (normal): das durchgestrichene Auge blendet ihn aus")) + '<div class="row2">%s</div>' % a +
            cap("Folder opened (hidden): the plain eye shows it again" if en else "Ordner geöffnet (ausgeblendet): das einfache Auge zeigt ihn wieder") + '<div class="row2">%s</div>' % b +
            cap("The Hidden chip (next to the Favorites chip)" if en else "Der Chip „Ausgeblendet“ (neben dem Chip „Favoriten“)") + '<div class="row2">%s%s</div>' % (c, fav))
    return page(body, 520, 220)

def lbbar(t, on):
    btn = lambda cls, svg, extra="": '<button class="icon %s">%s</button>' % (cls, svg)
    return ('<div class="lb-bar"><button class="icon">✕</button><span class="lb-title">IMG_2231.jpg</span>'
            '<button class="icon fav-lb">%s</button><button class="icon hid-lb%s">%s</button><button class="icon">↺</button>'
            '<button class="icon">%s</button><button class="icon">⤓</button><button class="icon">ⓘ</button></div>') % (HEART_OFF, " on" if on else "", EYE_PLAIN if on else EYE, TRASH)

def mock_viewer(t, on):
    tip = t["show_tip"] if on else t["hide_tip"]
    body = '<div class="dark" style="width:560px;height:150px;padding-top:6px">%s</div><div class="tipx" style="left:292px;top:62px">%s</div>' % (lbbar(t, on), tip)
    return page(body, 592, 170)

def mock_panel(t, kind):
    note = t["note"] if kind == "hidden" else t["fnote"]
    en = t is L["en"]
    dl = ("<dl><dt>%s</dt><dd>IMG_2231.jpg</dd><dt>%s</dt><dd>%s</dd><dt>%s</dt><dd>2025-07 Kroatien/IMG_2231.jpg</dd></dl>" %
          ("Name", "Date" if en else "Datum", "Jul 14, 2025" if en else "14.07.2025", "Path" if en else "Pfad"))
    tags = '<div class="tags" style="margin-top:12px"><span class="own"><button>Strand</button><button class="x">✕</button></span><button>+ Tag</button></div>'
    bar = lbbar(t, kind == "hidden")
    if kind == "fav": bar = bar.replace("fav-lb", "fav-lb on").replace(HEART_OFF, HEART)
    body = ('<div class="dark" style="width:480px">%s<div class="lb-panel">%s%s</div></div><div class="toast" style="left:256px;transform:translateX(-50%%)">%s</div>'
            % (bar.replace("IMG_2231.jpg", ""), dl, tags, note))
    return page(body, 512, 250)

def mock_selbar(t):
    names = ["tag", "untag", "date", "geo", "move", "export"]
    btns = "".join('<button class="ib">%s</button>' % js_icon(n) for n in names)
    hid = '<button class="ib" style="background:var(--line)">%s</button>' % EYE
    out = ('<div class="shotbox"><div class="selbar tools"><button class="ib">%s</button><span class="sel-count" style="margin:0 8px">4</span>%s%s%s<span class="sep" style="width:1px;align-self:stretch;background:var(--line);margin:0 4px"></span><button class="ib danger">%s</button></div>'
           '<div class="tipx" style="left:176px;top:62px">%s</div></div>') % (js_icon("close"), btns, hid, "", TRASH, t["sel_tip"])
    return page(out, 540, 120)

def main():
    for lg, t in L.items():
        m = {"01-search": mock_search(t), "02-folder-menu": mock_menu(t, False), "03-hidden-folder-menu": mock_menu(t, True), "04-chips": mock_chips(t), "04b-chips-tablet": mock_chips(t, True),
             "05-viewer-hide": mock_viewer(t, False), "06-viewer-hidden": mock_viewer(t, True), "07-tag-box-hidden": mock_panel(t, "hidden"),
             "08-tag-box-favorite": mock_panel(t, "fav"), "09-selection-bar": mock_selbar(t)}
        for k, v in m.items():
            open(os.path.join(HERE, "%s-%s.html" % (k, lg)), "w", encoding="utf-8").write(v)
main()
