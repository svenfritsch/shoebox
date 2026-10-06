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
const TAG = de ? 'Favoriten' : 'Favorites';
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
  await shot('12-launcher', { fullPage: true });
  await b.close(); process.exit(0);
}

await p.goto(url); await p.waitForSelector('.cell'); await wait(2500);
await shot('01-grid');

// Viewer with the info panel (a landscape; newest first: 3 July, 3 May, then March).
await p.locator('.cell').nth(6).click(); await wait(800);
await p.keyboard.press('i'); await wait(800);
await shot('02-viewer');
await p.keyboard.press('Escape'); await wait(300);

// Duplicates, from a clean state; tall so the whole page is in the shot.
await p.click('#nav-dups'); await wait(7000);
await p.setViewportSize({ width: 1280, height: 1500 }); await wait(800);
await shot('09-duplicates');
await p.setViewportSize({ width: 1280, height: 860 });
await p.click('#all'); await wait(800);

// People and pets: the cards waiting for a name, then name them.
await p.click('#nav-people'); await wait(1200);
await p.locator('#page .link').first().click(); await wait(2500);
await shot('14-unnamed');
await p.evaluate(async (names) => {
  for (const kind of ['faces', 'pets']) {
    const res = await api(LIBAPI + '/clusters?kind=' + kind);
    for (const c of res.clusters) {
      await post(LIBAPI + '/clusters/' + c.id + '/name', { generation: c.generation, name: kind === 'pets' ? names.pet : names.person });
    }
  }
}, { person: 'Mia', pet: 'Whiskers' });
await p.goto(url); await p.waitForSelector('.cell'); await wait(2000);
await p.click('#nav-people'); await wait(2000);
await shot('15-people');
await p.click('#all'); await wait(800);
// A photo with both of them, with the info panel.
await p.locator('.cell').nth(5).click(); await wait(1000);
await p.keyboard.press('i'); await wait(1500);
await shot('16-info-people');
await p.keyboard.press('Escape'); await wait(300);

// Select three photos, tag them.
await p.click('#select'); await wait(300);
for (const i of [3, 4, 5]) await p.locator('.cell').nth(i).click();
await wait(300);
await shot('03-select');
await p.click('#sel-tag'); await wait(400);
await p.fill('#modal-body input', TAG); await wait(300);
await shot('04-tag');
await p.keyboard.press('Enter'); await wait(800);
await p.click('#sel-done').catch(() => {}); await wait(300);

// Move dialog.
await p.keyboard.press('Escape'); await wait(300);
await p.click('#select'); await p.locator('.cell').nth(0).click(); await p.locator('.cell').nth(1).click(); await wait(300);
await p.click('#sel-move'); await wait(400);
await p.fill('#modal-body input[type=text], #modal-body input:not([type])', HOLIDAY); await wait(300);
await shot('05-move');
await p.keyboard.press('Escape'); await wait(300);
await p.click('#sel-done').catch(() => {}); await wait(300);

// Search: a person, then a tag: two chips.
await p.fill('#search', 'Mia'); await wait(900);
await shot('06-search-suggest');
await p.keyboard.press('ArrowDown'); await p.keyboard.press('Enter'); await wait(1200);
await p.fill('#search', TAG); await wait(900);
await p.keyboard.press('ArrowDown'); await p.keyboard.press('Enter'); await wait(1200);
await p.keyboard.press('Escape'); await p.locator('#title').click(); await wait(800);
await shot('07-search-chips');
await p.click('#all'); await wait(500);

// Import dialog.
await p.click('#import'); await wait(600);
await shot('08-import');
await p.keyboard.press('Escape'); await wait(300);
await p.click('#modal-actions button >> nth=0').catch(() => {}); await wait(300);

// Trash: trash a photo, show the page.
await p.click('#all'); await wait(600);
await p.click('#select'); await p.locator('.cell').nth(0).click(); await wait(200);
await p.click('#sel-trash'); await wait(500);
await p.locator('#modal-actions button').last().click(); await wait(800);
await p.click('#nav-trash'); await wait(1500);
await wait(3500); await shot('10-trash');

await p.click('#nav-settings'); await wait(1000);
await shot('11-settings');

// iPad layout (touch).
const pad = await b.newContext({ viewport: { width: 834, height: 1112 }, deviceScaleFactor: 2, hasTouch: true, isMobile: true });
await pad.addInitScript((l) => { try { localStorage.setItem('shoebox.lang', l); } catch (e) {} }, lang);
const q = await pad.newPage();
await q.goto(url); await q.waitForSelector('.cell'); await q.waitForTimeout(1500);
await q.screenshot({ path: `${out}/13-ipad.png` });
await b.close();
