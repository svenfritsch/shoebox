// Look at the photo app without building shoebox: core/web as it is on disk,
// against the fake API (fake-api.mjs) with a few photos, folders, tags, people
// and a card on the Unnamed page. Writes one screenshot per look: light and
// dark, a desktop window and an iPad (touch, so the bigger controls of
// `pointer: coarse`). Not a test; for seeing a change to the page (see the
// `frontend` skill).
//
//   NODE_PATH=$(npm root -g) node core/tests/browser/look.mjs [hash] [out dir]
//   NODE_PATH=$(npm root -g) node core/tests/browser/look.mjs 'view=unnamed' /tmp/look
//
// `hash` is what follows `#` in the page's address (default: the timeline);
// LOOK_LANG=de: the German texts; LOOK_JS: a script run in the page before the
// screenshots (open a dialog, select photos, ...). APP_JS=<file> serves another
// app.js, as in the tests.
import { browser } from './faces-common.mjs';
import { fakeLibrary, ORIGIN } from './fake-api.mjs';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';

const hash = process.argv[2] || '';
const out = process.argv[3] || 'look';
mkdirSync(out, { recursive: true });

function fill(lib) {
  lib.photos = Array.from({ length: 60 }, (_, i) => ({ id: i + 1, version: String(i + 1).padStart(8, '0'), day: 20260315 - Math.floor(i / 12) * 100, folder: 1 + (i % 3) }));
  lib.folders = [
    { id: 1, parent: null, name: '2026-03 Holiday', path: '2026-03 Holiday', count: 20 },
    { id: 2, parent: null, name: 'Family', path: 'Family', count: 20 },
    { id: 3, parent: 2, name: 'Christmas', path: 'Family/Christmas', count: 20 },
  ];
  lib.tags = [{ id: 1, name: 'Beach', count: 12, kind: 'user' }, { id: 2, name: 'Family', count: 20, kind: 'folder' }];
  const anna = lib.person('Anna');
  lib.group('Family', anna);
  lib.person('Ben', { faces: 2, group: lib.groups[0] });
  lib.cluster(4, anna);
  lib.cluster(3);
}

const looks = [
  { name: 'desktop-light', viewport: { width: 1280, height: 800 }, colorScheme: 'light' },
  { name: 'desktop-dark', viewport: { width: 1280, height: 800 }, colorScheme: 'dark' },
  { name: 'ipad-light', viewport: { width: 1024, height: 768 }, colorScheme: 'light', hasTouch: true, isMobile: true },
];

for (const look of looks) {
  const { name, ...opts } = look;
  const ctx = await browser.newContext(opts);
  await ctx.addInitScript((lang) => { try { localStorage.setItem('shoebox.lang', lang); } catch (e) {} }, process.env.LOOK_LANG || 'en');
  const p = await ctx.newPage();
  const lib = fakeLibrary();
  fill(lib);
  const errors = [];
  p.on('pageerror', (e) => errors.push(e.message));
  await p.route(ORIGIN + '/**', lib.route);
  await p.route('https://tile.openstreetmap.org/**', (r) => r.abort());
  await p.goto(`${ORIGIN}/#${hash}`);
  await p.waitForFunction(() => !!(window.people && people.info && people.list));
  await p.waitForLoadState('networkidle');
  if (process.env.LOOK_JS) { await p.evaluate(process.env.LOOK_JS); await p.waitForLoadState('networkidle'); }
  // Two frames, so what the last replies changed is drawn.
  await p.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  const file = join(out, `${name}.png`);
  await p.screenshot({ path: file });
  const coarse = await p.evaluate(() => matchMedia('(pointer: coarse)').matches);
  console.log(`${file}${coarse ? ' (pointer: coarse)' : ''}${errors.length ? '  page errors: ' + errors.join(' | ') : ''}`);
  if (lib.unknown.length) console.log(`  the fake does not know: ${lib.unknown.join(', ')}`);
  await ctx.close();
}
await browser.close();
