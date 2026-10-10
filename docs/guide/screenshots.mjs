// Screenshots for the guide. usage: node screenshots.mjs <app|launcher> <lang> <outdir> <url> <library-path>
// Needs Playwright with a Chromium (CHROME=path) and a freshly generated and
// recognised sample library served by `shoebox serve` (see screenshots.sh).
import { createRequire } from 'node:module';
const { chromium } = createRequire(import.meta.url)('playwright'); // honours NODE_PATH
const [mode, lang, out, url, libPath] = process.argv.slice(2);
const b = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {});
const ctx = await b.newContext({ viewport: { width: 1280, height: 860 }, deviceScaleFactor: 2 });
await ctx.addInitScript((l) => { try { localStorage.setItem('shoebox.lang', l); } catch (e) {} }, lang);
const p = await ctx.newPage();
const de = lang === 'de';
// Not 'Favorites': that word is the heart feature now.
const TAG = 'Highlights';
const HOLIDAY = de ? '2025-08 Urlaub' : '2025-08 Holiday';
const shot = (name, opts = {}) => p.screenshot({ path: `${out}/${name}.png`, ...opts });
const wait = (ms = 600) => p.waitForTimeout(ms);

if (mode === 'launcher') {
  await p.setViewportSize({ width: 760, height: 1100 });
  await p.goto(url); await wait(1200);
  await p.fill('#root', libPath); await p.keyboard.press('Enter'); await wait(600);
  // Show typical drive names instead of whatever this machine has.
  await p.evaluate(() => { var c = document.querySelectorAll('#drive-chips > *'); ['Photos', 'Backup'].forEach((n, i) => { if (c[i]) c[i].textContent = n; }); for (var i = 2; i < c.length; i++) c[i].remove(); });
  await p.click('button[data-kind=scan]');
  await p.waitForSelector('#summary:not(:empty)', { timeout: 60000 }); await wait(1500);
  // Show shoebox on the drive (the folder it was copied to) instead of this machine's path.
  await p.evaluate((lib) => { var h = document.getElementById('addons-hint'); h.textContent = h.textContent.replace(/\/\S*recognizer/, lib + '/shoebox-macos/recognizer'); }, libPath);
  await shot('launcher-overview', { fullPage: true });
  // A photo copied onto the drive by hand, then a second scan: the scan lists
  // it as already there and offers to delete the new copy.
  const { readdirSync, mkdirSync, copyFileSync } = await import('node:fs');
  const walk = (d) => readdirSync(d, { withFileTypes: true }).flatMap((e) => e.name.startsWith('.') ? [] : e.isDirectory() ? walk(`${d}/${e.name}`) : /\.jpe?g$/i.test(e.name) ? [`${d}/${e.name}`] : []);
  const src = walk(libPath).sort()[0];
  const dir = `${libPath}/${de ? '2025-09 Neue Fotos' : '2025-09 New photos'}`;
  mkdirSync(dir, { recursive: true });
  copyFileSync(src, `${dir}/${src.split('/').pop()}`);
  await p.click('button[data-kind=scan]');
  await p.waitForSelector('#arrivals-box:not([hidden])', { timeout: 60000 }); await wait(1500);
  await p.locator('#progress-card').screenshot({ path: `${out}/scan-copies.png` });
  await b.close(); process.exit(0);
}

await p.goto(url); await p.waitForSelector('.cell'); await wait(1000);
// Moving to the trash is off until it is allowed in the settings; the guide shows it on.
await p.evaluate(() => post(LIBAPI + '/allow-trash', { allow: true }));
await p.goto(url); await p.waitForSelector('.cell'); await wait(2500);
await shot('ui-overview');

// Viewer with the info panel (a landscape; newest first: 3 July, 3 May, then March).
await p.locator('.cell').nth(6).click(); await wait(800);
await p.keyboard.press('i'); await wait(800);
await shot('viewer-info');
await p.keyboard.press('Escape'); await wait(300);

