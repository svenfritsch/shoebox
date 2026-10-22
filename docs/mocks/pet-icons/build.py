# Mocks: the pet icon moved out of the avatar (above the circle) and the
# candidate icons for the remaining emojis. Our own line drawings, same
# construction as lineIcon() in core/web/app.js. Run: python3 build.py, then
# render.mjs (Playwright) makes the PNGs.
def svg(p, sw=1.8, fill=False):
    return ('<svg viewBox="0 0 24 24" width="1em" height="1em" aria-hidden="true" style="fill:%s" stroke="currentColor" '
            'stroke-width="%s" stroke-linecap="round" stroke-linejoin="round">%s</svg>' % ('currentColor' if fill else 'none', sw, p))
CAT = '<path d="M8.5 4.5l1.3 1.8h3.4l1.3-1.8v4.2a3.3 3.3 0 0 1-6 0z"/><path d="M8.7 11.5C6.5 13 5.5 15.5 5.5 18c0 1.2.8 2 2 2h6.5c2.6 0 4.5-1.6 4.5-3.5V8.5"/><path d="M18.5 8.5a1.5 1.5 0 0 1 3 0"/>'
DOG = '<path transform="rotate(-45 12 12) translate(12 12) scale(1.15) translate(-12 -12)" d="M7.19 10H16.81A2.4 2.4 0 1 1 20.39 10Q19.6 12 20.39 14A2.4 2.4 0 1 1 16.81 14H7.19A2.4 2.4 0 1 1 3.61 14Q4.4 12 3.61 10A2.4 2.4 0 1 1 7.19 10z"/>'
PAW = '<circle cx="5.5" cy="11.5" r="1.7" fill="currentColor"/><circle cx="9.3" cy="7" r="1.7" fill="currentColor"/><circle cx="14.7" cy="7" r="1.7" fill="currentColor"/><circle cx="18.5" cy="11.5" r="1.7" fill="currentColor"/><path d="M8.3 17.5c0-2.2 1.7-4 3.7-4s3.7 1.8 3.7 4c0 1.5-1 2.2-2.2 2.2-.6 0-1-.3-1.5-.3s-.9.3-1.5.3c-1.2 0-2.2-.7-2.2-2.2z"/>'
PLACE = '<path d="M12 21C9.5 18.2 6 14.8 6 10.8a6 6 0 0 1 12 0c0 4-3.5 7.4-6 10.2z"/><circle cx="12" cy="10.8" r="1.6"/>'
TRASH = '<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v6M14 11v6"/>'
# new candidates
MAP_FOLD = '<path d="M3.5 6.5l5-2 7 2.5 5-2v12.5l-5 2-7-2.5-5 2z"/><path d="M8.5 4.5v12.5M15.5 7v12.5"/>'
DRIVE_BOX = '<rect x="3" y="7" width="18" height="10" rx="3"/><path d="M7 14h6"/><circle cx="17" cy="14" r=".7" fill="currentColor"/>'
DRIVE_DISC = '<rect x="4" y="4" width="16" height="16" rx="3.5"/><circle cx="12" cy="11" r="3.6"/><circle cx="12" cy="11" r=".6" fill="currentColor"/><path d="M8 17.2h2"/>'
PENCIL1 = '<path d="M5 19l1-4L16.5 4.5a2.1 2.1 0 0 1 3 3L9 18z"/><path d="M14.5 6.5l3 3"/>'
PENCIL2 = '<path d="M4 19.5h16"/><path d="M6.5 16.5l.7-3.2 8.3-8.3a2 2 0 0 1 2.8 2.8l-8.3 8.3z"/>'
RENAME = '<rect x="3" y="7" width="18" height="10" rx="2.5"/><path d="M12 10v4M10.3 10h3.4M10.3 14h3.4"/>'
WARN_TRI = '<path d="M12 4.5l8.5 15h-17z"/><path d="M12 10.5v4"/><circle cx="12" cy="17" r=".6" fill="currentColor"/>'
WARN_FRAME = '<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><path d="M12.5 4.5l-2 5 3 2-2.5 4 1 4"/>'
WARN_IMG = '<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><circle cx="9" cy="10" r="1.5"/><path d="M4 17l5-4.5 3.5 3 2.5-2L20 17"/><path d="M4 20L20 4" />'

