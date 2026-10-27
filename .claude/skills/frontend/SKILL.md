---
name: frontend
description: Build or change the UI of the shoebox photo app or the Control Panel (core/web, core/launcher-web, core/i18n): a screen, dialog, control, icon, style, UI text, keyboard or touch behaviour, or what the page asks the server for. Covers the common controls and design tokens, icons, texts in both languages, touch and keyboard, how to look at the page without building shoebox, and which browser tests to write. Use before editing app.js, app.css, index.html, the launcher pages or a UI string.
---

# Front end of shoebox

The photo app is one page with no build step and no framework: `core/web/` is
served as it is on disk (and embedded in the binary). It is used on a Mac or PC
with a mouse and on an iPad by touch, in light and dark. The photos are the
content; everything around them is quiet. A change is good when it looks like it
was always there: same controls, same words, same behaviour as the screens
next to it.

## 1. Before you write code

1. **Find the place and its twins.** `app.js` is about 9,400 lines of globals.
   Get its map with `grep -n '^// ----' core/web/app.js` and read the whole
   section you change. Then grep for the same thing done elsewhere: the i18n
   key, the class name, the endpoint (`grep -n "'/suggest'" core/web/*.js`). A
   behaviour usually exists twice (one drive and All drives, timeline and a
   person's page, the photo app and the Control Panel); change both or say why not.
2. **Name the existing control** you will use (section 3). A new class is the
   exception and goes into the common block.
3. **Say the tests** (section 7) and **the guide sections** (the `guide` skill)
   before building, in the plan or the first reply.

| File | What |
|---|---|
| `core/web/index.html` | the page's skeleton, header, sidebar, dialogs that always exist |
| `core/web/app.js` | everything else: filters in the URL hash, search box, timeline, viewer, selection, pages (duplicates, trash, faces, settings), several drives |
| `core/web/app.css` | all styles; tokens at the top, then the common controls |
| `core/web/dates.js`, `geo.js`, `export.js`, `tip.js` | self-contained parts: estimated dates, maps and places, the export dialog, the quick label (tooltip) |
| `core/web/vendor/` | Leaflet (vendored; a change means `python3 scripts/third-party-licenses.py`) |
| `core/launcher-web/` | the Control Panel (`index.html`, `launcher.js`, `launcher.css`) |
| `core/i18n/en.json`, `de.json`, `i18n.js` | every text the pages show |

New self-contained code goes into its own script like `dates.js`, not deeper
into `app.js`.

## 2. Tokens: colour, size, type

Use the custom properties at the top of `app.css`; never write a colour, a
control height or a radius as a number in a rule.

- Colours: `--bg`, `--fg`, `--muted`, `--line`, `--panel`, `--cell-bg`,
  `--accent` (warm brown), `--accent-soft` / `--accent-soft-h` (translucent
  accent for an "on" or hovered control), `--accent-fg` (accent text),
  `--ok` / `--ok-fg`, `--bad` / `--bad-fg`. The dark theme redefines them under
  `prefers-color-scheme: dark`, so a rule that only uses tokens is right in both.
- Sizes: `--ctl-h` (32px; 44px for `pointer: coarse`), `--ctl-h-sm` (26px; 34px
  coarse), `--ctl-r` (radius), `--badge-h` (18px, a label lying on a photo),
  `--bar-h`, `--side-w`.
- Type: the system font at 15px/1.4 from `body`; smaller text is 13 or 12px
  in `--muted`. No web fonts (the app must work offline), no new families.
- The accent marks state (selected, on, focused, the current year), not
  decoration. No gradients, no shadows on cards, no entrance animations.
  Motion only answers an action (opening, a change in place), and every
  animation or transition has a `prefers-reduced-motion: reduce` rule that
  turns it off.

## 3. The common controls

Controls take their size, radius, hover and focus from the "common controls"
part of `app.css`. Look for the class that already is your control and use it:

| Class | For |
|---|---|
| `.btn` (`.quiet`, `.small`, `.danger`: red with white words) | text buttons |
| `.icon` | icon buttons of the header and the viewer bar |
| `.frame-icon` | a framed square icon button (the header's heart and Select) |
| `.ib` | tools of the selection bar |
| `.abtn` | face actions |
| `.more-btn`, `.icon-btn`, `.sel-all` | the ⋯ button, small icon buttons, "Select all" |
| `.tabs` | every row of tabs: a `.tabs` box of buttons, the current one `.on` or `.active` |
| `.badge-chip` (`.add` for the dashed "+ add") | tags and face names |
| `.badge-input` | the small field that types one |
| `.chip` | filters in the search row |
| `.text-input` | text fields (the search box, the settings' fields) |
| plain `select` (`select.small` is `--ctl-h-sm` high) | drop-downs; the shared chevron is already there |
| `.scrub` | the year strip right of the timeline |

Rules:
- Pills have two heights only: `--ctl-h-sm` among text and controls,
  `--badge-h` for a label lying on a photo (`.badge`, the text snippet).
- Text buttons stay on one line, cut with an ellipsis; `tip.js` shows the
  whole label. Set a tooltip with `setTip` or `data-i18n-tip`.
- No per-screen heights, paddings, radii or colours for something that already
  exists. A new kind of control gets its class in the common block (with a
  tooltip or ellipsis rule if its label can be long) and a row in this table
  and in the `CLAUDE.md` rule.
- Menus are `showMenu`, short notes are `toast`; dialogs keep the focus inside
  (Tab) and give it back when they close (`keyboard.mjs` checks this).

## 4. Icons

Icons are our own line drawings: `lineIcon(paths)` in `app.js` (24 px box,
`currentColor`, stroke 2, round ends). Draw the path yourself; never paste an
icon set's SVG or path data, never trace one, and make it visibly different
from Material Symbols, Feather, SF Symbols and the like (other shapes,
proportions, stroke ends, where parts sit). Plain everyday shapes (a cross, a
plus, a trash can, a map marker, a tray with an arrow) may look like theirs as
long as the path is ours. Reuse an existing drawing when the meaning is the
same (`grep -n "_ICON = lineIcon" core/web/app.js`). No emojis anywhere in the
pages (`core/tests/no_emoji.rs`); plain symbols (✓ ✗ ✕ ♥ ☰ ↺ ⇄) may stay.

## 5. Words

- Every text is `tr('area.key', {n})` / `trn()` / `data-i18n` (also
  `data-i18n-title`, `-placeholder`, `-aria-label`, `-tip`), with the key in
  both `en.json` and `de.json` (`core/tests/i18n.rs` fails otherwise, and on a
  key nobody uses). German uses the informal "du". Dates and numbers through
  `I18n.date` / `I18n.number`.
- Name things by what the person sees, not by the code: "Unnamed", not
  "clusters"; "photo app", "Control Panel". Take the words the screens already
  use (grep both JSON files) and keep one word per idea in each language.
- An action keeps its name through the flow: the button "Move…", the dialog
  "Move 3 photos", the note "Moved". Buttons say what happens ("Delete the new
  copies"), not "OK" or "Submit".
- A message about a problem says what happened and what to do next, plainly,
  without apologising. An empty page says why it is empty and how to fill it.
- Sentence case, no exclamation marks, no filler. Texts about hiding say it
  only keeps photos out of the timeline (it is not privacy).

## 6. Touch, keyboard and the server

- **Touch**: nothing may need hover. Controls shown on hover with a mouse are
  always shown under `(hover: none)` or `(pointer: coarse)` (the corner heart
  of a cell). Targets are at least `--ctl-h` on touch. Drag and drop is for a
  mouse; there is always a menu way ("Move to group…").
- **Keyboard**: every control is reachable with Tab and has the shared
  `:focus-visible` ring; shortcuts are single keys listed in the guide (check
  the free ones first: `grep -n "e.key ===" core/web/app.js`).
- **Requests**: the server has one connection per drive, so the page asks
  only on actions, never polls (except `/api/info` while a job runs), calls off
  what it no longer needs (`AbortController`, `dropPending`, `clearNode`),
  and grows long lists as they are scrolled to (`growGrid`). A change that
  adds a request is a change to the tests (section 7) and maybe to the fake.
- **Read-only and other states**: a library kept next to the program hides
  everything that changes an original (`body.read-only`); All drives is
  read-only; trash controls only show with `allow_trash`. Check your control
  in those states too.

## 7. Look at it, then test it

**Look at the page** without building shoebox: `look.mjs` serves `core/web` as
it is on disk against the fake API with a few photos, folders, people and cards,
and writes a screenshot for desktop light, desktop dark and an iPad (touch):

```sh
NODE_PATH=$(npm root -g) node core/tests/browser/look.mjs '' <scratch dir>
NODE_PATH=$(npm root -g) node core/tests/browser/look.mjs 'view=unnamed' <scratch dir>
LOOK_LANG=de LOOK_JS="document.querySelector('#select').click()" NODE_PATH=$(npm root -g) node core/tests/browser/look.mjs '' <scratch dir>
```

Open the PNGs and look at them before and after the change (`APP_JS=<old copy
of app.js>` shows the page as it was): alignment with the controls beside it,
the dark theme, the iPad's bigger controls, long German labels
(`LOOK_LANG=de`). The hash is what the tests use (`view=unnamed`,
`view=people`, `view=settings`, `tag=1`, ...). It prints
page errors and requests the fake does not know. Screenshots go to the
scratchpad, never into git. Pages the fake does not serve need the real server
(`core/tests/browser/run.sh`, which needs a release build).

**Tests** (rules in `CLAUDE.md` and "Browser tests" in `docs/plan.md`):
- What the page does on its own (when it asks, what it calls off, what it
  shows from a reply, a key, a toggle): a test in
  `core/tests/browser/faces-mocked.mjs` on the fake clock, and the fake
  (`fake-api.mjs`) answering as the server does.
- A new or changed endpoint or reply: a Rust API test first, then the fake.
- Keyboard use of the grid, viewer, selection bar and dialogs:
  `keyboard.mjs` (real server).
- Show that each new test fails with the change broken (`APP_JS=`), and run it
  a few times. No fixed sleeps.
- `NODE_PATH=$(npm root -g) node core/tests/browser/faces-mocked.mjs` needs no
  build (`ONLY=<words>` runs only the tests whose name has them).

## 8. Before the pull request

- Tokens and common classes only; no one-off sizes or colours; dark theme and
  touch looked at.
- Every new text in both JSON files; `core/tests/i18n.rs` and
  `core/tests/no_emoji.rs` pass (`cargo test --test i18n --test no_emoji`).
- Tests from section 7 written, seen failing without the change.
- The guide updated (the `guide` skill) or stated as not affected.
- A new control class added to the table in section 3 and to `CLAUDE.md`.