// Duplicates, from a clean state; tall so the whole page is in the shot.
await p.click('#nav-dups'); await wait(7000);
await p.setViewportSize({ width: 1280, height: 1500 }); await wait(800);
await shot('duplicates');
await p.setViewportSize({ width: 1280, height: 860 });
await p.click('#all'); await wait(800);

// People and pets: the cards waiting for a name.
await p.click('#nav-people'); await wait(1200);
await p.locator('#page .link').first().click(); await wait(2500);
await shot('people-unnamed');

// "Select" on a card: tick some faces, then name, ignore or mark only those.
const card = p.locator('#un-box .cluster').first();
await card.locator('button.btn.quiet').nth(2).click(); await wait(500);
await card.locator('.cface').nth(1).click(); await card.locator('.cface').nth(3).click(); await wait(300);
await card.locator('input[type=text]').fill('Mia'); await wait(300);
await card.screenshot({ path: `${out}/faces-select.png` });
await card.locator('.cclose').click(); await wait(300);
await p.click('#all'); await wait(800);

// The same in the info panel of one photo: two passers-by, ticked to ignore them.
await p.getByText(/^2023-11/).first().click(); await wait(1500);
await p.locator('.cell').nth(1).click(); await wait(1500);
// The info panel stays open from the earlier photo; 'i' would close it.
if (!(await p.locator('.pfaces:visible').count())) { await p.keyboard.press('i'); await wait(1500); }
await p.locator('.pfaces:visible .ptools button').nth(1).click(); await wait(500);
await p.locator('.pfaces:visible .pface.pickable input').nth(0).check(); await p.locator('.pfaces:visible .pface.pickable input').nth(1).check(); await wait(500);
await shot('info-select');
await p.locator('.pfaces:visible .pickacts button').nth(2).click(); await wait(300);
await p.keyboard.press('Escape'); await wait(300);
await p.click('#all'); await wait(800);

// Name everybody (the people in the drawn photos), make groups, put them in.
// Two strangers and the framed picture stay unnamed on purpose.
const GROUPS = de ? { family: 'Familie', friends: 'Freunde', colleagues: 'Kollegen', pets: 'Haustiere' } : { family: 'Family', friends: 'Friends', colleagues: 'Colleagues', pets: 'Pets' };
await p.evaluate(async (g) => {
  const WHO = { '2023-08': ['Mia', 'Rosa', 'Ben'], '2023-09': ['Lena', 'Jonas', 'Sam'], '2023-10': ['Priya', 'Marco', 'Chen'], '2025-05': ['Mia'], '2025-07': ['Mia'] };
  const ids = {};
  for (const kind of ['faces', 'pets']) {
    const res = await api(LIBAPI + '/clusters?kind=' + kind);
    for (const c of res.clusters) {
      let name = null;
      if (kind === 'pets') name = c.size > 5 ? 'Whiskers' : 'Buddy';
      else {
        const f = c.faces[0];
        const info = await api(LIBAPI + '/files/' + f.file);
        const row = WHO[info.path.slice(0, 7)];
        if (row) name = row[Math.min(info.faces.map((x) => x.x).sort((a, b) => a - b).indexOf(f.x), row.length - 1)];
      }
      if (!name) continue;
      const d = await post(LIBAPI + '/clusters/' + c.id + '/name', { generation: c.generation, name });
      ids[name] = d.person.id;
    }
  }
  const members = { [g.family]: ['Rosa', 'Ben'], [g.friends]: ['Lena', 'Jonas', 'Sam'], [g.colleagues]: ['Priya', 'Marco', 'Chen'], [g.pets]: ['Whiskers', 'Buddy'] };
  for (const [name, who] of Object.entries(members)) {
    const grp = await post(LIBAPI + '/groups', { name });
    for (const w of who) await post(LIBAPI + '/people/' + ids[w] + '/group', { group_id: grp.id });
  }
}, GROUPS);
await p.goto(url); await p.waitForSelector('.cell'); await wait(2000);
await p.click('#nav-people'); await wait(2000);
await p.setViewportSize({ width: 1280, height: 1020 }); await wait(500);