CSS = '''
:root{--bg:#161616;--fg:#ececec;--line:#2c2c2e;--panel:#1e1e1f;--accent:#e0a46d;--muted:#9a9a9f;--pet:#5ac8fa}
*{box-sizing:border-box}
body{margin:0;padding:24px;background:var(--bg);color:var(--fg);font:16px -apple-system,system-ui,"Segoe UI",sans-serif;width:__W__px}
svg{width:1em;height:1em;vertical-align:-.12em}
.card{background:var(--panel);border:1px solid var(--line);border-radius:14px;padding:18px 20px}
h4{margin:0 0 14px;font-size:17px}
.grid{display:grid;grid-template-columns:repeat(4,150px);column-gap:12px;row-gap:14px}
.pet{display:flex;flex-direction:column;align-items:center}
.pet .av{width:150px;height:150px;border-radius:50%;box-shadow:0 0 0 2px var(--pet);background-size:cover}
.more{width:146px;height:146px;border-radius:50%;border:1.5px dashed #555;color:var(--muted);display:flex;align-items:center;justify-content:center;margin:2px}
.sp{height:34px;display:flex;align-items:flex-end;justify-content:center;font-size:22px;color:var(--pet)}
.sp.B{height:34px}
.sp.B span,.sp.C span{width:34px;height:34px;border-radius:50%;display:flex;align-items:center;justify-content:center}
.sp.B span{background:rgba(22,22,22,.72);box-shadow:0 0 0 1.5px var(--pet);margin-bottom:-15px;position:relative;z-index:2;backdrop-filter:blur(4px)}
.sp.C span{background:rgba(90,200,250,.16);box-shadow:0 0 0 1.5px rgba(90,200,250,.6);margin-bottom:6px}
.sp.A svg{width:26px;height:26px}
.sp.B svg,.sp.C svg{width:22px;height:22px}
.cap{color:var(--muted);font-size:13px;margin:0 0 10px}
.head{display:flex;gap:26px;align-items:center}
.head .av{width:150px;height:150px;border-radius:50%;box-shadow:0 0 0 3px var(--pet);background-size:cover}
.head h2{margin:0;font-size:40px;display:flex;align-items:center;gap:14px}
.head h2 svg{width:36px;height:36px;color:var(--pet)}
.head .sub{color:var(--muted);font-size:22px;margin-top:6px}
.chips{display:flex;gap:12px;margin-top:22px}
.chip{display:flex;align-items:center;gap:12px;border:1px solid #3a3a3c;border-radius:30px;padding:10px 18px;font-size:22px;background:#1b1b1c}
.chip svg{width:24px;height:24px}
.chip.plus{padding:10px 22px;color:var(--accent)}
.table{display:grid;grid-template-columns:200px 120px 1fr 1fr;gap:0;font-size:15px}
.table>div{padding:12px 10px;border-bottom:1px solid var(--line);display:flex;align-items:center;gap:14px}
.table .h{color:var(--muted);font-size:13px}
.table svg{width:30px;height:30px}
.emo{font-size:26px}
.opt{display:flex;align-items:center;gap:8px;color:var(--fg)}
.opt small{color:var(--muted)}
.tag{font-size:12px;color:var(--accent)}
'''
def page(body, w):
    return '<!doctype html><meta charset="utf-8"><style>%s</style>%s' % (CSS.replace('__W__', str(w)), body)

def grid(variant):
    cells = ''
    for i in range(8):
        cells += '<div class="pet"><div class="sp %s"><span>%s</span></div><div class="av" style="background-image:url(pet%d.jpg)"></div></div>' % (
            variant, svg(CAT), i) if variant != 'A' else '<div class="pet"><div class="sp A">%s</div><div class="av" style="background-image:url(pet%d.jpg)"></div></div>' % (svg(CAT), i)
    cells += '<div class="pet"><div class="sp"></div><div class="more">+2</div></div>'
    return '<div class="card"><h4>10 pets</h4><div class="grid">%s</div></div>' % cells

open('grid-A.html', 'w').write(page(grid('A'), 760))
open('grid-B.html', 'w').write(page(grid('B'), 760))
open('grid-C.html', 'w').write(page(grid('C'), 760))
open('header.html', 'w').write(page(
 '<div class="card"><div class="head"><div class="av" style="background-image:url(layka.jpg)"></div><div><h2>%s Layka</h2><div class="sub">53 photos · Haustiere</div></div></div>'
 '<div class="chips"><div class="chip">%s Layka <span style="color:#aaa">✕</span></div><div class="chip plus">+</div></div></div>' % (svg(DOG), svg(PAW, 1.8)), 760))

def row(emo, where, cur, opts):
    o = ''.join('<div class="opt">%s<small>%s</small></div>' % (svg(p, 1.8, f), n) for p, n, f in opts)
    return '<div><span class="emo">%s</span></div><div>%s</div><div>%s</div><div style="gap:28px">%s</div>' % (emo, where, cur, o)
rows = [
 row('🐱', 'Pet badge, tags, names', 'cat icon (have)', [(CAT, 'cat', False)]),
 row('🐶', 'same', 'bone (have)', [(DOG, 'dog', False)]),
 row('🐾', 'generic pet', 'paw (have)', [(PAW, 'paw', False)]),
 row('🗑', 'Delete place', 'TRASH_ICON (have)', [(TRASH, 'trash', False)]),
 row('🗺', 'Map button on a place chip', 'new', [(MAP_FOLD, 'A folded map', False), (PLACE, 'B marker (have; chip already uses it)', False)]),
 row('💾', 'Drive row', 'new', [(DRIVE_BOX, 'A flat external drive', False), (DRIVE_DISC, 'B drive, front view', False)]),
 row('✎', 'Edit chip', 'new', [(PENCIL1, 'A pencil', False), (PENCIL2, 'B pencil on a line', False)]),
 row('✎', 'Rename place', 'new', [(RENAME, 'A text field (rename)', False), (PENCIL1, 'B same pencil as the chip', False)]),
 row('⚠︎', 'Broken thumbnail', 'new', [(WARN_TRI, 'A warning triangle', False), (WARN_IMG, 'B photo, struck through', False)]),
]
hdr = '<div class="h">Emoji today</div><div class="h">Where</div><div class="h">Icon</div><div class="h">Options</div>'
open('options.html', 'w').write(page('<div class="card"><div class="table">%s%s</div></div>' % (hdr, ''.join(rows)), 1100))
