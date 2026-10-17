// Screenshots for the guide. usage: node screenshots.mjs <app|launcher|drives|across|suggestions|text> <lang> <outdir> <url> <library-path>
// Needs Playwright with a Chromium (CHROME=path) and a freshly generated and
// recognised sample library served by `shoebox serve` (see screenshots.sh).
import { createRequire } from 'node:module';
const { chromium } = createRequire(import.meta.url)('playwright'); // honours NODE_PATH
const [mode, lang, out, url, libPath] = process.argv.slice(2);
const b = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {});
const ctx = await b.newContext({ viewport: { width: 1280, height: 860 }, deviceScaleFactor: 2 });
await ctx.addInitScript((l) => { try { localStorage.setItem('shoebox.lang', l); localStorage.setItem('shoebox.addons.open', '1'); } catch (e) {} }, lang);
const p = await ctx.newPage();
const de = lang === 'de';
// Not 'Favorites': that word is the heart feature now.
const TAG = 'Highlights';
const HOLIDAY = de ? '2025-08 Urlaub' : '2025-08 Holiday';
const t0 = Date.now();
const shot = async (name, opts = {}) => { await p.screenshot({ path: `${out}/${name}.png`, ...opts }); console.log(`  ${lang} ${name} ${((Date.now() - t0) / 1000).toFixed(1)}s`); };
const wait = (ms = 600) => p.waitForTimeout(ms);
// Until a toast has gone (it shows for 4 s) and the pictures on the page are loaded.
const toastGone = () => p.waitForSelector('#toast', { state: 'hidden', timeout: 10000 });
const imagesLoaded = () => p.waitForFunction(() => [...document.images].every((i) => i.complete), null, { timeout: 30000 });
// A button by its label (core/i18n), not by its place: cards differ in which buttons they have.
const { readFileSync, writeFileSync } = await import('node:fs');
const SELECT = JSON.parse(readFileSync(new URL(`../../core/i18n/${lang}.json`, import.meta.url), 'utf8'))['facecheck.select'];

