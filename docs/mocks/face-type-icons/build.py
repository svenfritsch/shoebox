# Mock: the "Who is it?" kind picker of "Add face or pet" with our own icons
# instead of emojis. Same CSS as core/web/app.css (.dialog .kinds), same
# lineIcon construction as core/web/app.js. Run: python3 build.py
PERSON = '<circle cx="12" cy="8" r="3.6"/><path d="M4.5 20c.5-4 3.5-6.2 7.5-6.2s7 2.2 7.5 6.2"/>'
CAT = '<path d="M8.5 4.5l1.3 1.8h3.4l1.3-1.8v4.2a3.3 3.3 0 0 1-6 0z"/><path d="M8.7 11.5C6.5 13 5.5 15.5 5.5 18c0 1.2.8 2 2 2h6.5c2.6 0 4.5-1.6 4.5-3.5V8.5"/><path d="M18.5 8.5a1.5 1.5 0 0 1 3 0"/>'
DOG = '<path transform="rotate(-45 12 12) translate(12 12) scale(1.15) translate(-12 -12)" d="M7.19 10H16.81A2.4 2.4 0 1 1 20.39 10Q19.6 12 20.39 14A2.4 2.4 0 1 1 16.81 14H7.19A2.4 2.4 0 1 1 3.61 14Q4.4 12 3.61 10A2.4 2.4 0 1 1 7.19 10z"/>'
def icon(p, sw=2):
    return ('<svg viewBox="0 0 24 24" width="1em" height="1em" aria-hidden="true" style="fill:none" stroke="currentColor" '
            f'stroke-width="{sw}" stroke-linecap="round" stroke-linejoin="round">{p}</svg>')
html = f'''<!doctype html><meta charset="utf-8"><style>
:root {{ --bg:#161616; --fg:#ececec; --line:#2c2c2e; --accent:#e0a46d; }}
body {{ margin:0; padding:20px; background:var(--bg); color:var(--fg); font:16px -apple-system,system-ui,sans-serif; width:960px }}
h3 {{ font-weight:400; font-size:20px; margin:0 0 16px }}
.dialog .kinds {{ display:flex; margin:0 0 12px }}
.dialog .kinds button {{ flex:1; padding:7px 8px; border:1px solid var(--line); background:var(--bg); color:var(--fg); font-size:14px; font-family:inherit }}
.dialog .kinds button:first-child {{ border-radius:8px 0 0 8px }}
.dialog .kinds button:last-child {{ border-radius:0 8px 8px 0 }}
.dialog .kinds button + button {{ border-left:0 }}
.dialog .kinds button.on {{ background:var(--accent); border-color:var(--accent); color:#fff; font-weight:600 }}
.dialog .kinds svg {{ width:1.25em; height:1.25em; vertical-align:-.25em; margin-right:.35em }}
.gap {{ height:28px }}
</style><div class="dialog">
<h3>Who is it?</h3>
<div class="kinds"><button class="on">{icon(PERSON)}Person</button><button>{icon(CAT,1.8)}Cat</button><button>{icon(DOG,1.8)}Dog</button></div>
<div class="gap"></div>
<h3>Who is it? <small style="color:#9a9a9f;font-size:13px">(Cat selected)</small></h3>
<div class="kinds"><button>{icon(PERSON)}Person</button><button class="on">{icon(CAT,1.8)}Cat</button><button>{icon(DOG,1.8)}Dog</button></div>
</div>'''
open('mock.html', 'w').write(html)