// Groups: the list with rename, order and delete ...
await p.locator('#page button.btn.quiet').nth(1).click(); await wait(600);
await shot('groups-dialog');
await p.locator('#modal-actions button').last().click(); await wait(600);
// ... and putting a person into one from the person's ⋯ menu.
const miaMenu = p.locator('button.more-btn[aria-label$="Mia"]');
await miaMenu.scrollIntoViewIfNeeded(); await wait(600);   // scrolling closes an open menu
await miaMenu.click(); await wait(500);
await p.locator('.menu button').nth(3).click(); await wait(600);
await shot('move-to-group');
await p.locator('#modal-body button', { hasText: GROUPS.family }).first().click(); await wait(5500);   // the toast goes, the status line settles
await p.evaluate(() => { document.getElementById('scroller').scrollTop = 0; }); await wait(500);
await shot('people-overview');
await p.setViewportSize({ width: 1280, height: 860 });
await p.click('#all'); await wait(800);
// A photo with both of them, with the info panel.
await p.locator('.cell').nth(5).click(); await wait(1000);
await p.keyboard.press('i'); await wait(1500);
await shot('viewer-people');
await p.keyboard.press('Escape'); await wait(300);

// Select three photos, tag them.
await p.click('#select'); await wait(300);
for (const i of [3, 4, 5]) await p.locator('.cell').nth(i).click();
await wait(300);
await shot('select-photos');
await p.click('#sel-tag'); await wait(400);
await p.fill('#modal-body input', TAG); await wait(300);
await shot('add-tag');
await p.keyboard.press('Enter'); await wait(800);
await p.click('#sel-done').catch(() => {}); await wait(300);

// Move dialog.
await p.keyboard.press('Escape'); await wait(300);
await p.click('#select'); await p.locator('.cell').nth(0).click(); await p.locator('.cell').nth(1).click(); await wait(300);
await p.click('#sel-move'); await wait(400);
await p.fill('#modal-body input[type=text], #modal-body input:not([type])', HOLIDAY); await wait(300);
await shot('move-dialog');
await p.keyboard.press('Escape'); await wait(300);
await p.click('#sel-done').catch(() => {}); await wait(300);

// Search: a person, then a tag: two chips.
await p.fill('#search', 'Mia'); await wait(900);
await shot('search-suggestions');
await p.keyboard.press('ArrowDown'); await p.keyboard.press('Enter'); await wait(1200);
await p.fill('#search', TAG); await wait(900);
await p.keyboard.press('ArrowDown'); await p.keyboard.press('Enter'); await wait(1200);
await p.keyboard.press('Escape'); await p.locator('#title').click(); await wait(800);
await shot('multitag-search');
await p.click('#all'); await wait(500);

// Trash: trash a photo, show the page.
await p.click('#all'); await wait(600);
await p.click('#select'); await p.locator('.cell').nth(0).click(); await wait(200);
await p.click('#sel-trash'); await wait(500);
await p.locator('#modal-actions button').last().click(); await wait(800);
await p.click('#nav-trash'); await wait(1500);
await wait(3500); await shot('trash');

await p.click('#nav-settings'); await wait(1000);
await shot('settings');

// iPad layout (touch).
const pad = await b.newContext({ viewport: { width: 834, height: 1112 }, deviceScaleFactor: 2, hasTouch: true, isMobile: true });
await pad.addInitScript((l) => { try { localStorage.setItem('shoebox.lang', l); } catch (e) {} }, lang);
const q = await pad.newPage();
await q.goto(url); await q.waitForSelector('.cell'); await q.waitForTimeout(1500);
await q.screenshot({ path: `${out}/ipad.png` });
await b.close();