const GROUPS = de ? { family: 'Familie', friends: 'Freunde', colleagues: 'Kollegen', pets: 'Haustiere' } : { family: 'Family', friends: 'Friends', colleagues: 'Colleagues', pets: 'Pets' };
const nameEveryone = () => p.evaluate(async (g) => {
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

// Only the suggestion lists (tag field, name field): a fresh recognised library, no
// other steps. usage: node screenshots.mjs suggestions <lang> <outdir> <url> <library-path>
if (mode === 'suggestions') {
  await p.goto(url); await p.waitForSelector('.cell'); await wait(1500);
  await nameEveryone();

  // Tags are seeded oldest first with pauses: the first two are the most used,
  // the rest the most recently used.
const SEED = de
  ? [['Garten', 6], ['Strand', 4], ['See', 1], ['Picknick', 1], ['Markt', 1], ['Sonnenuntergang', 1], ['Radtour', 1]]
  : [['Garden', 6], ['Beach', 4], ['Lake', 1], ['Picnic', 1], ['Market', 1], ['Sunset', 1], ['Cycling', 1]];
for (const [name, n] of SEED) {
  await p.evaluate(async ([name, n]) => {
    const t = await api(LIBAPI + '/timeline');
    await post(LIBAPI + '/tags/add', { ids: t.ids.slice(0, n), name });
  }, [name, n]);
  await wait(1100);
}
await p.goto(url); await p.waitForSelector('.cell'); await wait(1500);
await p.click('#select'); await p.locator('.cell').nth(0).click(); await wait(300);
await p.click('#sel-tag'); await wait(900);
await shot('tag-suggestions');
await p.keyboard.press('Escape'); await wait(300);   // the first closes the list
await p.keyboard.press('Escape'); await wait(300);   // the second the dialog
await p.click('#sel-done').catch(() => {}); await wait(300);
// The name field of the bar under the single faces (the strangers left unnamed): who was
// named last, who has the most photos.
await p.click('#nav-people'); await wait(1500);
await p.locator('#page .link').first().click(); await wait(3000);
await p.locator('.sface-grid .cface').first().click(); await wait(500);
await p.locator('.singles-bar .pfield input').click(); await wait(800);
await shot('name-suggestions');
  // The bar must stay on one row on a normal screen, also with the longest button (a pet card's
  // "it's a dog, not a cat").
  const rows = await p.evaluate(() => {
    const kids = [...document.querySelector('.singles-bar').children];
    kids.forEach((k) => { k.hidden = false; });
    kids[5].textContent = 'Es ist ein Hund, keine Katze';
    const mid = kids.map((k) => { const r = k.getBoundingClientRect(); return r.top + r.height / 2; });
    return Math.max(...mid) - Math.min(...mid);
  });
  if (rows > 12) throw new Error(`the bar under the single faces wraps (${rows}px between its items)`);
  await b.close(); process.exit(0);
}

if (mode === 'drives') {
  // An original and its backup: Manage drives, the filter on the backup, the
  // photos that are different on the backup, and the badge in the info panel.
  await p.goto(url); await p.waitForSelector('.cell'); await wait(1000);
  const ids = await p.evaluate(async () => {
    const libs = await api('/api/libraries');
    const orig = libs.find((l) => l.name === 'Photos'), bak = libs.find((l) => l.name === 'Photos Backup');
    await post('/api/all/role', { library: bak.id, role: 'backup', of: orig.id });
    await post('/api/all/role', { library: orig.id, role: 'separate' });
    localStorage.setItem('shoebox-library', orig.id);
    return [orig.id, bak.id];
  });
  const drivesPage = async () => { await p.goto(url + '#view=drives'); await p.reload(); await p.waitForSelector('.drive-card'); await wait(1500); };
  await p.setViewportSize({ width: 1280, height: 780 });
  await drivesPage();
  // One list open, to show its files and the folder icon.
  await p.locator('.drive-card:not(.bk) .lists summary').first().click(); await wait(400);
  await shot('drives-overview');
  // "Show photos" on the backup's "only on the backup": that drive's timeline with the chip.
  await p.locator('.drive-card.bk .lists summary .go').nth(1).click(); await p.waitForSelector('.cell'); await wait(1800);
  await p.setViewportSize({ width: 1280, height: 700 });
  await shot('drives-only-backup');
  // The info panel of one of them: the badge next to the name.
  await p.locator('.cell').first().click(); await wait(1000);
  if (!(await p.locator('#lb-panel:visible').count())) await p.keyboard.press('i');
  await wait(1000);
  const box = await p.locator('#lb-panel').boundingBox();
  await shot('drives-badge', { clip: { x: box.x, y: 0, width: box.width, height: 480 } });
  await p.keyboard.press('Escape'); await wait(300);
  // "Show photos" on "different on the backup": both versions, side by side.
  await p.setViewportSize({ width: 1280, height: 780 });
  await drivesPage();
  await p.locator('.drive-card.bk .lists summary .go').first().click(); await p.waitForSelector('.cell'); await wait(1800);
  await p.setViewportSize({ width: 1280, height: 560 });
  await shot('drives-different');
  await b.close(); process.exit(0);
}

if (mode === 'across') {
  // Two drives of their own that share photos, in All drives: the Duplicates page, tab
  // Across drives, with the Ignore folders field: its suggestions, then one folder ignored.
  const HIKE = de ? '2024-09 Wanderung' : '2024-09 Hike';
  await p.goto(url); await p.waitForSelector('.cell'); await wait(1000);
  await p.evaluate(async () => {
    const libs = await api('/api/libraries');
    for (const l of libs) await post('/api/all/role', { library: l.id, role: 'separate' });
    localStorage.setItem('shoebox-library', 'all');
    localStorage.removeItem('shoebox-dup-ignore');
  });
  await p.setViewportSize({ width: 1280, height: 780 });
  await p.goto(url + '#view=duplicates'); await p.reload();
  await p.waitForSelector('.group.cross'); await imagesLoaded(); await wait(800);
  const field = p.locator('.chips-text');
  await field.click(); await p.waitForSelector('.chips-row'); await wait(500);
  await shot('across-suggest');
  // The first row is the hike folder (on both drives); pick it with the keys.
  await field.press('ArrowDown'); await field.press('Enter');
  await p.waitForFunction((n) => document.querySelectorAll('.chips-field .chip').length === 1 && document.querySelector('.chips-field .chip').textContent.startsWith(n), HIKE);
  await p.waitForFunction(() => document.querySelectorAll('.group.cross').length === 6); await imagesLoaded();
  await field.press('Escape'); await wait(600);
  await shot('across-ignore');
  await b.close(); process.exit(0);
}

if (mode === 'launcher') {
  await p.setViewportSize({ width: 760, height: 1100 });
  await p.goto(url); await wait(1200);
  await p.fill('#root', libPath); await p.keyboard.press('Enter'); await wait(600);
  // Show typical drive names instead of whatever this machine has.
  await p.evaluate(() => { var c = document.querySelectorAll('#drive-chips > *'); ['Photos', 'Backup'].forEach((n, i) => { if (c[i]) c[i].textContent = n; }); for (var i = 2; i < c.length; i++) c[i].remove(); });
  await p.click('button[data-kind=scan]');
  await p.waitForSelector('#summary:not(:empty)', { timeout: 60000 }); await wait(1500);
  // Show shoebox on the drive (the folder it was copied to) instead of this machine's path.
  await p.evaluate((lib) => { var h = document.getElementById('addons-hint'); h.textContent = h.textContent.replace(/\/\S*recognizer/, lib + '/shoebox-macos/program/recognizer'); }, libPath);
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

// Text search (phase 9): the sample library has a folder of pictures with words
// in them (SHOEBOX_SAMPLE_TEXT=1) that the mock recognizer "reads".
if (mode === 'text') {
  const word = de ? 'rechnung' : 'invoice';
  await p.goto(url); await p.waitForSelector('.cell'); await wait(1500);
  // The suggestions: the words in photos are a group of their own.
  await p.setViewportSize({ width: 1280, height: 560 });
  await p.fill('#search', word.slice(0, 4)); await wait(1200);
  await shot('text-search-dropdown');
  await p.fill('#search', ''); await p.keyboard.press('Escape'); await p.evaluate(() => document.activeElement && document.activeElement.blur());
  // text: as a chip, the photos that have the word, with the matching line.
  await p.goto(url + '#text=' + word); await p.waitForSelector('.cell .snip'); await wait(1500);
  await shot('text-search-results');
  // The viewer: the scanner button outlines the lines and opens the list.
  await p.locator('.cell').first().click(); await wait(1200);
  await p.setViewportSize({ width: 1280, height: 860 });
  // Opened from a text search, the viewer shows the outlines and the list by itself.
  await p.waitForSelector('#lb-text.on'); await wait(1500);
  if (await p.locator('#lb-panel').isHidden()) { await p.keyboard.press('i'); await wait(1200); }
  await shot('viewer-text');
  await p.keyboard.press('Escape'); await wait(300);
  // Settings: forgetting what was found (Data cleanup).
  await p.goto(url + '#view=settings'); await p.waitForSelector('.settings-section'); await wait(1500);
  await p.setViewportSize({ width: 1280, height: 1400 }); await wait(500);
  await p.locator('.settings-section', { hasText: de ? 'Daten aufräumen' : 'Data cleanup' }).screenshot({ path: `${out}/settings-forget.png` });
  await b.close(); process.exit(0);
}

await p.goto(url); await p.waitForSelector('.cell'); await wait(1000);
// Moving to the trash is off until it is allowed in the settings; the guide shows it on.
await p.evaluate(() => post(LIBAPI + '/allow-trash', { allow: true }));
await p.goto(url); await p.waitForSelector('.cell'); await wait(2500);
await shot('ui-overview');
// Where the controls of "The screen at a glance" are, in the order of its
// legend (one numbered dot each): check.py checks that the dots sit on them.
const GLANCE = [['Folders menu', '#menu'], ['Search box', '#search'], ['Favorites', '#fav-btn'], ['Type', '#types-btn'],
  ['Year', '#years'], ['Select', '#select'], ['Quick links', '.sidebar-head'], ['Folders', '#tree'],
  ['Timeline', '#sizer'], ['Language', '#lang'], ['Status', '#status']];
const boxes = await p.evaluate((sels) => sels.map((sel) => {
  const e = document.querySelector(sel), r = e && e.getBoundingClientRect();
  return r && r.width && r.height ? { x: r.x, y: r.y, w: r.width, h: Math.min(r.height, innerHeight - r.y) } : null;
}), GLANCE.map((g) => g[1]));
writeFileSync(`${out}/ui-overview.json`, JSON.stringify({ width: 1280, height: 860, names: GLANCE.map((g) => g[0]), boxes }, null, 1));

// Finding your way: on another page the top bar names the page and the sidebar marks it.
await p.setViewportSize({ width: 1280, height: 520 });
await p.click('#nav-dups'); await wait(3000);
await shot('navigation');
await p.setViewportSize({ width: 1280, height: 860 });
await p.goto(url); await p.waitForSelector('.cell'); await wait(1500);

// Viewer with the info panel (a landscape; newest first: 3 July, 3 May, then March).
await p.locator('.cell').nth(6).click(); await wait(800);
await p.keyboard.press('i'); await wait(800);
await shot('viewer-info');
// The panel alone, close up.
await p.locator('.lb-panel').screenshot({ path: `${out}/viewer-info-panel.png` });
// The pencil next to the date opens the dialog for a date of your own.
await p.locator('.lb-panel .pen').click(); await wait(400);
await p.fill('.dates-dialog input', '1987'); await p.selectOption('.dates-dialog select >> nth=0', '6'); await wait(300);
await shot('dates-dialog');
await p.keyboard.press('Escape'); await wait(300);
await p.keyboard.press('Escape'); await wait(300);

// Duplicates, from a clean state; tall so the whole page is in the shot.
await p.click('#nav-dups'); await p.waitForSelector('.dup-pre summary', { timeout: 60000 }); await imagesLoaded(); await wait(800);
await p.setViewportSize({ width: 1280, height: 1500 }); await wait(800);
// The Preselect menu open, then its "Smaller versions": that group comes to the
// top with the messenger copy ticked.
await p.click('.dup-pre summary'); await wait(400);
await shot('duplicates-preselect', { clip: { x: 0, y: 0, width: 1280, height: 760 } });
await p.locator('.dup-pre button', { hasText: de ? 'Kleinere Versionen' : 'Smaller versions' }).first().click(); await wait(1200);
await shot('duplicates');
await p.setViewportSize({ width: 1280, height: 860 });
await p.click('#all'); await wait(800);

// People and pets: the cards waiting for a name.
await p.click('#nav-people'); await wait(1200);
await p.locator('#page .link').first().click(); await wait(2500);
await p.waitForSelector('#un-box .cluster button.btn.quiet', { timeout: 120000 }); await wait(1500);   // the cards come when the clusters are computed
await shot('people-unnamed');

// "Select" on a card: tick some faces, then name, ignore or mark only those.
const card = p.locator('#un-box .cluster').first();
await card.locator('button.btn.quiet', { hasText: SELECT }).first().click(); await wait(500);
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
await nameEveryone();
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
await p.locator('#modal-body button', { hasText: GROUPS.family }).first().click(); await wait(500);
await toastGone(); await wait(800);   // the status line settles
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
// Export: the same dialog from the tag chip (the folder name is the tag's name).
await p.locator('.export-chip').click(); await wait(1200);
await shot('export-dialog');
await p.keyboard.press('Escape'); await wait(400);
await p.click('#all'); await wait(500);
// Search by date: `date:` and a month lists it in every year.
await p.fill('#search', de ? 'datum:jul' : 'date:jul'); await wait(900);
await shot('dates-search');
await p.fill('#search', ''); await p.keyboard.press('Escape'); await wait(300);

// Trash: trash a photo, show the page.
await p.click('#all'); await wait(600);
await p.click('#select'); await p.locator('.cell').nth(0).click(); await wait(200);
await p.click('#sel-trash'); await wait(500);
await p.locator('#modal-actions button').last().click(); await wait(800);
await p.click('#nav-trash'); await toastGone(); await imagesLoaded(); await wait(800); await shot('trash');

await p.click('#nav-settings'); await wait(1000);
await shot('settings');

// iPad layout (touch).
const pad = await b.newContext({ viewport: { width: 834, height: 1112 }, deviceScaleFactor: 2, hasTouch: true, isMobile: true });
await pad.addInitScript((l) => { try { localStorage.setItem('shoebox.lang', l); } catch (e) {} }, lang);
const q = await pad.newPage();
await q.goto(url); await q.waitForSelector('.cell'); await q.waitForTimeout(1500);
await q.screenshot({ path: `${out}/ipad.png` });

await b.close();
