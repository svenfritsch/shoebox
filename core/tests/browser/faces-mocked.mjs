// The Unnamed page, groups, the people list, a person's faces page (and the
// viewer opened from it), the search box and the trash page against a fake API
// (fake-api.mjs) on a fake clock: which requests the page makes after each
// action and when, and what the sidebar shows from the replies (PRs #142 and
// #143); the search box's requests while someone types. No shoebox is needed:
// the page is core/web as it is on disk.
//
//   node core/tests/browser/faces-mocked.mjs     (ONLY=text: only those tests)
//
// Time only moves when a test moves it (`settle`), so "nothing is asked for
// until the grouping is done" is checked over minutes of the page's time in a
// few milliseconds, and the steps of a grouping come one at a time. The real
// server tells the page of each step by its event stream; here the test does,
// by calling the page's own `loadInfo` (`tell`), as the stream's event does.
import { browser, check, until, tr, test, finish, requests, noListRequests, clusterRounds, sidebarIs, groupNames, menu, typeName, face, card, single, nameCard } from './faces-common.mjs';
import { fakeLibrary, ORIGIN, LIB } from './fake-api.mjs';
import { inflateSync } from 'node:zlib';

const ctx = await browser.newContext({ viewport: { width: 1280, height: 1400 } });
await ctx.addInitScript(() => { try { localStorage.setItem('shoebox.lang', 'en'); } catch (e) {} });
ctx.setDefaultTimeout(20000);

// The page's time runs on: its timers fire, the fake answers, until three
// rounds of a second bring no request (but polls of /info, which come every
// 1.5 s while a grouping runs). Each round waits until the page has every reply
// (`lib.waiting`), not only until the fake has sent it: on a slow machine the
// page reads a naming's reply later, and the only request that follows is /info.
async function settle(p, lib) {
  for (let round = 0, calm = 0; calm < 3; round++) {
    check(round < 300, 'the page never stops asking the server');
    const before = lib.log.filter((e) => e.path !== '/info').length;
    await p.clock.runFor(1000);
    await until('the fake to answer', () => lib.open === 0 && lib.waiting() === 0);
    await p.evaluate(() => 0);
    await p.evaluate(() => 0);
    calm = lib.open === 0 && lib.log.filter((e) => e.path !== '/info').length === before ? calm + 1 : 0;
  }
}

// What the event stream does when the grouping moves on: the page asks for the status.
async function tell(p, lib) {
  await p.evaluate(() => loadInfo());
  await settle(p, lib);
}

// A page at `hash` on a fake library that `setup(lib)` filled; `run(p, lib, s)`
// with `s` what setup returned. The page must ask for nothing the fake does not
// know and throw no error.
async function on(hash, setup, run) {
  return async (keep) => {
    const lib = fakeLibrary();
    const s = setup(lib);
    const p = keep(await ctx.newPage());
    // Requests the page sent and has not had the whole reply of yet.
    let waiting = 0;
    p.on('request', () => waiting++);
    p.on('requestfinished', () => waiting--);
    p.on('requestfailed', () => waiting--);
    lib.waiting = () => waiting;
    const errors = [];
    p.on('pageerror', (e) => errors.push(e.message));
    await p.route(ORIGIN + '/**', lib.route);
    // The map's tiles: none (the page shows its "offline" label, as without internet).
    await p.route('https://tile.openstreetmap.org/**', (r) => r.abort());
    if (lib.maps) await p.addInitScript(() => { try { localStorage.setItem('shoebox-maps', '1'); } catch (e) {} });
    await p.clock.install({ time: new Date('2026-06-01T10:00:00') });
    await p.goto(`${ORIGIN}/#${hash}`);
    // Started: the status and the people list are in (`settle` alone could
    // find the page calm while its scripts are still loading).
    await until('the page to start', async () => {
      await p.clock.runFor(100);
      return p.evaluate(() => !!(window.people && people.info && people.list && !peopleFlight));
    });
    await settle(p, lib);
    await run(p, lib, s);
    check(!lib.unknown.length, `the page asked for what the fake does not answer (add it to fake-api.mjs): ${lib.unknown.join(', ')}`);
    check(!errors.length, 'error in the page: ' + errors.join('; '));
  };
}

// The POST whose path matches has arrived.
const posted = (lib, re) => until(`a POST to ${re}`, () => lib.log.some((e) => e.method === 'POST' && re.test(e.path)));

// A name given on the Unnamed page by `act`: from the reply alone the person is
// in the sidebar; no clusters are asked for while the name waits for the
// grouping, nor while it starts or runs; once it is done, one refresh (large
// and single faces together); never the people list or the groups.
async function namedThenGrouped(p, lib, act) {
  const from = lib.log.length;
  await act();
  await settle(p, lib);
  noListRequests(lib.log, from, 'a name on the Unnamed page');
  await sidebarIs(p, lib.lists(), 'from the reply');
  check(lib.pending, 'a name waits for the next grouping');
  for (const step of ['stale', 'running']) {
    lib.start(step);
    await tell(p, lib);
    check(!requests(lib.log, from, '/clusters').length, `the clusters were asked for while the grouping was ${step}`);
  }
  lib.grouped();
  await tell(p, lib);
  const rounds = clusterRounds(lib.log, from);
  check(rounds === 'large, small', `after the grouping the clusters were asked for as [${rounds}], expected one refresh (large, small)`);
  noListRequests(lib.log, from, 'a name on the Unnamed page, after the grouping');
  await sidebarIs(p, lib.lists(), 'after the grouping');
}

// A change to people or groups by `act`: the page asks for neither the people
// list nor the groups and its sidebar is right from the reply. Names and
// groups change no suggestion, so no grouping follows. After the grouping that
// follows a merge or a deletion, the People page asks for the list once, on
// purpose (the suggestions changed); no other page does.
async function changedThenGrouped(p, lib, why, act) {
  const from = lib.log.length;
  await act();
  await settle(p, lib);
  await tell(p, lib);
  noListRequests(lib.log, from, why);
  await sidebarIs(p, lib.lists(), why);
  if (!lib.pending) return;
  const after = lib.log.length;
  lib.start();
  await tell(p, lib);
  lib.grouped();
  await tell(p, lib);
  const n = requests(lib.log, after, '/people').length, g = requests(lib.log, after, '/groups').length;
  if (await p.evaluate(() => state.filter.view === 'people')) check(n === 1 && g === 1, `${why}: after the grouping the People page asked for the list ${n} times, the groups ${g} times, expected once`);
  else noListRequests(lib.log, after, `${why}, after the grouping`);
  await sidebarIs(p, lib.lists(), `${why}, after the grouping`);
}

// Two people, three cards, one card that looks like Pia, four single faces.
function faces(lib) {
  const olga = lib.person('Olga Listed', { faces: 4 });
  const pia = lib.person('Pia Look', { faces: 3 });
  return { olga, pia, a: lib.cluster(3), b: lib.cluster(4), c: lib.cluster(5), look: lib.cluster(3, pia), singles: [1, 2, 3, 4].map(() => lib.cluster(1)) };
}

const first = (c) => c.faces[0].id;

// ---- the Unnamed page

await test('a card named with a new name: the person from the reply, the clusters once the grouping is done', await on('view=unnamed', faces, async (p, lib, s) => {
  const id = first(s.a);
  await namedThenGrouped(p, lib, async () => {
    await nameCard(p, id, 'Nora Card');
    await posted(lib, /^\/clusters\/\d+\/name$/);
  });
  check(!(await card(p, id).count()), 'the card is gone');
  check(lib.people.some((x) => x.name === 'Nora Card' && x.photos === 3), 'Nora is on the server with the card\'s three faces');
}));

await test('a card named for someone the list has (typed)', await on('view=unnamed', faces, async (p, lib, s) => {
  await namedThenGrouped(p, lib, async () => {
    await nameCard(p, first(s.b), 'Olga Listed');
    await posted(lib, /^\/clusters\/\d+\/name$/);
  });
  check(s.olga.faces === 8, `Olga has ${s.olga.faces} faces, expected 4 + 4`);
}));

await test('a card named from its suggestion (✓)', await on('view=unnamed', faces, async (p, lib, s) => {
  const sugg = card(p, first(s.look)).locator('.csugg');
  check((await sugg.locator('b').textContent()) === 'Pia Look', 'the card says who they look like');
  await namedThenGrouped(p, lib, async () => {
    await sugg.locator('button').first().click();
    await posted(lib, /^\/clusters\/\d+\/name$/);
  });
  check(s.pia.faces === 6, `Pia has ${s.pia.faces} faces, expected 3 + 3`);
}));

await test('single faces named with a new name from the bar (Enter)', await on('view=unnamed', faces, async (p, lib, s) => {
  const [a, b] = s.singles.map(first);
  await single(p, a).click();
  await single(p, b).click();
  const bar = p.locator('.singles-bar');
  check(await bar.isVisible(), 'picking faces shows the bar');
  await namedThenGrouped(p, lib, async () => {
    await bar.locator('.pfield input').fill('Sam Single');
    await bar.locator('.pfield input').press('Enter');
    await posted(lib, /^\/faces\/assign$/);
  });
  check(!(await single(p, a).count()) && !(await single(p, b).count()), 'the named faces are gone');
  check(await bar.isHidden(), 'the bar goes with the picked faces');
}));

await test('a single face named for someone the list has (Name button)', await on('view=unnamed', faces, async (p, lib, s) => {
  const a = first(s.singles[2]);
  await single(p, a).click();
  const bar = p.locator('.singles-bar');
  await namedThenGrouped(p, lib, async () => {
    await bar.locator('.pfield input').fill('Olga Listed');
    await bar.locator('.pfield input').press('Escape'); // the list folds away
    await bar.locator('[data-act="name"]').click();
    await posted(lib, /^\/faces\/assign$/);
  });
  check(s.olga.faces === 5, `Olga has ${s.olga.faces} faces, expected 4 + 1`);
}));

// A card holds at most `clusters::MAX_CLUSTER` (100) faces, fewer than
// GROW_STEP (120): with the page's own step all of them come at once. The
// parts are tried with a smaller step (set in the page) and faces as wide as
// the screen, so the end of a part is far below it.
await test('a long card adds its faces as they are scrolled to, its tiles stay across Select and a refresh (growGrid)', await on('view=unnamed', (lib) => ({ big: lib.cluster(100), other: lib.cluster(3) }), async (p, lib, s) => {
  const k = card(p, first(s.big));
  const tiles = () => k.locator('.cfaces .cface').count();
  const shows = async (n, what) => { await settle(p, lib); await until(what, async () => (await tiles()) === n); };
  await k.locator('.cmore').click();
  await shows(100, 'all faces of the card with the page\'s own step');
  check(!(await k.locator('.grow-end').count()), 'no end marker once all are in');
  await k.locator('.cclose').click();
  await shows(8, 'the card closed again');

  await p.evaluate(() => { GROW_STEP = 40; document.getElementById('page').style.setProperty('--face-size', '600px'); });
  await k.locator('.cmore').click();
  await shows(40, 'the first part');
  check((await k.locator('.grow-end').count()) === 1, 'an end marker after the first part');
  await k.evaluate((c) => c.querySelectorAll('.cfaces .cface').forEach((f, i) => { f.dataset.testMark = i; }));
  await k.locator('.grow-end').scrollIntoViewIfNeeded();
  await shows(80, 'the second part');
  await k.locator('.grow-end').scrollIntoViewIfNeeded();
  await shows(100, 'the rest');
  check(!(await k.locator('.grow-end').count()), 'no end marker once all are in');

  const marked = () => k.evaluate((c) => [...c.querySelectorAll('.cfaces .cface')].filter((f) => f.dataset.testMark != null).length);
  await k.locator('.cbar [data-act="pick"]').click();
  await settle(p, lib);
  check((await k.getAttribute('class')).includes('picking'), 'Select is on');
  check((await marked()) === 40, `after Select ${await marked()} of the 40 marked tiles are left`);
  check((await tiles()) === 100, `after Select the card shows ${await tiles()} faces`);

  await namedThenGrouped(p, lib, async () => {
    await nameCard(p, first(s.other), 'Uwe Elsewhere');
    await posted(lib, /^\/clusters\/\d+\/name$/);
  });
  check(requests(lib.log, 0, `/clusters/${s.big.id}/faces`).length >= 2, 'the open card took its faces again after the refresh');
  check((await k.getAttribute('class')).includes('picking'), 'the big card is still selecting');
  check((await marked()) === 40, `after a name elsewhere ${await marked()} of the 40 marked tiles are left`);
  check((await tiles()) === 100, `after a name elsewhere the card shows ${await tiles()} faces`);
}));

// ---- when the next grouping comes (the line at the top right)

// The line and its circle as the page shows them.
const line = (p) => p.evaluate(() => {
  const l = document.getElementById('un-next'), tick = l && l.querySelector('.tick');
  return l && {
    shown: !l.hidden, text: l.querySelector('.text').textContent, buttons: l.querySelectorAll('button').length,
    deg: tick ? parseFloat(tick.style.getPropertyValue('--regroup-deg')) : null, ms: tick ? tick.style.transitionDuration : null,
    floating: l.classList.contains('floating'), live: document.getElementById('un-live').textContent,
    status: document.getElementById('status').textContent, title: document.getElementById('current-month').textContent,
  };
});

// Many cards (named one after the other) and single faces.
function manyCards(lib) {
  const olga = lib.person('Olga Listed', { faces: 4 });
  return { olga, cards: Array.from({ length: 12 }, (_, k) => lib.cluster(14 - k)), singles: [1, 2].map(() => lib.cluster(1)) };
}

await test('the next grouping: the line after a name, cards and seconds count down, the circle refills, then finding and gone; no button', await on('view=unnamed', manyCards, async (p, lib, s) => {
  let l = await line(p);
  check(!l.shown, 'no line before a decision');
  check(l.title === await tr(p, 'people.unnamed'), `the top bar says "${l.title}", not the Unnamed page`);
  const status = l.status;
  await namedThenGroupedLine(p, lib, s);
  l = await line(p);
  check(l.status === status, `the sidebar's status line changed while grouping: "${l.status}"`);
}));

async function namedThenGroupedLine(p, lib, s) {
  // The line as "<cards>, <seconds>" (the page's time runs on while it settles).
  const parts = async () => {
    const l = await line(p);
    for (let n = 1; n <= 10; n++) for (let secs = 0; secs <= 30; secs++) {
      if (l.text === await p.evaluate(([n, secs]) => trn('unnamed.next', n, { s: secs }), [n, secs])) return { ...l, cards: n, secs };
    }
    return { ...l, cards: null, secs: null };
  };
  await nameCard(p, first(s.cards[0]), 'Ada First');
  await posted(lib, /^\/clusters\/\d+\/name$/);
  await settle(p, lib);
  let l = await parts();
  check(l.shown && l.cards === 4 && l.secs > 20, `after the first name the line says "${l.text}"`);
  check(l.buttons === 0, 'the line has no button');
  check(Math.abs(l.deg - 12 * (l.secs - 1)) < 13, `the circle is at ${l.deg}° with ${l.secs} s left (12° a second)`);
  check(l.live === await p.evaluate(() => trn('unnamed.next_cards', 4)), `screen readers hear "${l.live}"`);
  const said = l.live, before = l.secs;

  await p.clock.runFor(5000);
  l = await parts();
  check(l.cards === 4 && l.secs === before - 5, `5 s later the line says "${l.text}"`);
  check(Math.abs(l.deg - 12 * (l.secs - 1)) < 13, `5 s later the circle is at ${l.deg}° with ${l.secs} s left`);
  check(l.live === said, 'screen readers are not told every second');

  await nameCard(p, first(s.cards[1]), 'Ben Second');
  await until('the second name', () => lib.log.filter((e) => e.method === 'POST' && /^\/clusters\/\d+\/name$/.test(e.path)).length === 2);
  await settle(p, lib);
  l = await parts();
  check(l.cards === 3 && l.secs > before - 5, `after the second name the line says "${l.text}" (the seconds start again)`);
  check(Math.abs(l.deg - 12 * (l.secs - 1)) < 13, `the circle filled up again: ${l.deg}° with ${l.secs} s left`);
  check(l.live === await p.evaluate(() => trn('unnamed.next_cards', 3)), `screen readers hear the new count: "${l.live}"`);

  await p.evaluate(() => { document.getElementById('scroller').scrollTop = 600; document.getElementById('scroller').dispatchEvent(new Event('scroll')); });
  l = await line(p);
  check(l.floating, 'scrolled, the line is in its box');

  const from = lib.log.length;
  lib.start('running');
  lib.progress = [37, 100];
  await tell(p, lib);
  l = await line(p);
  check(l.shown && l.text === await tr(p, 'unnamed.regrouping'), `while grouping the line says "${l.text}"`);
  check(Math.abs(l.deg - 133.2) < 1, `the circle shows the progress: ${l.deg}°, expected 37% of 360°`);
  check(l.live === await tr(p, 'unnamed.regrouping'), 'screen readers hear that it started');
  check(!requests(lib.log, from, '/clusters').length, 'no clusters asked for while grouping');

  lib.grouped();
  await tell(p, lib);
  l = await line(p);
  check(!l.shown, 'the line is gone when the grouping is done');
  check(l.live === await tr(p, 'unnamed.regrouped_done'), 'screen readers hear that it is done');
  check(clusterRounds(lib.log, from) === 'large, small', 'the cards updated once');
}

await test('the circle steps once a second with reduced motion', await on('view=unnamed', manyCards, async (p, lib, s) => {
  await p.emulateMedia({ reducedMotion: 'reduce' });
  await nameCard(p, first(s.cards[0]), 'Ada First');
  await posted(lib, /^\/clusters\/\d+\/name$/);
  await settle(p, lib);
  await p.clock.runFor(1000);
  const a = await line(p);
  await p.clock.runFor(1000);
  const b = await line(p);
  check(a.ms === '0ms' && b.ms === '0ms', `no smooth movement: ${a.ms}, ${b.ms}`);
  check(Math.abs(a.deg - b.deg - 12) < 1.5, `one step of 12° a second: ${a.deg}° then ${b.deg}°`);
}));

await test('leaving the Unnamed page while names wait asks for the grouping soon; with nothing waiting it asks nothing', await on('view=unnamed', manyCards, async (p, lib, s) => {
  await p.evaluate(() => showView('people'));
  await settle(p, lib);
  check(!requests(lib.log, 0, '/clusters/left', 'POST').length, 'left with nothing waiting: nothing asked');
  await p.evaluate(() => showView('unnamed'));
  await settle(p, lib);
  await nameCard(p, first(s.cards[0]), 'Ada First');
  await posted(lib, /^\/clusters\/\d+\/name$/);
  await settle(p, lib);
  await p.evaluate(() => showView('people'));
  await settle(p, lib);
  check(requests(lib.log, 0, '/clusters/left', 'POST').length === 1, 'leaving with names waiting tells the server once');
}));

// ---- the search box

// A pet's row: picture, species icon and name on one middle line, as are a
// text row's icon and words (not the picture on the text's baseline).
await test('search suggestions: picture, icon and name of a row are centred on one line', await on('view=people', (lib) => {
  lib.person('Spucky', { faces: 5, species: 'cat' });
  lib.person('Spud Person', { faces: 2 });
}, async (p, lib) => {
  await p.locator('#search').fill('spu');
  await settle(p, lib);
  const rows = await p.locator('#suggest .item').evaluateAll((bs) => bs.map((b) => {
    const mid = (e) => { const r = e.getBoundingClientRect(); return r.top + r.height / 2; };
    return { text: b.textContent, parts: [...b.querySelectorAll('.avatar, .ic, .text, .count')].map((e) => [e.className, mid(e)]) };
  }));
  check(rows.length === 3, `expected Spucky, Spud Person and the text row, got ${JSON.stringify(rows.map((r) => r.text))}`);
  check(rows[0].parts.length === 4 && rows[0].parts[0][0].includes('pet-cat'), `the pet's row has picture, icon, name and count: ${JSON.stringify(rows[0])}`);
  for (const r of rows) {
    const mids = r.parts.map((x) => x[1]);
    check(Math.max(...mids) - Math.min(...mids) <= 1, `not on one middle line: ${JSON.stringify(r)}`);
  }
}));

await test('a first click into the search box opens the list', await on('', (lib) => {
  lib.person('Spucky', { faces: 5, species: 'cat' });
}, async (p, lib) => {
  check(await p.evaluate(() => document.activeElement !== $('search')), 'the box starts focused');
  await p.locator('#search').click();
  await settle(p, lib);
  check(await p.locator('#suggest').isVisible(), 'the list is not open after the first click');
  check(await p.locator('#suggest .item').count() > 0, 'the list has no rows after the first click');
}));

await test('the list opens on a first click on any page, not only the timeline', await on('view=people', (lib) => lib.person('Spucky', { faces: 5, species: 'cat' }), async (p, lib) => {
  await p.locator('#search').click();
  await settle(p, lib);
  check(await p.locator('#suggest .item').count() > 0, 'no suggestions after a click in the box on the Faces page');
  await p.locator('#suggest .item', { hasText: 'Spucky' }).click();
  await settle(p, lib);
  await until('the pick on the timeline', () => p.evaluate(() => !state.filter.view && state.filter.people.length === 1));
}));

await test('picking a suggestion closes the list and asks for no suggestions until the box is clicked or typed in', await on('', (lib) => {
  lib.person('Spucky', { faces: 5, species: 'cat' });
}, async (p, lib) => {
  await p.locator('#search').fill('spu');
  await settle(p, lib);
  const from = lib.log.length;
  await p.locator('#suggest .item', { hasText: 'Spucky' }).click();
  check(await p.locator('#suggest').isHidden(), 'the list is open right after the pick');
  await settle(p, lib);
  await until('the pick in the search', () => p.evaluate(() => state.filter.people.length === 1));
  check(await p.locator('#suggest').isHidden(), 'the list opened again for the new search');
  check(!lib.log.slice(from).some((e) => e.path === '/suggest'), 'suggestions were asked for after the pick');
  check(await p.evaluate(() => document.activeElement === $('search')), 'the box lost the focus with the pick');
  await p.locator('#search').click();
  await settle(p, lib);
  check(await p.locator('#suggest').isVisible(), 'a click in the focused box does not open the list again');
}));

// ---- a search term shown in the sidebar

// A sidebar taller than the window: two folded groups, 40 tags, 60 folders
// (Zielordner last, at the bottom).
function longSidebar(lib) {
  const anna = lib.person('Anna');
  lib.group('Familie', anna);
  const zora = lib.person('Zora');
  const zoo = lib.group('Zoo', zora);
  lib.tags = Array.from({ length: 40 }, (_, i) => ({ id: i + 1, name: `Tag ${String(i + 1).padStart(2, '0')}`, count: 1, kind: 'manual' }));
  lib.folders = [{ id: 1, parent: null, name: '', path: '', count: 60 }, ...Array.from({ length: 60 }, (_, i) => {
    const name = i === 59 ? 'Zielordner' : `Ordner ${String(i + 1).padStart(2, '0')}`;
    return { id: i + 2, parent: 1, name, path: name, count: 1 };
  })];
  return { zora, zoo, last: 61 };
}

// Whether the sidebar row of the name button `sel` is in view (above the status line).
const rowInView = (p, sel) => p.evaluate((sel) => {
  const b = document.querySelector(sel);
  if (!b || !b.offsetParent) return false;
  const s = $('side-scroll').getBoundingClientRect(), r = b.parentNode.getBoundingClientRect();
  return r.top >= s.top - 0.5 && r.bottom <= Math.min(s.bottom, $('status').getBoundingClientRect().top) + 0.5;
}, sel);
const sideTop = (p) => p.evaluate(() => $('side-scroll').scrollTop);

// Types `text` and picks its suggestion of `kind`.
async function pickKind(p, lib, text, kind) {
  await p.locator('#search').fill(text);
  await settle(p, lib);
  const i = await p.evaluate((k) => sugg.items.findIndex((it) => it.kind === k), kind);
  check(i >= 0, `no ${kind} offered for ${text}`);
  const before = await p.evaluate(() => location.hash);
  await p.locator(`#suggest .item[data-i="${i}"]`).click();
  // The pick comes in by the hashchange event, which may follow the clicks' timers.
  await until(`the ${kind} in the search`, () => p.evaluate((h) => location.hash !== h && shownHash === location.hash, before));
  await settle(p, lib);
}

await test('a folder, person or tag picked in the search box is scrolled into view in the sidebar, a folded group unfolds; Back does not scroll', await on('', longSidebar, async (p, lib, s) => {
  const folder = `#tree .name[data-id="${s.last}"]`, person = `#people-tree .name[data-person="${s.zora.id}"]`, tag = '#own-tags .name[data-tag="40"]';
  check(await p.evaluate(() => $('side-scroll').scrollHeight > $('side-scroll').clientHeight * 1.5), 'the sidebar is not long enough to scroll');
  check(!(await rowInView(p, folder)) && !(await rowInView(p, person)), 'the folder or the person is in view before the picks');
  await pickKind(p, lib, 'Zielordner', 'folder');
  check(await p.evaluate(() => state.filter.folder) === s.last, 'the folder is not in the search');
  check(await rowInView(p, folder), 'the picked folder is not in view');
  // The person's group is folded and at the top: it unfolds and the sidebar goes up.
  await pickKind(p, lib, 'Zora', 'person');
  check(await rowInView(p, person), 'the picked person is not in view');
  check(await p.evaluate((g) => !!people.open[g], s.zoo.id), 'the person\'s group did not unfold');
  check(await p.evaluate(() => !people.open[people.groups.find((g) => g.name === 'Familie').id]), 'another group unfolded');
  await pickKind(p, lib, 'Tag 40', 'tag');
  check(await rowInView(p, tag), 'the picked tag is not in view');
  check(await p.evaluate(() => document.scrollingElement.scrollTop === 0 && $('sidebar').scrollTop === 0), 'the page or the sidebar\'s frame scrolled, not only its list');
  check(!(await rowInView(p, person)), 'the person is still in view (the sidebar did not move to the tag)');
  // Only a pick: the way back (or a reload) leaves the sidebar where it is.
  await p.evaluate(() => { $('side-scroll').scrollTop = 0; });
  await p.evaluate(() => history.back());
  await until('the search without the tag', () => p.evaluate(() => state.filter.tags.length === 0 && state.filter.people.length === 1));
  await settle(p, lib);
  check(await sideTop(p) === 0, `going back scrolled the sidebar to ${await sideTop(p)}`);
  check(await p.locator(person).count() === 1, 'the person\'s group did not stay open');
}));

await test('a click on the name of a folder or person chip shows it in the sidebar (also with Enter); a reload and the Hidden chip do not', await on('folder=61&person=2&hidden=1', longSidebar, async (p, lib, s) => {
  const folder = `#tree .name[data-id="${s.last}"]`, person = `#people-tree .name[data-person="${s.zora.id}"]`;
  check(await sideTop(p) === 0, `the sidebar was scrolled on load to ${await sideTop(p)}`);
  const go = p.locator('#filters .chip .go');
  check(await go.count() === 2, `expected the folder and the person chip to show in the sidebar, got ${await go.count()}`);
  check((await go.first().getAttribute('data-tip')) === await tr(p, 'chip.show_in_sidebar'), 'tip: ' + await go.first().getAttribute('data-tip'));
  check(await p.locator('#filters .chip.hid .go').count() === 0, 'the Hidden chip offers the sidebar');
  await p.locator('#filters .chip.hid').click({ position: { x: 30, y: 10 } });
  await settle(p, lib);
  check(await sideTop(p) === 0, 'a click on the Hidden chip scrolled the sidebar');
  await go.filter({ hasText: 'Zielordner' }).click();
  check(await rowInView(p, folder), 'the folder is not in view after a click on its chip');
  check(await p.locator('#filters .chip:not(.more):not(.clear)').count() === 3 && await p.evaluate(() => state.filter.folder === 61 && state.filter.hidden), 'the click changed the search');
  // Back up with the keyboard: the person's group unfolds.
  await go.filter({ hasText: 'Zora' }).focus();
  await p.keyboard.press('Enter');
  check(await rowInView(p, person), 'the person is not in view after Enter on their chip');
  check(await p.evaluate((g) => !!people.open[g], s.zoo.id), 'the person\'s group did not unfold');
}));

// ---- the Unnamed page's filter

await test('the Unnamed page keeps its filter (people and pets, faces, pets) over a reload', await on('view=unnamed', (lib) => ({ cards: [lib.cluster(5), lib.cluster(4)] }), async (p, lib) => {
  check(await p.locator('#un-kind').inputValue() === 'all', 'the filter starts at all');
  await p.locator('#un-kind').selectOption('pets');
  await settle(p, lib);
  await p.reload();
  await p.locator('#un-kind').waitFor();
  check(await p.locator('#un-kind').inputValue() === 'pets', 'after a reload the filter is not pets');
  await p.evaluate(() => localStorage.setItem('shoebox-unnamed-kind', 'nonsense'));
  await p.reload();
  await p.locator('#un-kind').waitFor();
  check(await p.locator('#un-kind').inputValue() === 'all', 'a value nobody offers is not taken');
}));

// ---- groups and people (the People page)

const person = (p, x) => p.locator(`#page .person[data-id="${x.id}"]`);
async function personMenu(p, x, key) {
  await person(p, x).locator('.more-btn').click();
  await p.locator('.menu').waitFor();
  await menu(p, key);
}

// "Check n faces" (or "Faces") opens a person's faces; their ⋯ menu has no
// second way there.
await test('the ⋯ menu of a person on the People page and on their page: no Review faces', await on('view=people', (lib) => lib.person('Rita Menu', { suggested: 3 }), async (p, lib, rita) => {
  const items = async () => {
    await p.locator('.menu').waitFor();
    const got = await p.locator('.menu [role=menuitem]').allTextContents();
    await p.keyboard.press('Escape');
    await p.locator('.menu').waitFor({ state: 'detached' });
    return got;
  };
  const words = (keys) => Promise.all(keys.map((k) => tr(p, k)));
  const edit = ['people.menu.rename', 'people.menu.move', 'people.menu.new_group', 'people.menu.merge'];
  await person(p, rita).locator('.more-btn').click();
  const tile = await items();
  const want = await words(edit);
  check(JSON.stringify(tile) === JSON.stringify(want), `People page: ${JSON.stringify(tile)}, expected ${JSON.stringify(want)}`);
  await p.evaluate((id) => { location.hash = `view=person&id=${id}&tab=suggested`; }, rita.id);
  await p.locator('#pp-grid .face').first().waitFor();
  await p.locator('.person-head .more-btn').click();
  const page = await items();
  check(JSON.stringify(page) === JSON.stringify(await words(edit)), `person's page: ${JSON.stringify(page)}`);
}));

// ---- a person's faces

await test('a person\'s page: "Show more" stands clear of the last row of faces', await on('view=people', (lib) => lib.person('Sam More', { suggested: 201 }), async (p, lib, sam) => {
  await p.evaluate((id) => { location.hash = `view=person&id=${id}&tab=suggested`; }, sam.id);
  const more = p.locator('#page button', { hasText: await tr(p, 'app.show_more') });
  await more.waitFor();
  await settle(p, lib);
  const gap = await p.evaluate(() => {
    const cards = document.querySelectorAll('#pp-grid .face');
    const btn = document.querySelector('#pp-grid + button');
    return btn.getBoundingClientRect().top - cards[cards.length - 1].getBoundingClientRect().bottom;
  });
  check(gap >= 12, `"Show more" is ${gap} px under the last row of faces`);
}));

// The faces the next "Show more" would bring come by themselves once the
// shown ones have all left; when none are left the page says so.
await test('a person\'s page: "Confirm all shown" brings the next faces without "Show more"', await on('view=people', (lib) => lib.person('Ines Next', { suggested: 250 }), async (p, lib, ines) => {
  await p.evaluate((id) => { location.hash = `view=person&id=${id}&tab=suggested`; }, ines.id);
  await p.locator('#pp-grid .face').first().waitFor();
  await settle(p, lib);
  const cards = () => p.locator('#pp-grid .face').count();
  const more = p.locator('#pp-grid + button');
  check((await cards()) === 200 && (await more.isVisible()), `at first: ${await cards()} faces, Show more ${await more.isVisible() ? 'shown' : 'hidden'}`);
  const confirmAll = async () => {
    const from = lib.log.length;
    await p.locator('.toolbar button', { hasText: await tr(p, 'person.confirm_all') }).click();
    await settle(p, lib);
    return lib.log.slice(from).filter((e) => e.method === 'GET' && e.path === `/people/${ines.id}/faces`).map((e) => `${e.query.get('state')} ${e.query.get('offset')}`);
  };
  const asked = await confirmAll();
  check(JSON.stringify(asked) === JSON.stringify(['suggested 0']), `after the first: asked for ${JSON.stringify(asked)}, expected the next faces once`);
  check((await cards()) === 50 && !(await more.isVisible()), `after the first: ${await cards()} faces, Show more ${await more.isVisible() ? 'shown' : 'hidden'}`);
  await confirmAll();
  check((await cards()) === 0 && (await p.locator('#pp-grid .sub').textContent()) === (await tr(p, 'person.none')), 'after the last: the page says there are none');
  check(lib.faceLists[ines.id].suggested.length === 0 && ines.faces === 253, `the fake has ${lib.faceLists[ines.id].suggested.length} suggestions left, ${ines.faces} faces`);
}));

// A face decided in the viewer opened from a person's page has left the tab
// when the viewer closes; the other cards stay as they were (no new page).
await test('a person\'s page: a face rejected in the viewer is gone from it once the viewer closes', await on('view=people', (lib) => lib.person('Vera View', { suggested: 3 }), async (p, lib, vera) => {
  await p.evaluate((id) => { location.hash = `view=person&id=${id}&tab=suggested`; }, vera.id);
  await p.locator('#pp-grid .face').first().waitFor();
  await settle(p, lib);
  const [a, b, c] = lib.faceLists[vera.id].suggested.map((f) => f.id);
  await p.evaluate(() => document.querySelectorAll('#pp-grid .face').forEach((x) => { x.dataset.old = '1'; }));
  const faces = (from) => lib.log.slice(from).filter((e) => e.method === 'GET' && e.path === `/people/${vera.id}/faces`).length;
  const view = async (id) => {
    await p.locator(`#pp-grid .face[data-key="f${id}"] .crop`).click();
    await p.locator('#lightbox').waitFor({ state: 'visible' });
    await settle(p, lib);
  };
  const close = async () => {
    await p.keyboard.press('Escape');
    await p.locator('#lightbox').waitFor({ state: 'hidden' });
    await settle(p, lib);
  };
  // Looked at only: nothing is asked for again.
  let from = lib.log.length;
  await view(a);
  await close();
  check(faces(from) === 0, `closed without a decision, the faces were asked for ${faces(from)} times`);
  // ✗ in the info panel.
  await view(b);
  await p.locator('#lb-info').click();
  await p.locator('#lb-panel .pface button.no').click();
  await posted(lib, /^\/faces\/reject$/);
  await settle(p, lib);
  check(await p.locator(`#pp-grid .face[data-key="f${b}"]`).count() === 1, 'the card left before the viewer closed');
  from = lib.log.length;
  await close();
  const left = await p.locator('#pp-grid .face').evaluateAll((xs) => xs.map((x) => [x.dataset.key, x.dataset.old]));
  check(JSON.stringify(left) === JSON.stringify([[`f${a}`, '1'], [`f${c}`, '1']]), `after the viewer: ${JSON.stringify(left)}, expected the other two cards as they were`);
  check(faces(from) === 1, `the faces were asked for ${faces(from)} times, expected once`);
  const tab = await p.locator('#person-tabs .tab[data-tab="suggested"]').textContent();
  check(tab.includes('(2)'), `the tab says ${tab}`);
}));

// One pixel of the page (r, g, b) at x, y, from a screenshot of just it.
async function pixel(p, x, y) {
  const png = await p.screenshot({ clip: { x, y, width: 1, height: 1 } });
  // A 1×1 PNG: after the signature, chunks of length, type, data, CRC; the
  // inflated IDAT is a filter byte and the pixel (filter 0 or 1 alike for one pixel).
  let at = 8, data = [];
  while (at < png.length) {
    const len = png.readUInt32BE(at), type = png.toString('ascii', at + 4, at + 8);
    if (type === 'IDAT') data.push(png.subarray(at + 8, at + 8 + len));
    at += 12 + len;
  }
  const raw = inflateSync(Buffer.concat(data));
  return [raw[1], raw[2], raw[3]];
}

// Confirmed faces are cut out of the photo's thumbnail (no stored crop): the
// selection's frame shows over that picture as over a crop.
await test('a person\'s page: a selected face shows its frame on Confirmed as on Suggested', await on('view=people', (lib) => lib.person('Olaf Outline', { faces: 3, suggested: 3 }), async (p, lib, olaf) => {
  const accent = await p.evaluate(() => {
    const e = document.createElement('div');
    e.style.color = 'var(--accent)';
    document.body.appendChild(e);
    const c = getComputedStyle(e).color.match(/\d+/g).slice(0, 3).map(Number);
    e.remove();
    return c;
  });
  for (const tab of ['suggested', 'confirmed']) {
    await p.evaluate(([id, t]) => { location.hash = `view=person&id=${id}&tab=${t}`; }, [olaf.id, tab]);
    await p.locator(`#person-tabs .tab.active[data-tab="${tab}"]`).waitFor();
    await p.locator('#pp-grid .face').nth(1).waitFor();
    await settle(p, lib);
    await p.locator('#pp-pick').click();
    const card = p.locator('#pp-grid .face').nth(1);
    await card.locator('.crop').click();
    check(await card.evaluate((c) => c.classList.contains('sel')), `${tab}: the card is not selected`);
    const box = await card.locator('.crop').boundingBox();
    const got = await pixel(p, box.x + 1, box.y + box.height / 2);
    check(got.every((v, i) => Math.abs(v - accent[i]) <= 8), `${tab}: the edge of the selected face is rgb(${got}), not the accent rgb(${accent})`);
  }
}));

await test('a person moved to a group, then both out of it: the empty group is gone', await on('view=people', (lib) => {
  const anton = lib.person('Anton Move'), bea = lib.person('Bea Move');
  lib.group('Move Group', bea);
  return { anton, bea };
}, async (p, lib, { anton, bea }) => {
  const move = (x, to) => changedThenGrouped(p, lib, `${x.name} to ${to}`, async () => {
    await personMenu(p, x, 'people.menu.move');
    await p.locator('#modal .taglist button', { hasText: to }).click();
    await posted(lib, /^\/people\/\d+\/group$/);
  });
  await move(anton, 'Move Group');
  check(await p.locator('#page .pgroup', { hasText: 'Move Group' }).locator(`.person[data-id="${anton.id}"]`).count() === 1, 'the People page shows Anton in Move Group');
  const none = await tr(p, 'people.no_group');
  await move(bea, none);
  await move(anton, none);
  check(!(await groupNames(p)).includes('Move Group'), 'the empty group left the sidebar');
}));

await test('a group on the People page folds and unfolds from its title; all start open', await on('view=people', (lib) => {
  lib.group('Fold One', lib.person('Fia One'));
  lib.group('Fold Two', lib.person('Gus Two'));
}, async (p, lib) => {
  const grp = (name) => p.locator('#page .pgroup', { hasText: name });
  for (const n of ['Fold One', 'Fold Two']) check(await grp(n).locator('.people').isVisible(), `${n} does not start open`);
  await grp('Fold One').locator('h3').click();
  check(await grp('Fold One').locator('.people').isHidden(), 'Fold One stays open after a click on its title');
  check(await grp('Fold One').locator('h3 .toggle').getAttribute('aria-expanded') === 'false', 'its arrow does not say folded');
  check(await grp('Fold Two').locator('.people').isVisible(), 'Fold Two folded with Fold One');
  await grp('Fold One').locator('h3 .toggle').click();
  check(await grp('Fold One').locator('.people').isVisible(), 'the arrow does not unfold the group');
}));

await test('a new group with a person',await on('view=people', (lib) => lib.person('Cleo New'), async (p, lib, cleo) => {
  await changedThenGrouped(p, lib, 'a new group', async () => {
    await personMenu(p, cleo, 'people.menu.new_group');
    await typeName(p, 'Cleo Group');
    await posted(lib, /^\/groups$/);
  });
  check((await groupNames(p)).includes('Cleo Group'), 'the sidebar has the new group');
}));

await test('groups ordered, renamed and deleted in the groups dialog', await on('view=people', (lib) => {
  lib.group('Order One', lib.person('Dan Groups'));
  lib.group('Order Two', lib.person('Eva Groups'));
}, async (p, lib) => {
  await p.locator('#page .toolbar button', { hasText: await tr(p, 'people.groups_btn') }).click();
  const row = (name) => p.locator('#modal .grow-row', { has: p.locator('.gname', { hasText: name }) });
  await changedThenGrouped(p, lib, 'Order Two moved up', async () => {
    await row('Order Two').locator('.grip').focus();
    await p.keyboard.press('Alt+ArrowUp');
    await posted(lib, /^\/groups\/reorder$/);
  });
  const names = await groupNames(p);
  check(names.indexOf('Order Two') < names.indexOf('Order One'), `the sidebar's groups: ${names}`);
  await changedThenGrouped(p, lib, 'Order One renamed', async () => {
    await row('Order One').locator(`button[data-tip="${await tr(p, 'people.rename')}"]`).click();
    await typeName(p, 'Order Renamed');
    await posted(lib, /^\/groups\/\d+\/rename$/);
  });
  check(await row('Order Renamed').isVisible(), 'the groups dialog is open again, with the new name');
  await changedThenGrouped(p, lib, 'Order Two deleted', async () => {
    await row('Order Two').locator(`button[data-tip="${await tr(p, 'app.delete')}"]`).click();
    await p.locator('#modal-actions .btn.danger').click();
    await posted(lib, /^\/groups\/\d+\/delete$/);
  });
  const after = await groupNames(p);
  check(after.includes('Order Renamed') && !after.includes('Order One') && !after.includes('Order Two'), `the sidebar's groups: ${after}`);
}));

await test('a person renamed', await on('view=people', (lib) => lib.person('Gus Rename'), async (p, lib, gus) => {
  await changedThenGrouped(p, lib, 'renamed', async () => {
    await personMenu(p, gus, 'people.menu.rename');
    await typeName(p, 'Aaron Renamed');
    await posted(lib, /^\/people\/\d+\/rename$/);
  });
}));

await test('a person merged into someone without a group: they take the group', await on('view=people', (lib) => {
  const hana = lib.person('Hana Merge'), ida = lib.person('Ida Merge', { faces: 5 });
  lib.group('Merge Group', hana);
  return { hana, ida };
}, async (p, lib, { hana, ida }) => {
  await changedThenGrouped(p, lib, 'merged', async () => {
    await personMenu(p, hana, 'people.menu.merge');
    await p.locator('#modal-body .pfield input').fill('Ida Merge');
    await p.locator('#modal-body .pfield input').press('Enter');
    await p.locator('#modal-actions button').last().click();
    await posted(lib, /^\/people\/\d+\/merge$/);
  });
  check(ida.group_id != null && !lib.people.includes(hana), 'Ida took the group, Hana is gone');
}));

await test('a person without faces deleted: their group goes with them', await on('view=people', (lib) => {
  const zed = lib.person('Zed Delete', { faces: 0 });
  lib.group('Delete Group', zed);
  lib.person('Someone Else');
  return zed;
}, async (p, lib, zed) => {
  await changedThenGrouped(p, lib, 'deleted', async () => {
    await personMenu(p, zed, 'people.menu.delete');
    await p.locator('#modal-actions .btn.danger').click();
    await posted(lib, /^\/people\/\d+\/delete$/);
  });
  check(!(await groupNames(p)).includes('Delete Group'), 'the group left the sidebar');
}));

// The naming dialog stays open until the server has answered: a second click,
// Enter or pick meanwhile must not send the naming again (a slow server once
// gave a video the same person three times). `hold` keeps the replies of
// /faces/assign back until `release` answers them.
async function holdAssign(p) {
  const held = [];
  await p.route(ORIGIN + '/**/faces/assign', (route) => { held.push(route); });
  return held;
}
const dialogButtons = (p) => p.evaluate(() => Array.from(document.querySelectorAll('#modal-actions button')).map((b) => b.disabled));
const clickSave = (p) => p.evaluate(() => { const b = document.querySelectorAll('#modal-actions button'); b[b.length - 1].click(); });

await test('the naming dialog sends one request while one is pending, and its buttons are disabled', await on('view=people', (lib) => lib.person('Pia Dialog'), async (p, lib, pia) => {
  const held = await holdAssign(p);
  await p.evaluate((id) => { nameFacesDialog([{ id }], null); }, pia.cover);
  const field = p.locator('#modal-body .pfield input');
  await field.fill('Pia Dialog');
  await field.press('Enter');
  await until('the request', () => held.length === 1);
  for (let i = 0; i < 3; i++) await clickSave(p);
  await p.evaluate(() => 0);
  check(held.length === 1, `the page sent ${held.length} namings while the first was pending, expected 1`);
  const disabled = await dialogButtons(p);
  check(disabled.length > 0 && disabled.every(Boolean), `the dialog's buttons are not disabled while it waits: ${disabled}`);
  check(await field.isDisabled(), 'the name field is not disabled while it waits');
  await held[0].fallback();
  await until('the dialog to close', () => p.evaluate(() => $('modal').hidden));
  check(lib.log.filter((e) => e.method === 'POST' && e.path.endsWith('/faces/assign')).length === 1, 'the face was named more than once');
}));

await test('a failed naming shows the error and was sent once', await on('view=people', (lib) => lib.person('Pia Failed'), async (p, lib, pia) => {
  const held = await holdAssign(p);
  await p.evaluate((id) => { nameFacesDialog([{ id }], null); }, pia.cover);
  await p.locator('#modal-body .pfield input').fill('Pia Failed');
  await clickSave(p);
  await clickSave(p);
  await until('the request', () => held.length === 1);
  await held[0].fulfill({ status: 500, contentType: 'application/json', body: JSON.stringify({ error: 'down' }) });
  await until('the error dialog', async () => (await p.locator('#modal-title').textContent()) === (await tr(p, 'app.failed')));
  check(held.length === 1, `the naming was sent ${held.length} times`);
}));

// ---- the search box

// Typing asks for the suggestions, all in one request, once the letters pause
// (150 ms), and for the photos after 300 ms. The server answers both one at a
// time in the order asked, so what was asked for an earlier text is called off
// when a newer one is asked for (see "Reads on a pool of connections" in
// docs/decisions.md).
await test('the search box asks once typing pauses and calls off what a newer text replaces', await on('', (lib) => lib.person('Longo Search'), async (p, lib) => {
  const from = lib.log.length;
  const asked = (path) => lib.log.slice(from).filter((e) => e.path === path).map((e) => e.query.get('q') || '');
  const calledOff = [];
  p.on('requestfailed', (r) => { const u = new URL(r.url()); calledOff.push(u.pathname.slice(LIB.length) + '?q=' + (u.searchParams.get('q') || '')); });
  lib.hold = /^\/suggest$/;
  // The page's time stands still but where the test moves it (the waits below take real time).
  await p.clock.pauseAt(await p.evaluate(() => Date.now()) + 1000);
  await p.focus('#search'); // the list for an empty box, at once
  for (const ch of 'lon') {
    await p.keyboard.type(ch);
    await p.clock.runFor(100);
  }
  check(asked('/suggest').join() === '', `asked while the letters came 100 ms apart: [${asked('/suggest')}]`);
  await p.clock.runFor(100);
  await until('the suggestions for "lon"', () => asked('/suggest').includes('lon'));
  check(asked('/suggest').join() === ',lon', `suggestions asked for [${asked('/suggest')}], expected once for "" (the box entered) and once for "lon"`);
  check(!asked('/timeline').length, 'the photos were asked for before 300 ms without a letter');
  // Another letter while the answer for "lon" waits: it is called off.
  await p.keyboard.type('g');
  await p.clock.runFor(200);
  await until('the suggestions for "long"', () => asked('/suggest').includes('long'));
  await until('the requests for "" and "lon" called off', () => calledOff.includes('/suggest?q=lon') && calledOff.includes('/suggest?q='));
  lib.release();
  await settle(p, lib);
  check(asked('/suggest').join() === ',lon,long', `suggestions asked for [${asked('/suggest')}], expected "", "lon", "long" once each`);
  check(asked('/timeline').join() === 'long', `the photos asked for [${asked('/timeline')}], expected once for "long"`);
  check(!calledOff.some((r) => r.endsWith('?q=long')), `what "long" asked for was called off: ${calledOff}`);
  const shown = await p.locator('#suggest .item .label').allTextContents();
  check(shown.includes('Longo Search'), `the list shows [${shown}]`);
}));

// What the search box asked for since `from`: the typed text of each request
// to `path`, and the requests the page called off ("/suggest?q=lon").
function searchLog(p, lib) {
  const from = lib.log.length, calledOff = [];
  p.on('requestfailed', (r) => { const u = new URL(r.url()); calledOff.push(u.pathname.slice(LIB.length) + '?q=' + (u.searchParams.get('q') || '')); });
  return { calledOff, asked: (path) => lib.log.slice(from).filter((e) => e.path === path).map((e) => e.query.get('q') || '') };
}
// A time window of real time in which nothing may happen (the page's clock stands still).
const quiet = () => new Promise((go) => setTimeout(go, 300));
// The page's time stands still but where the test moves it (the waits take real time).
const pauseClock = async (p) => p.clock.pauseAt(await p.evaluate(() => Date.now()) + 1000);

await test('the search box waits 150 ms after the last letter for the suggestions and 300 ms for the photos', await on('', (lib) => lib.person('Longo Search'), async (p, lib) => {
  await pauseClock(p);
  await p.focus('#search');
  await settle(p, lib);
  const { asked } = searchLog(p, lib);
  await p.keyboard.type('l');
  await p.clock.runFor(149);
  await p.keyboard.type('o'); // 149 ms after the "l": its suggestions and photos are never asked for
  await p.clock.runFor(149);
  await quiet();
  check(!asked('/suggest').length, `asked 149 ms after the last letter: [${asked('/suggest')}]`);
  await p.clock.runFor(1);
  await until('the suggestions 150 ms after the last letter', () => asked('/suggest').includes('lo'));
  await p.clock.runFor(149);
  await quiet();
  check(!asked('/timeline').length, `the photos asked for 299 ms after the last letter: [${asked('/timeline')}]`);
  await p.clock.runFor(1);
  await until('the photos 300 ms after the last letter', () => asked('/timeline').includes('lo'));
  await settle(p, lib);
  check(asked('/suggest').join() === 'lo', `suggestions asked for [${asked('/suggest')}], expected once for "lo"`);
  check(asked('/timeline').join() === 'lo', `photos asked for [${asked('/timeline')}], expected once for "lo"`);
}));

await test('the search box calls off the photos an older text asked for', await on('', () => null, async (p, lib) => {
  await pauseClock(p);
  await p.focus('#search');
  await settle(p, lib);
  const { asked, calledOff } = searchLog(p, lib);
  lib.hold = /^\/timeline$/;
  await p.keyboard.type('a');
  await p.clock.runFor(300);
  await until('the photos for "a"', () => asked('/timeline').includes('a'));
  await p.keyboard.type('b');
  await p.clock.runFor(300);
  await until('the photos for "ab"', () => asked('/timeline').includes('ab'));
  await until('the photos for "a" called off', () => calledOff.includes('/timeline?q=a'));
  lib.release();
  await settle(p, lib);
  check(asked('/timeline').join() === 'a,ab', `photos asked for [${asked('/timeline')}]`);
  check(!calledOff.includes('/timeline?q=ab'), 'the photos for "ab" were called off');
  check(await p.evaluate(() => state.filter.q) === 'ab', 'the search is not "ab"');
}));

await test('Escape and leaving the search box call off the suggestions on their way', await on('', () => null, async (p, lib) => {
  await pauseClock(p);
  await p.focus('#search');
  await settle(p, lib);
  const { asked, calledOff } = searchLog(p, lib);
  lib.hold = /^\/suggest$/;
  await p.keyboard.type('x');
  await p.clock.runFor(150);
  await until('the suggestions for "x"', () => asked('/suggest').includes('x'));
  await p.keyboard.press('Escape');
  await until('the suggestions for "x" called off by Escape', () => calledOff.includes('/suggest?q=x'));
  await p.keyboard.type('y');
  await p.clock.runFor(150);
  // (Escape also empties a search field in the browser.)
  await until('the suggestions for "y"', () => asked('/suggest').includes('y'));
  await p.evaluate(() => $('search').blur());
  await p.clock.runFor(150); // the list closes a moment after the box is left
  await until('the suggestions for "y" called off when the box was left', () => calledOff.includes('/suggest?q=y'));
  lib.release();
  await settle(p, lib);
  check(await p.locator('#suggest').isHidden(), 'the list is open after the box was left');
}));

await test('a list that takes a moment shows placeholder rows, a quick one does not', await on('', (lib) => lib.person('Spucky', { faces: 5, species: 'cat' }), async (p, lib) => {
  await pauseClock(p);
  const rows = () => p.locator('#suggest .wait-row').count();
  // Quick: the list is there before the placeholders are due.
  await p.locator('#search').click();
  await until('the list', async () => (await p.locator('#suggest .item').count()) > 0);
  await p.clock.runFor(500);
  check(await rows() === 0, 'placeholder rows next to a list that came at once');
  await p.keyboard.press('Escape');
  // Slow: placeholders from 120 ms on, until the list replaces them.
  lib.hold = /^\/suggest$/;
  await p.locator('#search').blur();
  await p.locator('#search').focus();
  await until('the request', () => lib.log.some((e) => e.path === '/suggest'));
  await p.clock.runFor(119);
  check(await rows() === 0, 'placeholder rows before 120 ms');
  await p.clock.runFor(1);
  check(await rows() > 0 && await p.locator('#suggest').isVisible(), 'no placeholder rows after 120 ms');
  check(await p.locator('#suggest').getAttribute('aria-busy') === 'true', 'the list does not say it is busy');
  lib.release();
  await until('the list', async () => (await p.locator('#suggest .item').count()) > 0);
  check(await rows() === 0, 'placeholder rows next to the list');
  check(await p.locator('#suggest').getAttribute('aria-busy') === null, 'the list still says it is busy');
}));

// The busy line under the top bar: shown when an api() request has run 0.4 s,
// gone when none is left. `ask` starts a request the page makes itself (api()
// is the page's own function); `/folders` is quiet on a calm page.
const busyLine = (p) => p.locator('#busy-line').isVisible();
const ask = (p, opts) => p.evaluate((o) => { window.asked = window.asked || []; const ctrl = new AbortController(); window.asked.push(ctrl); return void api(LIBAPI + '/folders', o && o.opted ? { busy: false } : { signal: ctrl.signal }).catch(() => {}); }, opts);

await test('the busy line: an answer within 0.4 s never shows it; a held one shows it at 0.4 s and it goes with the answer', await on('', () => null, async (p, lib) => {
  await pauseClock(p);
  check(!await busyLine(p), 'the line is there on a calm page');
  await ask(p);
  await until('the request', () => lib.log.some((e) => e.path === '/folders'));
  await settle(p, lib);
  check(!await busyLine(p), 'the line showed for an answer that came at once');
  lib.hold = /^\/folders$/;
  const from = lib.log.length;
  await ask(p);
  await until('the held request', () => lib.log.length > from);
  await p.clock.runFor(399);
  check(!await busyLine(p), 'the line before 0.4 s');
  await p.clock.runFor(1);
  check(await busyLine(p), 'no line at 0.4 s');
  lib.release();
  await until('the line to go with the answer', async () => !await busyLine(p));
}));

await test('the busy line stays until the last of two overlapping requests ends, and a request called off removes it too', await on('', () => null, async (p, lib) => {
  await pauseClock(p);
  lib.hold = /^\/folders$/;
  await ask(p);
  await p.clock.runFor(200);
  await ask(p);
  await until('both requests', () => lib.log.filter((e) => e.path === '/folders').length === 2);
  await p.clock.runFor(200);
  check(await busyLine(p), 'no line when the first had run 0.4 s');
  // The first is called off: one is left, the line stays.
  await p.evaluate(() => window.asked[0].abort());
  await p.evaluate(() => 0);
  check(await busyLine(p), 'the line went while the second was still open');
  await p.evaluate(() => window.asked[1].abort());
  await until('the line to go when the last is called off', async () => !await busyLine(p));
  lib.release();
}));

await test('the busy line: a request that opts out never shows it', await on('', () => null, async (p, lib) => {
  await pauseClock(p);
  lib.hold = /^\/folders$/;
  await ask(p, { opted: true });
  await until('the request', () => lib.log.some((e) => e.path === '/folders'));
  await p.clock.runFor(2000);
  check(!await busyLine(p), 'the line showed for a request with busy: false');
  lib.release();
}));

await test('the search suggestions held past 0.4 s show the grey rows and no busy line', await on('', (lib) => lib.person('Spucky', { faces: 5, species: 'cat' }), async (p, lib) => {
  await pauseClock(p);
  lib.hold = /^\/suggest$/;
  await p.locator('#search').focus();
  await until('the request', () => lib.log.some((e) => e.path === '/suggest'));
  await p.clock.runFor(500);
  check(await p.locator('#suggest .wait-row').count() > 0, 'no grey rows after 0.5 s');
  check(!await busyLine(p), 'the line showed next to the grey rows');
  lib.release();
  await until('the list', async () => (await p.locator('#suggest .item').count()) > 0);
}));

// The text row and the prefixes: the server counts what `text:` and `name:`
// (and the text row of a plain search) would find; a term that finds nothing
// gets no row (a prefix says why). `date:` asks for dates. Nothing else is
// asked for a number (the fake answers no `/count`).
const rows = async (p) => {
  const items = await p.locator('#suggest .item').evaluateAll((bs) => bs.map((b) => b.querySelector('.label').textContent + '=' + b.querySelector('.count').textContent + (b.classList.contains('off') ? ' (off)' : '')));
  const hints = await p.locator('#suggest > .hint').allTextContents();
  return { items, hints };
};
const typeIn = async (p, lib, text) => {
  await p.fill('#search', text);
  await settle(p, lib);
  return rows(p);
};
await test('the search box: the text row and text: and name: show what the server counted, nothing when it finds nothing', await on('', (lib) => {
  lib.textLines = 40;
  lib.words = { hello: 3 };
  lib.fileNames = { img_1: 2 };
}, async (p, lib) => {
  await p.focus('#search');
  await settle(p, lib);
  const from = lib.log.length;
  let r = await typeIn(p, lib, 'text:hello');
  check(r.items.join() === '“hello”=3', `text:hello shows [${r.items}]`);
  r = await typeIn(p, lib, 'text:zzz');
  check(!r.items.length && r.hints.includes(await tr(p, 'search.text_none', { term: 'zzz' })), `text:zzz shows [${r.items}], hints [${r.hints}]`);
  r = await typeIn(p, lib, 'name:IMG_1');
  check(r.items.join() === '“IMG_1”=2', `name:IMG_1 shows [${r.items}]`);
  r = await typeIn(p, lib, 'name:nope');
  check(!r.items.length && r.hints.includes(await tr(p, 'search.name_none', { term: 'nope' })), `name:nope shows [${r.items}], hints [${r.hints}]`);
  r = await typeIn(p, lib, 'hello');
  check(r.items.includes('“hello”=3'), `hello: the text row with its number, got [${r.items}]`);
  r = await typeIn(p, lib, 'zzz');
  check(!r.items.some((i) => i.startsWith('“')), `zzz: a text row that finds nothing, got [${r.items}]`);
  r = await typeIn(p, lib, 'text:');
  check(!r.items.length && r.hints.includes(await tr(p, 'search.prefix_empty_text')), `text: alone shows [${r.items}], hints [${r.hints}]`);
  await typeIn(p, lib, 'date:2019');
  const asked = lib.log.slice(from).filter((e) => e.path === '/suggest').map((e) => e.query.get('q'));
  check(asked.join() === 'text:hello,text:zzz,name:IMG_1,name:nope,hello,zzz,date:2019', `asked for [${asked}]`);
}));

await test('the search box before any text was read: the text row greyed, text: says how to read it', await on('', () => null, async (p, lib) => {
  await p.focus('#search');
  await settle(p, lib);
  let r = await typeIn(p, lib, 'hello');
  check(r.items.includes('“hello”= (off)'), `hello: the greyed text row, got [${r.items}]`);
  r = await typeIn(p, lib, 'text:hello');
  check(!r.items.length && r.hints.includes(await tr(p, 'search.text_unread')), `text:hello shows [${r.items}], hints [${r.hints}]`);
}));

// The trash page: a badge for each file that went with a photo, and for a
// copy removed as a duplicate one that opens the copy that stayed.
const trashed = (lib) => {
  const at = Date.parse('2026-05-30T12:00:00') / 1000;
  const item = (id, batch, path, kind, size, more) => ({ id, batch, path, kind, size, deleted_at: at, ...more });
  lib.trash = [
    item(1, 3, 'Südafrika/IMG_6714.HEIC', 'heic', 2_100_000),
    item(2, 3, 'Südafrika/IMG_6714.MOV', 'video', 3_400_000),
    // The server lists a batch by name: the RAW before its JPEG.
    item(5, 4, 'Selfies/IMG_1795.CR2', 'raw', 25_000_000),
    item(6, 4, 'Selfies/IMG_1795.JPG', 'jpeg', 4_000_000),
    item(3, 2, 'Selfies/IMG_1794.JPG', 'jpeg', 4_000_000, { duplicate_of: { id: 77, path: '2019-04 Kapstadt/IMG_1794.JPG', kind: 'jpeg', version: 'abcdef12' } }),
    item(4, 1, 'Downloads/IMG_2002 (2).JPG', 'jpeg', 900_000, { duplicate_of: { id: null, path: null, kind: null, version: null } }),
  ];
};

await test('trash cards: badges for the files that came along, the Duplicate badge opens the copy that stayed', await on('view=trash', trashed, async (p) => {
  await p.locator('.card.pick').first().waitFor();
  const cards = await p.locator('.card.pick').evaluateAll((cs) => cs.map((c) => ({
    name: c.querySelector('.name').textContent,
    meta: [...c.querySelectorAll('.meta')].map((m) => m.textContent).join(' | '),
    badges: [...c.querySelectorAll('.badge')].map((b) => [b.tagName, b.textContent, b.getAttribute('data-tip')]),
  })));
  const dup = await tr(p, 'trash.duplicate');
  check(cards[0].name === 'IMG_6714.HEIC' && JSON.stringify(cards[0].badges) === JSON.stringify([['SPAN', 'LIVE ▶', 'IMG_6714.MOV · 3.4 MB']]), `Live Photo card: ${JSON.stringify(cards[0])}`);
  check(!cards[0].meta.includes('MOV'), `the card still lists the video under it: ${cards[0].meta}`);
  check(cards[1].name === 'IMG_1795.JPG' && JSON.stringify(cards[1].badges) === JSON.stringify([['SPAN', 'RAW', 'IMG_1795.CR2 · 25.0 MB']]), `RAW card: ${JSON.stringify(cards[1])}`);
  cards.splice(1, 1);
  const tip = (await tr(p, 'trash.duplicate_of')).replace('{path}', '2019-04 Kapstadt/IMG_1794.JPG') + '\n' + (await tr(p, 'trash.duplicate_open'));
  check(JSON.stringify(cards[1].badges) === JSON.stringify([['BUTTON', dup, tip]]), `duplicate card: ${JSON.stringify(cards[1])}`);
  check(JSON.stringify(cards[2].badges) === JSON.stringify([['SPAN', dup, await tr(p, 'trash.duplicate_gone')]]), `duplicate whose kept copy is gone: ${JSON.stringify(cards[2])}`);

  // A click on the badge opens the kept photo, and does not select the card.
  await p.evaluate(() => { window.openFiles = (list, at) => { window.opened = { list, at }; }; });
  await p.locator('.card.pick').nth(2).locator('button.badge').click();
  const opened = await p.evaluate(() => window.opened);
  check(JSON.stringify(opened) === JSON.stringify({ list: [{ file: 77, kind: 'jpeg', version: 'abcdef12' }], at: 0 }), `opened ${JSON.stringify(opened)}`);
  check(!(await p.locator('.card.pick.sel').count()), 'the click on the badge selected the card');
  // The gone one opens nothing; a click there selects the card as before.
  await p.evaluate(() => { window.opened = null; });
  await p.locator('.card.pick').nth(3).locator('.badge').click();
  check(!(await p.evaluate(() => window.opened)) && (await p.locator('.card.pick').nth(3).evaluate((c) => c.classList.contains('sel'))), 'a click on the gone badge should select its card');
}));

await test('trash page: "Only duplicates" shows just the removed copies, and Select all and Delete for good reach only those', await on('view=trash', trashed, async (p, lib) => {
  await p.locator('.card.pick').first().waitFor();
  const toggle = p.locator('.trash-head input[type=checkbox]');
  check(await p.locator('.card.pick').count() === 4, 'all four photos shown at first');
  check((await p.locator('.trash-head label').textContent()).includes('2'), 'the toggle says how many duplicates there are');
  await toggle.check();
  await until('only the duplicates', async () => (await p.locator('.card.pick').count()) === 2);
  const names = await p.locator('.card.pick .name').allTextContents();
  check(JSON.stringify(names) === JSON.stringify(['IMG_1794.JPG', 'IMG_2002 (2).JPG']), `shown: ${names}`);
  const all = p.locator('.dup-bar button', { hasText: (await tr(p, 'trash.select_all')).replace('{count}', '2') });
  await all.click();
  check(await p.locator('.card.pick.sel').count() === 2, 'Select all ticks the two duplicates');
  // A red button has white words, not the accent's.
  check((await p.locator('.dup-bar .btn.danger').evaluate((e) => getComputedStyle(e).color)) === 'rgb(255, 255, 255)', 'the red button\'s label is not white');
  await p.locator('.dup-bar .btn.danger').click();
  await p.locator('#modal-actions .btn.danger').click();
  await until('the delete', async () => lib.trash.length === 4);
  // The photos trashed by hand are still there, and the toggle stays on until no duplicate is left.
  const left = lib.trash.map((t) => t.batch).sort().join();
  check(left === '3,3,4,4', `left in the trash: ${left}`);
  await until('the page to follow', async () => (await p.locator('.card.pick').count()) === 2);
  check(!(await toggle.isChecked()) && await toggle.isDisabled(), 'with no duplicate left the toggle is off and disabled');
}));

// What the page ticks by itself on the Duplicates screen: a copy with fewer
// pixels but a bigger file is left for the person (suggestedIds, Smaller versions).
await test('duplicates: a copy with fewer pixels but a bigger file is not ticked by Suggested or Smaller versions', await on('view=trash', () => null, async (p) => {
  const ids = await p.evaluate(() => {
    const f = (id, more) => ({ id, row: 0, pick: false, keeper: null, bigger_file: false, same: null, in_copies: false, ...more });
    const g = { kind: 'resolution', files: [f(1, { pick: true }), f(2, { keeper: 1 }), f(3, { keeper: 1, bigger_file: true })] };
    return { suggested: suggestedIds(g), smaller: PRESELECT.find((e) => e.id === 'smaller').ids(g) };
  });
  check(JSON.stringify(ids) === JSON.stringify({ suggested: [2], smaller: [2] }), `ticked: ${JSON.stringify(ids)}`);
}));

// The thumbnail of a same-photo group opens the viewer on the group's versions
// (card order), the arrow keys switch between them, Esc comes back to the thumbnail.
const dupFile = (id, name, w, h, size, more) => ({ id, path: 'Selfies/' + name, name, folder_id: 1, kind: 'jpeg', size, width: w, height: h, taken: '2015-11-19T17:32:00', version: 'abcdef1' + id, same: null, tags: [], row: 0, keeper: null, bigger_file: false, in_copies: false, pick: false, copy_named: false, ...more });
await test('duplicates: a click on the picture opens the viewer on the versions, left and right switch, Escape comes back to the group', await on('view=duplicates', (lib) => {
  lib.dupGroups = [
    { exact: false, kind: 'resolution', files: [dupFile(1, 'IMG_1794_1.JPG', 3888, 2592, 1_000_000, { pick: true }), dupFile(2, 'IMG_1794.JPG', 3793, 2529, 1_300_000, { keeper: 1, bigger_file: true })] },
    { exact: false, kind: 'resolution', files: [dupFile(3, 'IMG_2210.JPG', 4032, 3024, 3_100_000, { pick: true }), dupFile(4, 'IMG-WA0007.jpg', 1600, 1200, 240_000, { keeper: 3 })] },
  ];
}, async (p) => {
  await p.locator('.dup-photo a.thumb').first().waitFor();
  // Both cards of the conflict are red and nothing is ticked there.
  const badges = await p.locator('.dup-row').first().locator('.badge-chip.worse').allTextContents();
  check(JSON.stringify(badges) === JSON.stringify([await tr(p, 'dups.smaller_file'), await tr(p, 'dups.lower_quality')]), `badges ${badges}`);
  const tall = await p.locator('.dup-row .copy .badge-chip').evaluateAll((xs) => xs.map((e) => Math.round(e.getBoundingClientRect().height)));
  check(tall.length >= 2 && tall.every((x) => x === 26), `the cards' badges are ${tall} px high, expected 26 like every badge among text`);
  const second = p.locator('.dup-photo a.thumb').nth(1);
  await second.click();
  const open = () => p.evaluate(() => ({ at: state.open, ids: state.data && state.data.ids, hidden: $('lightbox').hidden }));
  await until('the viewer', async () => !(await open()).hidden);
  let o = await open();
  check(o.at === 0 && JSON.stringify(o.ids) === JSON.stringify([3, 4]), `viewer on ${JSON.stringify(o)}`);
  await p.keyboard.press('ArrowRight');
  await until('the next version', async () => (await open()).at === 1);
  await p.keyboard.press('ArrowLeft');
  await until('the first version again', async () => (await open()).at === 0);
  await p.keyboard.press('Escape');
  await until('the viewer to close', async () => (await open()).hidden);
  check(await second.evaluate((a) => document.activeElement === a), 'focus is not back on the thumbnail of the second group');
  check(await p.locator('.dup-photo a.thumb').count() === 2, 'the page was drawn again');
  await second.click();
  await until('the viewer again', async () => !(await open()).hidden);
  await p.keyboard.press(' ');
  await until('Space closes it', async () => (await open()).hidden);
}));

// The compare dialog of a series: "Keep this one" on every picture; the Keep
// marks and Keep (N) only where there are more than two to choose from.
for (const n of [2, 3]) {
  await test(`duplicates: the compare dialog of ${n} similar photos ${n > 2 ? 'has' : 'has no'} Keep marks`, await on('view=duplicates', (lib) => {
    lib.dupGroups = [{ exact: false, kind: 'similar', files: [1, 2, 3].slice(0, n).map((id) => dupFile(id, `IMG_${2396 + id}.JPG`, 4032, 3024, 3_000_000 + id, { row: id, pick: true })) }];
  }, async (p) => {
    await p.locator('a.thumb').first().click();
    await p.locator('.dialog.compare').waitFor();
    check(await p.locator('.dialog.compare .compare-col').count() === n, 'one picture per file');
    check(await p.locator('.dialog.compare .compare-buttons .btn:not(.keep-toggle)').count() === n, 'Keep this one on every picture');
    check(await p.locator('.dialog.compare .keep-toggle').count() === (n > 2 ? n : 0), `Keep marks for ${n}`);
    check(await p.locator('.dialog.compare .actions .btn').count() === (n > 2 ? 2 : 1), `Keep (N) next to Close for ${n}`);
  }));
}

// ---- Our own icons instead of emojis

const EMOJI = /[\u{1F000}-\u{1FAFF}\u{26A0}\u{270E}]/u;

await test('pets on the Unnamed page carry their icon in a disc in the corner, people do not; nothing is an emoji', await on('view=unnamed', (lib) => ({
  cat: lib.cluster(3, null, 'cat'), dog: lib.cluster(3, null, 'dog'), person: lib.cluster(3),
}), async (p, lib, s) => {
  const tile = (c, k) => card(p, first(c)).locator('.cface').nth(k);
  for (const [c, species] of [[s.cat, 'cat'], [s.dog, 'dog']]) {
    const faces = await card(p, first(c)).locator('.cface').count();
    check(faces === 3, `the ${species} card shows ${faces} faces`);
    for (let k = 0; k < 3; k++) {
      const badge = tile(c, k).locator('.avatar.pet-' + species + ' .pet-badge');
      check(await badge.count() === 1 && await badge.locator('svg').count() === 1, `${species} face ${k}: expected one badge with an icon`);
      check((await badge.textContent()).trim() === '', `${species} face ${k}: the badge has text`);
      // In the corner, inside the face's square (so no box around it cuts it off).
      const [b, a] = await Promise.all([badge.boundingBox(), tile(c, k).locator('.avatar').boundingBox()]);
      check(b.x >= a.x && b.y >= a.y && b.x + b.width <= a.x + a.width + 0.5 && b.y + b.height <= a.y + a.height + 0.5, `${species} face ${k}: the badge is outside the face's square`);
      check(b.x + b.width / 2 > a.x + a.width / 2 && b.y + b.height / 2 > a.y + a.height / 2, `${species} face ${k}: the badge is not at the bottom right`);
    }
  }
  check(await card(p, first(s.person)).locator('.pet-badge').count() === 0, 'a person has a pet badge');
  // The old badge was a character after the face; the ring is not clipped by the circle now.
  const after = await p.locator('.avatar.pet').first().evaluate((a) => [getComputedStyle(a, '::after').content, getComputedStyle(a).overflow]);
  check(after[0] === 'none' || after[0] === 'normal' || after[0] === '""', `a text after the face: ${after[0]}`);
  check(after[1] === 'visible', `the pet's face clips what reaches past its circle: ${after[1]}`);
  const text = await p.evaluate(() => document.body.innerText);
  check(!EMOJI.test(text), 'the page shows an emoji: ' + text.match(EMOJI));
}));

await test('a pet in the people list has its icon before the name, a person has none; the head of a pet likewise', await on('view=people', (lib) => ({
  layka: lib.person('Layka', { faces: 3, species: 'dog' }), kim: lib.person('Kim', { faces: 3 }),
}), async (p, lib, s) => {
  const name = (who) => p.locator(`#page .person[data-id="${who.id}"] .pname`);
  const dog = name(s.layka);
  check(await dog.locator('svg').count() === 1 && (await dog.textContent()).trim() === 'Layka', `the dog's name: ${await dog.innerHTML()}`);
  check(await dog.evaluate((n) => n.firstChild.classList.contains('ic') && n.lastChild.nodeType === 3), 'the icon is not before the name');
  check(await name(s.kim).locator('svg').count() === 0, 'a person has an icon before the name');
  check(await p.locator(`#page .person[data-id="${s.layka.id}"] .avatar.pet-dog .pet-badge`).count() === 0, 'the people list shows the pet badge on a person');
  // The labels of the other places (head of a pet, tags, info panel) are made the same way.
  const kinds = await p.evaluate(() => ['cat', 'dog', 'pet', null].map((sp) => { var e = petLabel('h2', 'pname', sp, 'Tommy'); return [e.querySelectorAll('svg').length, e.textContent]; }));
  check(JSON.stringify(kinds) === JSON.stringify([[1, 'Tommy'], [1, 'Tommy'], [1, 'Tommy'], [0, 'Tommy']]), `labels: ${JSON.stringify(kinds)}`);
  const missing = await p.evaluate(() => ['person', 'cat', 'dog', 'paw', 'place', 'drive', 'pencil'].filter((n) => !/<svg[^>]*>.*<\/svg>/.test(ICONS[n] || '')));
  check(!missing.length, 'icons missing: ' + missing);
  // A thumbnail that does not load shows a picture icon, not a character.
  const broken = await p.evaluate(() => {
    document.body.insertAdjacentHTML('beforeend', '<div class="cell broken" style="width:80px;height:80px"><div class="ph"></div></div>');
    var st = getComputedStyle(document.querySelector('.cell.broken .ph'), '::after');
    return [st.content, st.maskImage || st.webkitMaskImage, st.width];
  });
  check(broken[0] === '""' && /svg/.test(broken[1]) && broken[2] === '28px', `broken thumbnail: ${JSON.stringify(broken)}`);
}));

await test('the kind picker of "Add face or pet" has an icon on each button (person, cat, dog), the arrows still move the choice', await on('view=people', () => null, async (p) => {
  await p.evaluate(() => { nameDrawnFace({ id: 1, kind: 'jpeg' }, [0.1, 0.1, 0.2, 0.2]); });
  const buttons = p.locator('#modal .kinds button');
  check(await buttons.count() === 3, 'the dialog has no three kinds');
  const labels = [await tr(p, 'draw.kind.person'), await tr(p, 'draw.kind.cat'), await tr(p, 'draw.kind.dog')];
  for (let k = 0; k < 3; k++) {
    const b = buttons.nth(k);
    check(await b.locator('svg').count() === 1 && (await b.textContent()).trim() === labels[k], `kind ${k}: ${await b.innerHTML()}`);
    check(!EMOJI.test(await b.textContent()), `kind ${k} has an emoji`);
  }
  check(await buttons.nth(0).getAttribute('aria-checked') === 'true', 'person is not the first choice');
  await buttons.nth(0).focus();
  await p.keyboard.press('ArrowRight');
  check(await buttons.nth(1).getAttribute('aria-checked') === 'true' && await buttons.nth(1).evaluate((b) => document.activeElement === b), 'the arrow did not move the choice to the cat');
}));

await test('a place chip has its marker only; the marker opens the map; the folder chip edits with a pencil icon', await on('view=people', () => null, async (p) => {
  await p.evaluate(() => {
    geo.places = [{ id: 7, name: 'Mainau' }];
    state.folderById = { 3: { id: 3, name: 'Trip', path: 'Trip' } };
    window.shown = [];
    window.showView = function (v, id) { window.shown.push([v, id]); };
    state.filter.view = null;
    state.filter.place = 7;
    renderChips();
  });
  // The filter bar also has the "+" that adds a term; the place chip is the first.
  const chip = p.locator('#filters .chip').first();
  check(await chip.locator('.ic.link').count() === 1, 'the place chip is missing');
  check(await chip.locator('button').count() === 1, 'the chip has more buttons than its remove button');
  check(await chip.locator('svg').count() === 1 && !EMOJI.test(await chip.textContent()), `the chip: ${await chip.innerHTML()}`);
  const mark = chip.locator('.ic.link');
  check(await mark.getAttribute('role') === 'button' && await mark.getAttribute('aria-label') === await tr(p, 'chip.show_on_map'), 'the marker is not a labelled button');
  await mark.click();
  await mark.focus();
  await p.keyboard.press('Enter');
  check(JSON.stringify(await p.evaluate(() => window.shown)) === '[["locations",7],["locations",7]]', 'the marker did not open the place on the map: ' + JSON.stringify(await p.evaluate(() => window.shown)));
  await p.evaluate(() => { state.filter.place = null; state.filter.folder = 3; readOnly = false; renderChips(); });
  const edit = p.locator('#filters .chip .edit');
  check(await edit.count() === 1 && await edit.locator('svg').count() === 1 && (await edit.textContent()) === '', `the folder chip's edit button: ${await edit.innerHTML()}`);
}));

await test('the place panel on the map has a button that shows the place in the timeline, without the map', await on('view=locations&id=7', () => null, async (p) => {
  await p.evaluate(() => {
    geo.places = [{ id: 7, name: 'Mainau', count: 2 }];
    geo.sel = { kind: 'place', id: 7 };
    geo.data = { count: 2, ids: [1, 2], kinds: 'jj', days: [20260315, 20260315], versions: 'orig0001orig0002', live: [], favs: [] };
    geo.shown = 0;
    geo.ui = { side: document.createElement('div') };
    renderSide();
    window.sideEl = geo.ui.side;
  });
  const label = await tr(p, 'geo.show_timeline');
  const mine = await p.evaluate((l) => [...window.sideEl.querySelectorAll('.geo-tools button')].map((b) => b.getAttribute('aria-label')).indexOf(l), label);
  check(mine === 0, `the timeline button is not the first tool of the place panel (index ${mine})`);
  check(await p.evaluate(() => window.sideEl.querySelector('.geo-tools button svg') !== null), 'the timeline button has no icon');
  await p.evaluate(() => window.sideEl.querySelector('.geo-tools button').click());
  await p.waitForFunction(() => /place=7/.test(location.hash) && !/view=/.test(location.hash));
  const hash = await p.evaluate(() => location.hash);
  check(!/view=|[?&#]id=/.test(hash), `the map page is still in the address: ${hash}`);
  check(await p.evaluate(() => state.filter.place === 7 && !state.filter.view), 'the place is not a chip of the timeline');
}));

// The Locations page with Leaflet, maps on: 200 photos on Lake Constance, more
// than the photo list shows at first (150, then "Show more").
function mapPhotos(lib) {
  lib.maps = true;
  lib.photos = Array.from({ length: 200 }, (_, k) => ({ id: k + 1, version: `pos${String(k).padStart(5, '0')}`, pos: [47.65 + k * 0.0001, 9.17] }));
}
const LAKE = { south: 47.6, west: 9.1, north: 47.7, east: 9.3 };

async function mapReady(p, lib) {
  await until('the map', async () => { await p.clock.runFor(100); return p.evaluate(() => !!(geo.map && geo.ui)); });
  await settle(p, lib);
}

// The photo in the list that has the keyboard focus (-1: none).
const sideFocus = (p) => p.evaluate(() => (geo.ui.grid ? [...geo.ui.grid.children].indexOf(document.activeElement) : -1));

await test('the photo list on the map: the arrow keys move in it, and the viewer gives back the photo seen last', await on('view=locations', mapPhotos, async (p, lib) => {
  await mapReady(p, lib);
  check(await p.evaluate(() => [...document.querySelectorAll('.geo-side .icon-btn')].every((b) => b.offsetWidth === b.offsetHeight && b.offsetWidth > 0)), 'a button of the photo list is not round (wider than high)');
  await p.evaluate((a) => selectArea(a), LAKE);
  await settle(p, lib);
  check(await p.evaluate(() => geo.data.count) === 200 && await p.evaluate(() => geo.ui.grid.children.length) === 150, 'the list does not show the first 150 of 200 photos');
  check(await sideFocus(p) === 0, `the picked photos did not take the keyboard focus (${await sideFocus(p)})`);
  const cols = await p.evaluate(() => getComputedStyle(geo.ui.grid).gridTemplateColumns.split(' ').length);
  check(cols > 1, `the list has ${cols} column`);
  const keys = [['ArrowRight', 1], ['ArrowDown', 1 + cols], ['ArrowUp', 1], ['ArrowLeft', 0], ['ArrowLeft', 0], ['ArrowUp', 0]];
  for (const [key, want] of keys) {
    await p.keyboard.press(key);
    check(await sideFocus(p) === want, `${key}: photo ${await sideFocus(p)} has the focus, expected ${want}`);
  }
  // Space opens the viewer; three photos on, Space closes it on the third.
  await p.keyboard.press('Space');
  await settle(p, lib);
  check(await p.evaluate(() => !$('lightbox').hidden && state.open === 0), 'Space did not open the photo');
  for (let k = 0; k < 3; k++) { await p.keyboard.press('ArrowRight'); await settle(p, lib); }
  await p.keyboard.press('Space');
  await settle(p, lib);
  check(await p.evaluate(() => $('lightbox').hidden), 'Space did not close the viewer');
  check(await sideFocus(p) === 3, `after the viewer photo ${await sideFocus(p)} has the focus, expected 3 (the one seen last)`);
  check(await p.evaluate(() => state.data !== geo.data), 'the timeline did not get its own photos back');
  // Seen last beyond the first 150: the list shows more, to that one.
  await p.evaluate(() => focusSideCell(148));
  await p.keyboard.press('Space');
  await settle(p, lib);
  for (let k = 0; k < 3; k++) { await p.keyboard.press('ArrowRight'); await settle(p, lib); }
  await p.keyboard.press('Escape');
  await settle(p, lib);
  check(await sideFocus(p) === 151, `after the viewer photo ${await sideFocus(p)} has the focus, expected 151`);
  await p.keyboard.press('Home');
  check(await sideFocus(p) === 0, 'Home: not the first photo');
  await p.keyboard.press('End');
  check(await sideFocus(p) === 199, `End: photo ${await sideFocus(p)}, not the last`);
  check(await p.evaluate(() => $('lightbox').hidden), 'a key in the list opened the viewer');
}));

await test('the border between map and photo list widens the list, within the least widths of both, and is kept', await on('view=locations', mapPhotos, async (p, lib) => {
  await mapReady(p, lib);
  await p.evaluate((a) => selectArea(a), LAKE);
  await settle(p, lib);
  const widths = () => p.evaluate(() => ({ side: Math.round(geo.ui.side.getBoundingClientRect().width), map: Math.round(geo.ui.main.getBoundingClientRect().width), leaflet: geo.map.getSize().x }));
  const drag = async (by) => {
    const b = await p.locator('.geo-resize').boundingBox();
    const x = b.x + b.width / 2, y = b.y + b.height / 2;
    await p.mouse.move(x, y);
    await p.mouse.down();
    await p.mouse.move(x + by / 2, y, { steps: 3 });
    await p.mouse.move(x + by, y, { steps: 3 });
    await p.mouse.up();
    await settle(p, lib);
  };
  const start = await widths();
  check(start.side === 360, `the list starts ${start.side}px wide, not 360`);
  await drag(-300);
  let w = await widths();
  check(Math.abs(w.side - 660) <= 1, `dragged 300px to the left the list is ${w.side}px wide, expected 660`);
  check(w.leaflet === w.map, `the map did not take its new size (${w.leaflet}px in a box of ${w.map})`);
  check(await p.evaluate(() => localStorage.getItem('geoSideWidth')) === String(w.side), 'the width is not kept');
  check(await p.evaluate(() => getComputedStyle(geo.ui.grid).gridTemplateColumns.split(' ').length) > 3, 'the wider list has no more columns');
  await drag(-5000);
  w = await widths();
  check(w.map === 320, `dragged far to the left the map is ${w.map}px wide, expected its least 320`);
  await drag(5000);
  w = await widths();
  check(w.side === 360, `dragged far to the right the list is ${w.side}px wide, expected its least 360`);
  // The keyboard moves the border too.
  await p.locator('.geo-resize').focus();
  await p.keyboard.press('ArrowLeft');
  await p.keyboard.press('ArrowLeft');
  w = await widths();
  check(w.side === 440, `two times ArrowLeft on the border: the list is ${w.side}px wide, expected 440`);
  // A new visit (the page loaded again) has the width the user left.
  await p.reload();
  await until('the page to start again', async () => { await p.clock.runFor(100); return p.evaluate(() => !!(window.people && people.info)); });
  await mapReady(p, lib);
  w = await widths();
  check(w.side === 440, `after a reload the list is ${w.side}px wide, expected 440`);
}));

// A JPEG turned in the viewer gets a new quick hash, so the grid's thumbnail is
// another address (`?v=`). The grid asks for it once the user has stopped
// turning (three turns in a row are one request), not after each turn and not
// only at the next scan.
await test('a photo turned in the viewer: the grid asks for its new thumbnail once, after the last turn', await on('', (lib) => {
  lib.photos = [1, 2, 3].map((id) => ({ id, version: `orig000${id}` }));
}, async (p, lib) => {
  const cells = p.locator('#sizer a.cell');
  await until('the grid', async () => (await cells.count()) === 3);
  const asked = () => lib.thumbs.filter((a) => a.includes('/thumb?v=rot'));
  check(lib.thumbs.includes('/files/1/thumb?v=orig0001'), `the grid asked for ${JSON.stringify(lib.thumbs)}`);
  await cells.first().click();
  await p.locator('#lightbox').waitFor({ state: 'visible' });
  await settle(p, lib);
  for (let n = 1; n <= 3; n++) {
    await p.keyboard.press('r');
    await until(`turn ${n} to be answered`, async () => lib.log.filter((e) => e.method === 'POST' && e.path === '/files/1/rotate').length === n && !(await p.evaluate(() => lb.rotating)));
    // 600 ms a turn: the first pause would be over before the last turn if each turn did not start it again.
    await p.clock.runFor(600);
    await p.evaluate(() => 0);
    check(asked().length === 0, `after turn ${n} the grid asked for ${JSON.stringify(asked())}`);
  }
  await p.clock.runFor(300);
  await p.evaluate(() => 0);
  check(asked().length === 0, `before the pause is over the grid asked for ${JSON.stringify(asked())}`);
  await p.clock.runFor(200);
  await until('the new thumbnail', () => asked().length > 0);
  await settle(p, lib);
  check(JSON.stringify(asked()) === '["/files/1/thumb?v=rot00003"]', `after the last turn the grid asked for ${JSON.stringify(asked())}`);
  const srcs = await cells.evaluateAll((as) => as.map((a) => a.querySelector('img').getAttribute('src')));
  check(JSON.stringify(srcs) === JSON.stringify(['/api/lib/' + LIB.split('/').pop() + '/files/1/thumb?v=rot00003', '/api/lib/' + LIB.split('/').pop() + '/files/2/thumb?v=orig0002', '/api/lib/' + LIB.split('/').pop() + '/files/3/thumb?v=orig0003']), `the grid shows ${JSON.stringify(srcs)}`);
}));

// ---- hidden from the timeline (phase 20)

// The page applied what was done (a hash change, then its requests) and drew it: the time runs on, round
// after round, until `ok` says so. (`settle` alone can find the page calm before a hash change was heard.)
async function settleUntil(p, lib, what, ok) {
  for (let round = 0; round < 60; round++) {
    await p.clock.runFor(500);
    await until('the fake to answer', () => lib.open === 0 && lib.waiting() === 0);
    await p.evaluate(() => 0);
    if (await ok()) return;
  }
  throw new Error(`${what} never happened`);
}
const gridIs = (p, ids) => async () => JSON.stringify(await gridIds(p)) === JSON.stringify(ids);
const gridIds = (p) => p.locator('#sizer a.cell').evaluateAll((as) => as.map((a) => Number(a.querySelector('img').getAttribute('src').match(/files\/(\d+)\//)[1])));
const hiddenPhotos = (lib) => { lib.photos = [1, 2, 3].map((id) => ({ id, version: `orig000${id}` })); };
const cellCount = (p, n) => until(`${n} photos in the grid`, async () => (await p.locator('#sizer a.cell').count()) === n);
const postedTo = (lib, path) => lib.log.filter((e) => e.method === 'POST' && e.path === path);
// The icon of an action is the eye with a slash (hide) or without (show again).
const slashed = (p, sel) => p.evaluate((s) => !!document.querySelector(s + ' svg path[d="M4.5 20L19.5 4"]'), sel);

await test('hiding in the viewer (key h): the photo stays until the viewer closes, then the grid loses it; the Hidden chip shows it', await on('', hiddenPhotos, async (p, lib) => {
  const cells = p.locator('#sizer a.cell');
  await cellCount(p, 3);
  check(await p.locator('#hidden-row').count() === 0, 'a Hidden row with nothing hidden');
  await cells.first().click();
  await p.locator('#lightbox').waitFor({ state: 'visible' });
  await settle(p, lib);
  check(await slashed(p, '#lb-hide'), 'the hide button is not the eye with a slash');
  check((await p.locator('#lb-hide').getAttribute('data-tip')) === await tr(p, 'hidden.hide_hint'), 'tip of the hide button');
  await p.keyboard.press('h');
  await posted(lib, /^\/hidden$/);
  await settle(p, lib);
  check(JSON.stringify(postedTo(lib, '/hidden')[0] && lib.log.filter((e) => e.path === '/hidden').length) === '1', 'one POST to /hidden');
  check(await p.locator('#lb-hide.on').count() === 1 && !(await slashed(p, '#lb-hide')), 'the button is the plain eye, lit, once hidden');
  check((await p.locator('#lb-hide').getAttribute('data-tip')) === await tr(p, 'hidden.show_hint'), 'tip when hidden');
  check((await p.locator('#toast').textContent()) === await tr(p, 'hidden.toast_hidden.one', { n: 1 }), 'toast: ' + await p.locator('#toast').textContent());
  check(await p.locator('#sizer a.cell').count() === 3, 'the grid keeps its photos while the viewer is open');
  await p.keyboard.press('Escape');
  await cellCount(p, 2);
  await settle(p, lib);
  check(JSON.stringify(await gridIds(p)) === '[2,3]', `the grid shows ${await gridIds(p)}`);
  check(await p.locator('#hidden-row').count() === 0, 'the sidebar offers a Hidden row');
  // The Hidden chip: only the hidden photos.
  await p.evaluate(() => setFilter(withFilter({ hidden: true })));
  await settleUntil(p, lib, 'the Hidden chip list', gridIs(p, [1]));
  check(JSON.stringify(await gridIds(p)) === '[1]', `the Hidden chip shows ${await gridIds(p)}`);
  check(await p.locator('#filters .chip.hid').count() === 1, 'the Hidden chip');
  check(lib.log.some((e) => e.path === '/timeline' && e.query.get('hidden') === '1'), 'the timeline was asked with hidden=1');
  // In the viewer the eye shows it again.
  await p.locator('#sizer a.cell').first().click();
  await p.locator('#lightbox').waitFor({ state: 'visible' });
  await settle(p, lib);
  check(await p.locator('#lb-hide.on').count() === 1, 'the photo is hidden, the button is lit');
  await p.keyboard.press('h');
  await until('showing again', () => postedTo(lib, '/hidden').length === 2);
  check(postedTo(lib, '/hidden')[1] && lib.photos[0].hidden === false, 'the photo is shown again');
}));

await test('the selection bar: the eye hides the selected photos, and in the hidden list it shows them again', await on('', hiddenPhotos, async (p, lib) => {
  await cellCount(p, 3);
  await p.locator('#select').click();
  await p.locator('#sizer a.cell').nth(0).click();
  await p.locator('#sizer a.cell').nth(1).click();
  check(await slashed(p, '#sel-hide'), 'the bar offers the eye with a slash');
  check((await p.locator('#sel-hide').getAttribute('data-tip')) === await tr(p, 'sel.hide'), 'tip: ' + await p.locator('#sel-hide').getAttribute('data-tip'));
  await p.locator('#sel-hide').click();
  await posted(lib, /^\/hidden$/);
  await settle(p, lib);
  const body = lib.log.filter((e) => e.path === '/hidden').length;
  check(body === 1 && lib.photos.filter((x) => x.hidden).map((x) => x.id).join() === '1,2', 'two photos hidden');
  check((await p.locator('#toast').textContent()) === await tr(p, 'hidden.toast_hidden.other', { n: 2 }), 'toast: ' + await p.locator('#toast').textContent());
  await cellCount(p, 1);
  check(await p.locator('#selbar').isHidden(), 'the selection ends');
  // The hidden list: all selected are hidden, so the bar offers to show them.
  await p.evaluate(() => setFilter(withFilter({ hidden: true })));
  await settle(p, lib);
  await cellCount(p, 2);
  await p.locator('#select').click();
  await p.locator('#sel-all').click();
  check(!(await slashed(p, '#sel-hide')), 'the bar offers the plain eye when all are hidden');
  check((await p.locator('#sel-hide').getAttribute('data-tip')) === await tr(p, 'sel.unhide'), 'tip: ' + await p.locator('#sel-hide').getAttribute('data-tip'));
  await p.locator('#sel-hide').click();
  await until('showing again', () => lib.photos.every((x) => !x.hidden));
  await settle(p, lib);
  check(JSON.stringify(lib.log.filter((e) => e.path === '/hidden').map((e) => e.method)) === '["POST","POST"]', 'two POSTs');
}));

await test('a folder: the chip\'s eye and the right-click menu hide it, it stays dimmed in the tree and shows its photos when opened', await on('folder=2', (lib) => {
  lib.folders = [{ id: 1, parent: null, name: '', path: '', count: 3 }, { id: 2, parent: 1, name: 'Privat', path: 'Privat', count: 1 }, { id: 3, parent: 1, name: 'Urlaub', path: 'Urlaub', count: 2 }];
  lib.photos = [{ id: 1, version: 'orig0001', folder: 2 }, { id: 2, version: 'orig0002', folder: 3 }, { id: 3, version: 'orig0003', folder: 3 }];
}, async (p, lib) => {
  await cellCount(p, 1);
  const eye = p.locator('#filters .chip .eye');
  check(await eye.count() === 1 && await slashed(p, '#filters .chip .eye'), 'the folder chip has the eye with a slash');
  check((await eye.getAttribute('data-tip')) === await tr(p, 'hidden.folder_hide'), 'chip tip');
  await eye.click();
  await posted(lib, /^\/hidden\/folder$/);
  await settleUntil(p, lib, 'the folder shown as hidden', async () => (await p.locator('#filters .chip .eye.on').count()) === 1 && (await p.locator('#tree .row.hidden-folder').count()) === 1);
  check(JSON.stringify(lib.log.filter((e) => e.path === '/hidden/folder').length) === '1' && lib.folders[1].hidden_own, 'the folder is flagged');
  check(await p.locator('#filters .chip .eye.on').count() === 1 && !(await slashed(p, '#filters .chip .eye')), 'the chip shows the plain eye, lit');
  check(await p.locator('#tree .row.hidden-folder').count() === 1, 'the hidden folder is dimmed in the tree');
  check(await p.locator('#tree .row.hidden-folder .ico').count() === 1, 'with its small eye');
  check(JSON.stringify(await gridIds(p)) === '[1]', 'the opened hidden folder shows its photo');
  // All photos: the hidden folder's photo is not in it, and the count in the sidebar says so.
  await p.locator('#all').click();
  await settleUntil(p, lib, 'the timeline', gridIs(p, [2, 3]));
  check(JSON.stringify(await gridIds(p)) === '[2,3]', `the timeline shows ${await gridIds(p)}`);
  check(await p.locator('#tree .row.hidden-folder .name').count() === 1, 'the folder is still in the tree');
  // Right-click on the folder: the menu shows it again.
  await p.locator('#tree .row.hidden-folder .name').click({ button: 'right' });
  const entry = p.locator('.menu button');
  check((await entry.allTextContents()).join() === await tr(p, 'hidden.folder_show'), 'menu: ' + await entry.allTextContents());
  await entry.click();
  await until('the folder shown again', () => !lib.folders[1].hidden_own);
  await settleUntil(p, lib, 'the folder not dimmed any more', async () => (await p.locator('#tree .row.hidden-folder').count()) === 0);
  check(await p.locator('#tree .row.hidden-folder').count() === 0, 'not dimmed any more');
  // Right-click on a folder that is not hidden: the menu hides it.
  await p.locator('#tree .name', { hasText: 'Urlaub' }).click({ button: 'right' });
  check((await p.locator('.menu button').allTextContents()).join() === await tr(p, 'hidden.folder_hide'), 'menu for a normal folder');
  await p.keyboard.press('Escape');
}));

await test('typing versteckt as a tag: the photo is hidden, no tag is added, and a toast says so', await on('', hiddenPhotos, async (p, lib) => {
  await cellCount(p, 3);
  await p.locator('#sizer a.cell').first().click();
  await p.locator('#lightbox').waitFor({ state: 'visible' });
  await p.keyboard.press('i');
  await settle(p, lib);
  await p.locator('#lb-panel .tags .add').click();
  await p.locator('#lb-panel .tags input').fill('Versteckt');
  await p.keyboard.press('Enter');
  await posted(lib, /^\/hidden$/);
  await settle(p, lib);
  check(!postedTo(lib, '/tags/add').length, 'a tag was added');
  check(lib.photos[0].hidden === 'tag', 'the photo is hidden');
  check((await p.locator('#toast').textContent()) === await tr(p, 'tags.reserved_hidden'), 'toast: ' + await p.locator('#toast').textContent());
  check(await p.locator('#lb-hide.on').count() === 1, 'the eye is lit');
  check(await p.locator('#lb-panel .tags .own').count() === 0, 'no tag chip for it');
}));

await test('the search box offers Hidden for hidden, versteckt, verborgen or ausgeblendet while something is hidden, and picking it adds the chip', await on('', (lib) => {
  lib.photos = [1, 2, 3].map((id) => ({ id, version: `orig000${id}`, hidden: id === 1 ? 'tag' : false }));
}, async (p, lib) => {
  await p.focus('#search');
  await settle(p, lib);
  const label = await tr(p, 'hidden.label');
  for (const word of ['hid', 'Versteckt', 'verb', 'ausg']) {
    const r = await typeIn(p, lib, word);
    check(r.items.some((i) => i === label + '=1'), `${word} shows [${r.items}]`);
  }
  const r = await typeIn(p, lib, 'zz');
  check(!r.items.some((i) => i.startsWith(label)), `zz shows [${r.items}]`);
  await typeIn(p, lib, 'hidd');
  await p.locator('#suggest .item').first().click();
  await settleUntil(p, lib, 'the Hidden filter', gridIs(p, [1]));
  check(await p.evaluate(() => state.filter.hidden === true && location.hash.includes('hidden=1')), 'the filter has hidden=1');
  check(await p.locator('#filters .chip.hid').count() === 1, 'the Hidden chip');
  check(JSON.stringify(await gridIds(p)) === '[1]', `the grid shows ${await gridIds(p)}`);
}));

// The info panel's list of faces in Select mode: a click on a row ticks it,
// Shift-click ticks everything from the last one clicked, and dragging over rows
// ticks (or, from a ticked row, unticks) them; the drag takes back what the
// pointer leaves again.
await test('the viewer\'s Select list: Shift-click and dragging over rows tick several faces', await on('', (lib) => {
  lib.photos = [{ id: 1, version: 'orig0001' }];
  lib.photoFaces = { 1: [101, 102, 103, 104, 105].map((id) => ({ id, file: 1, kind: 'jpeg', version: 'orig0001', x: 0.1, y: 0.1, w: 0.1, h: 0.1, roll: 0, px: 100, score: 0.9, small: false, state: null, species: null, person: null, similarity: null, rejected: [], landmarks: [] })) };
}, async (p, lib) => {
  await p.locator('#sizer a.cell').first().click();
  await p.locator('#lb-info').click();
  await p.locator('#lb-panel .pface').first().waitFor();
  await p.locator(`#lb-panel .ptools button[data-tip^="${await tr(p, 'info.select')}"]`).click();
  const rows = p.locator('#lb-panel .pface.pickable');
  const ticked = () => p.locator('#lb-panel .pface.picked').evaluateAll((xs) => xs.map((x) => x.dataset.face).join());
  const boxed = () => p.locator('#lb-panel .pface input:checked').count();
  const middle = async (k) => { const b = await rows.nth(k).boundingBox(); return { x: b.x + b.width * 0.6, y: b.y + b.height / 2 }; };
  // A click, then Shift-click two rows further: the three, in either order.
  await rows.nth(0).click({ position: { x: 150, y: 10 } });
  check(await ticked() === '101', `after a click: ${await ticked()}`);
  await rows.nth(2).click({ modifiers: ['Shift'], position: { x: 150, y: 10 } });
  check(await ticked() === '101,102,103' && await boxed() === 3, `after Shift-click: ${await ticked()}`);
  await rows.nth(0).click({ position: { x: 150, y: 10 } });
  check(await ticked() === '102,103', `a click on a ticked row unticks it: ${await ticked()}`);
  // Shift-click on the box of a row works as on the row (from the last one clicked, the first).
  await rows.nth(4).locator('input').click({ modifiers: ['Shift'] });
  check(await ticked() === '101,102,103,104,105', `Shift-click on a box: ${await ticked()}`);
  await p.locator('#lb-panel .pickbar button.btn.quiet').click();
  await p.locator(`#lb-panel .ptools button[data-tip^="${await tr(p, 'info.select')}"]`).click();
  // Dragging over the first three rows, down and back up by one row: the pointer
  // leaves the third row again, so only two stay ticked, until it goes back.
  const a = await middle(0), c = await middle(2), b = await middle(1);
  await p.mouse.move(a.x, a.y);
  await p.mouse.down();
  await p.mouse.move(b.x, b.y, { steps: 4 });
  await p.mouse.move(c.x, c.y, { steps: 4 });
  check(await ticked() === '101,102,103', `while dragging over three rows: ${await ticked()}`);
  await p.mouse.move(b.x, b.y, { steps: 4 });
  check(await ticked() === '101,102', `the drag leaves the third: ${await ticked()}`);
  await p.mouse.move(c.x, c.y, { steps: 4 });
  await p.mouse.up();
  await p.clock.runFor(10);
  check(await ticked() === '101,102,103' && (await p.locator('#lb-panel .pickcount').textContent()).includes('3'), `after the drag: ${await ticked()}, ${await p.locator('#lb-panel .pickcount').textContent()}`);
  // The drag ends with no click on the row it ended on, and Shift-click goes on from it.
  await rows.nth(4).click({ modifiers: ['Shift'], position: { x: 150, y: 10 } });
  check(await ticked() === '101,102,103,104,105', `Shift-click after the drag: ${await ticked()}`);
  // A drag from a ticked row unticks what it covers.
  const d = await middle(3), e = await middle(4);
  await p.mouse.move(d.x, d.y);
  await p.mouse.down();
  await p.mouse.move(e.x, e.y, { steps: 4 });
  await p.mouse.up();
  await p.clock.runFor(10);
  check(await ticked() === '101,102,103', `a drag from a ticked row unticks: ${await ticked()}`);
}));

// A video the browser cannot decode (an old .avi): the viewer says so, once,
// and a photo next to it has no notice.
await test('a video the browser cannot play shows a notice, a photo does not', await on('', (lib) => {
  lib.photos = [{ id: 1, version: 'vid00001', kind: 'v' }, { id: 2, version: 'pho00002' }];
}, async (p, lib) => {
  const cells = p.locator('#sizer a.cell');
  await until('the grid', async () => (await cells.count()) === 2);
  await cells.first().click();
  const note = p.locator('#stage .lb-note');
  await until('the notice', async () => { await p.clock.runFor(100); return (await note.count()) === 1; });
  check((await note.textContent()) === await tr(p, 'lb.video_unplayable'), `notice: ${await note.textContent()}`);
  await p.keyboard.press('ArrowRight');
  await until('the photo', async () => (await p.locator('#stage img').count()) === 1);
  check((await note.count()) === 0, 'the photo still has the notice');
}));

await test('the Export button next to a tag chip is not offered while the list is empty', await on('tag=1', (lib) => {
  lib.photos = [1, 2].map((id) => ({ id, version: `orig000${id}`, hidden: 'tag' }));
  lib.tags = [{ id: 1, name: 'Strand', count: 2, kind: 'own' }];
}, async (p, lib) => {
  // Both photos are hidden, so the tag's list is empty: no button. The Hidden chip lists them, but has no button either.
  await settle(p, lib);
  check(await p.locator('#sizer a.cell').count() === 0, 'the tag list is empty');
  check(await p.locator('#filters .export-chip').count() === 1 && await p.locator('#filters .export-chip').isHidden(), 'the Export button is offered for an empty list');
  lib.photos[0].hidden = false;
  await p.evaluate(() => loadTimeline(false));
  await settle(p, lib);
  check(await p.locator('#filters .export-chip').isVisible(), 'the Export button is not offered once the list has a photo');
}));

await test('the Export button next to a tag chip is not offered while the Hidden chip is on', await on('tag=1', (lib) => {
  lib.photos = [1, 2].map((id) => ({ id, version: `orig000${id}` }));
  lib.tags = [{ id: 1, name: 'Strand', count: 2, kind: 'own' }];
}, async (p, lib) => {
  await cellCount(p, 2);
  check(await p.locator('#filters .export-chip').count() === 1, 'a tag chip alone has the Export button');
  await p.evaluate(() => setFilter(withFilter({ hidden: true })));
  await settle(p, lib);
  check(await p.locator('#filters .chip.hid').count() === 1, 'the Hidden chip is on');
  check(await p.locator('#filters .export-chip').count() === 0, 'the Export button is still offered with the Hidden chip');
}));

await test('common control sizes: the viewer\'s tools and the header\'s drop-downs share one height, and the person bar\'s words match Select all', await on('', hiddenPhotos, async (p, lib) => {
  await cellCount(p, 3);
  const box = (sel) => p.locator(sel).first().evaluate((e) => { const r = e.getBoundingClientRect(); return [Math.round(r.width), Math.round(r.height)]; });
  const types = await box('#types-btn'), lang = await box('#lang');
  check(types[1] === lang[1], `the Type button and the language drop-down are ${types[1]} and ${lang[1]} px high`);
  check((await p.locator('#lang').evaluate((e) => getComputedStyle(e).cursor)) === 'pointer', 'the language drop-down has no pointer cursor');
  // Select and the heart are framed icon buttons alike: one size, frame, radius and hover.
  const frame = (sel) => p.locator(sel).evaluate((e) => { const c = getComputedStyle(e), r = e.getBoundingClientRect(); return [Math.round(r.width), Math.round(r.height), c.borderRadius, c.borderTopWidth, c.borderTopColor, c.backgroundColor].join(' '); });
  const sel0 = await frame('#select'), fav0 = await frame('#fav-btn');
  check(sel0 === fav0, `Select (${sel0}) and the heart (${fav0}) are not framed alike`);
  const hoverBorder = async (sel) => { await p.locator(sel).hover(); return p.locator(sel).evaluate((e) => getComputedStyle(e).borderTopColor); };
  const selHover = await hoverBorder('#select'), favHover = await hoverBorder('#fav-btn');
  check(selHover === favHover && selHover !== sel0.split(' ').slice(4, 7).join(' '), `Select and the heart change their frame differently on hover: ${selHover}, ${favHover}`);
  // The small sizes: a small drop-down is as high as a small button, and the backup check's
  // "Show photos" keeps its label in the middle.
  const smalls = await p.evaluate(() => {
    const box = document.createElement('div');
    box.innerHTML = '<select class="small"><option>x</option></select><button class="btn small">x</button>'
      + '<div class="drive-card"><div class="lists"><details><summary><button class="btn quiet small go">x</button> 12 different</summary></details></div></div>';
    document.body.appendChild(box);
    const h = (q) => Math.round(box.querySelector(q).getBoundingClientRect().height);
    const go = getComputedStyle(box.querySelector('.go'));
    const out = [h('select'), h('.btn'), h('.go'), go.lineHeight, go.paddingTop, getComputedStyle(document.documentElement).getPropertyValue('--ctl-h-sm').trim()];
    box.remove();
    return out;
  });
  check(smalls[0] === 26 && smalls[1] === 26 && smalls[2] === 26 && smalls[3] === '26px' && smalls[4] === '0px' && smalls[5] === '26px', `small select, button and Show photos: ${smalls}`);
  check(!(await p.locator('#types-btn').textContent()).includes('▾'), 'the Type button has its own text arrow');
  const arrows = await p.evaluate(() => ['lang', 'types-btn'].map((id) => getComputedStyle(document.getElementById(id)).backgroundImage));
  check(arrows.every((x) => x.startsWith('url(') && x === arrows[0]), 'the selects and the Type button do not share one arrow');
  await p.locator('#sizer a.cell').first().click();
  await p.locator('#lightbox').waitFor({ state: 'visible' });
  await settle(p, lib);
  const sizes = [];
  for (const id of ['lb-fav', 'lb-rotate', 'lb-download', 'lb-info']) sizes.push(id + ':' + (await box('#' + id)).join('x'));
  check(new Set(sizes.map((x) => x.split(':')[1])).size === 1, `the viewer's tools differ in size: ${sizes}`);
  const hover = async (sel) => { await p.locator(sel).hover(); return p.locator(sel).evaluate((e) => getComputedStyle(e).backgroundColor); };
  check((await hover('#lb-info')) !== 'rgba(0, 0, 0, 0)', 'the info icon has no hover background');
  await p.locator('#lb-info').focus();
  await p.keyboard.press('Tab');
  await p.keyboard.press('Shift+Tab');
  check((await p.locator('#lb-info').evaluate((e) => getComputedStyle(e).outlineStyle)) === 'solid', 'the info icon has no focus ring');
}));

await test('common control sizes: on a person\'s suggested faces "Select all" and "Not x" are equally high and plain until hovered', await on('view=people', (lib) => lib.person('Nina Bar', { suggested: 3 }), async (p, lib, nina) => {
  await p.evaluate((id) => { location.hash = `view=person&id=${id}&tab=suggested`; }, nina.id);
  await p.locator('#pp-grid .face').nth(1).waitFor();
  await settle(p, lib);
  await p.locator('#pp-pick').click();
  const h = (sel) => p.locator(sel).first().evaluate((e) => e.getBoundingClientRect().height);
  const not = '#pp-bar .btn.quiet';
  check((await h('#pp-all')) === (await h(not)), `Select all is ${await h('#pp-all')} px high, "Not" ${await h(not)} px`);
  check((await p.locator(not).evaluate((e) => getComputedStyle(e).backgroundColor)) === 'rgba(0, 0, 0, 0)', '"Not" has a background before it is hovered');
}));

// The Confirmed tab's bar: Name, Ignore and Not a face are icon tools like
// Confirm and Undo (the icons of the Unnamed page's face actions), dimmed until
// a face is ticked; Name asks who it is, the others send the ticked face and it
// leaves the page.
await test('a person\'s Confirmed bar: Name, Ignore and Not a face are icon tools that act on the ticked face', await on('view=people', (lib) => ({ ida: lib.person('Ida Icon', { faces: 4 }), mia: lib.person('Mia Paw', { faces: 2, species: 'cat' }) }), async (p, lib, { ida, mia }) => {
  const open = async (who) => {
    await p.evaluate((id) => { location.hash = `view=person&id=${id}&tab=confirmed`; }, who.id);
    await p.locator(`#pp-grid .face[data-key="f${lib.faceLists[who.id].confirmed[1].id}"]`).waitFor();
    await settle(p, lib);
    await p.locator('#pp-pick').click();
  };
  const tool = (label) => p.locator(`#pp-bar .ib.pp-act[aria-label="${label}"]`);
  const name = await tr(p, 'person.name_btn'), ignore = await tr(p, 'unnamed.ignore'), notFace = await tr(p, 'facecheck.not_face'), notPet = await tr(p, 'pet.not_a.pet');
  // As the page writes them (an icon reads back in the browser's own spelling).
  const icons = await p.evaluate(() => {
    const as = (svg) => { const d = document.createElement('div'); d.innerHTML = svg; return d.innerHTML; };
    return { name: as(NAME_OTHER_ICON), ignore: as(IGNORE_ICON), face: as(NOT_FACE_ICON), pet: as(NOT_PET_ICON) };
  });
  await open(ida);
  const words = await p.locator('#pp-bar .btn').allTextContents();
  check(![name, ignore, notFace].some((w) => words.includes(w)), `Name, Ignore or Not a face is still a text button: ${words}`);
  check((await tool(name).innerHTML()) === icons.name, 'Name has not the person with a plus');
  check((await tool(ignore).innerHTML()) === icons.ignore, 'Ignore has not the eye with a slash');
  check((await tool(notFace).innerHTML()) === icons.face, 'Not a face has not the face with a slash');
  for (const label of [name, ignore, notFace]) check((await tool(label).getAttribute('aria-disabled')) === 'true', `${label} is not dimmed with nothing ticked`);
  await tool(ignore).click({ force: true });
  await settle(p, lib);
  check(!postedTo(lib, '/faces/ignore').length, 'Ignore sent something with nothing ticked');
  const act = async (label, path) => {
    const id = lib.faceLists[ida.id].confirmed[1].id;
    await p.locator(`#pp-grid .face[data-key="f${id}"] .crop`).click();
    check((await tool(label).getAttribute('aria-disabled')) === 'false', `${label} stays dimmed with a face ticked`);
    await tool(label).click();
    await posted(lib, new RegExp('^' + path + '$'));
    await settle(p, lib);
    check(postedTo(lib, path).length === 1, `${label}: ${postedTo(lib, path).length} requests to ${path}`);
    check(!lib.faceLists[ida.id].confirmed.some((f) => f.id === id), `${label} did not send the ticked face`);
    check(await p.locator(`#pp-grid .face[data-key="f${id}"]`).count() === 0, `${label}: the face stays on the page`);
  };
  const first = lib.faceLists[ida.id].confirmed[1].id;
  await p.locator(`#pp-grid .face[data-key="f${first}"] .crop`).click();
  await tool(name).click();
  await p.locator('#modal').waitFor();
  check((await p.locator('#modal-body p').first().textContent()) === await tr(p, 'person.who_one'), 'Name does not ask who the face is');
  await p.evaluate(() => closeModal());
  // Unticked again for the next action.
  await p.locator(`#pp-grid .face[data-key="f${first}"].sel .crop`).click();
  await act(ignore, '/faces/ignore');
  await act(notFace, '/faces/not-face');
  // A pet's page: Not a pet, with the paw.
  await open(mia);
  check(await tool(notFace).count() === 0, 'a pet\'s bar says Not a face');
  check((await tool(notPet).innerHTML()) === icons.pet, 'Not a pet has not the paw with a slash');
}));

await test('a text button never wraps: a long label is cut with an ellipsis and the quick label shows it whole', await on('view=people', (lib) => lib.person('Maximilian Alexander Friedrich Wilhelm von Hohenzollern-Sigmaringen', { suggested: 3 }), async (p, lib, long) => {
  await p.evaluate((id) => { location.hash = `view=person&id=${id}&tab=suggested`; }, long.id);
  await p.locator('#pp-grid .face').nth(1).waitFor();
  await settle(p, lib);
  await p.locator('#pp-pick').click();
  const not = p.locator('#pp-bar .btn.quiet');
  const m = await not.evaluate((e) => ({ h: e.getBoundingClientRect().height, cut: e.scrollWidth > e.clientWidth, text: e.textContent, over: getComputedStyle(e).textOverflow }));
  check(m.h === 32, `the long button is ${m.h} px high, expected one line of 32`);
  check(m.cut && m.over === 'ellipsis', 'the long label is not cut with an ellipsis');
  await not.hover();
  await p.clock.runFor(400);
  check((await p.locator('#quick-tip.show').textContent()) === m.text, 'the quick label does not show the whole label');
  await p.locator('#pp-all').hover();
  await p.clock.runFor(400);
  check(await p.locator('#quick-tip.show').count() === 0, 'a button that is not cut off shows a quick label');
}));

await test('the settings page uses the common controls: selects with the shared arrow, the folder field and its button 32 px high, chips with a round remove button', await on('view=settings', (lib) => { lib.copyFolders = ['Links', 'Gartensessions']; }, async (p, lib) => {
  await p.locator('.copy-folder-row input').first().waitFor();
  await settle(p, lib);
  const m = await p.evaluate(() => {
    const h = (e) => Math.round(e.getBoundingClientRect().height);
    const row = [...document.querySelectorAll('.copy-folder-row')].pop();
    const sel = document.querySelector('.pattern-choice select');
    return { input: h(row.querySelector('input')), add: h(row.querySelector('.btn')), select: h(sel), arrow: getComputedStyle(sel).backgroundImage.startsWith('url('), native: getComputedStyle(sel).appearance, x: getComputedStyle(row.previousElementSibling.querySelector('.chip-x')).borderRadius };
  });
  check(m.input === 32 && m.add === 32 && m.select === 32, `the folder field is ${m.input}, its button ${m.add}, a pattern select ${m.select} px high, expected 32`);
  check(m.arrow && m.native === 'none', 'the pattern selects have the native arrow');
  check(m.x === '50%', `the chip's remove button has the radius ${m.x}, expected a circle`);
}));

await test('the follow-up controls share the 32 px height: search box, face action buttons, dialog fields, kind toggle, viewer tools', await on('', hiddenPhotos, async (p, lib) => {
  await cellCount(p, 3);
  const heights = await p.evaluate(() => {
    const host = document.createElement('div');
    host.innerHTML = '<div class="dialog"><input type="text"><input type="number"><select><option>a</option></select><div class="kinds"><button>x</button><button class="on">y</button></div></div>'
      + '<button class="abtn">+</button><div class="lb-panel"><div class="ptools"><button class="tool">+</button></div></div>';
    document.body.appendChild(host);
    const h = (sel) => Math.round(host.querySelector(sel).getBoundingClientRect().height);
    const out = { search: Math.round(document.querySelector('.search input').getBoundingClientRect().height),
      text: h('.dialog input[type=text]'), number: h('.dialog input[type=number]'), select: h('.dialog select'), kind: h('.kinds button'), abtn: h('.abtn'), tool: h('.ptools .tool') };
    host.remove();
    return out;
  });
  for (const [name, v] of Object.entries(heights)) check(v === 32, `${name} is ${v} px high, expected 32`);
}));

await test('badges: the folder, tags, the + tag chip, face names and both small fields in the viewer\'s panel are 26 px high, and a maybe guess has no "Maybe"', await on('', (lib) => {
  lib.photos = [{ id: 1, version: 'orig0001', path: 'Familie/Aurelia mit einem sehr langen Ordnernamen, der nicht in die Zeile passt/IMG_1.JPG' }];
  lib.photoTags = { 1: [{ id: 1, name: 'Strand', source: 'user' }, { id: 2, name: 'Urlaub', source: 'folder' }] };
  const face = (id, extra) => ({ id, file: 1, kind: 'jpeg', version: 'orig0001', x: 0.1, y: 0.1, w: 0.1, h: 0.1, roll: 0, px: 100, score: 0.9, small: false, state: null, species: null, person: null, similarity: null, rejected: [], landmarks: [], ...extra });
  lib.photoFaces = { 1: [face(101, { state: 'maybe', person: { id: 9, name: 'Antonia Eger' } }), face(102)] };
}, async (p, lib) => {
  await p.locator('#sizer a.cell').first().click();
  await p.locator('#lb-info').click();
  await p.locator('#lb-panel .pface').first().waitFor();
  const h = (sel) => p.locator(sel).evaluateAll((xs) => xs.map((e) => Math.round(e.getBoundingClientRect().height)));
  const chips = await h('#lb-panel .tags .badge-chip');
  check(chips.length === 3 && chips.every((x) => x === 26), `tag chips are ${chips} px high, expected three of 26`);
  // The folder is a badge too, cut on one line inside the panel, its title the whole path.
  const folder = await p.locator('#lb-panel dd > button.badge-chip').evaluate((b) => {
    const r = b.getBoundingClientRect(), d = b.closest('.lb-panel').getBoundingClientRect(), t = b.querySelector('.t');
    return { h: Math.round(r.height), inside: r.right <= d.right + 0.5, cut: t.scrollWidth > t.clientWidth, title: b.title };
  });
  check(folder.h === 26 && folder.inside && folder.cut && folder.title.startsWith('Familie/Aurelia'), `the folder badge: ${JSON.stringify(folder)}`);
  const inner = await p.locator('#lb-panel .tags .own > button').evaluateAll((xs) => xs.map((e) => getComputedStyle(e).backgroundColor));
  check(inner.length === 2 && inner.every((x) => x === 'rgba(0, 0, 0, 0)'), `the buttons inside an own tag have a background of their own: ${inner}`);
  const guess = await p.locator('#lb-panel .pface .guess').textContent();
  check(guess === 'Antonia Eger?', `the guess reads "${guess}"`);
  const yn = await p.locator('#lb-panel .pface button.yes, #lb-panel .pface button.no').evaluateAll((xs) => xs.map((e) => { const c = getComputedStyle(e); return c.borderTopWidth + '|' + c.borderRadius + '|' + Math.round(e.getBoundingClientRect().height); }));
  check(yn.length === 2 && yn.every((x) => x === '0px|50%|22'), `the ✓ and ✗ in the panel are not 22 px circles without a border: ${yn}`);
  const other = await h('#lb-panel .pface button.add');
  check(other.length === 2 && other.every((x) => x === 26), `the face chips are ${other} px high`);
  await p.locator('#lb-panel .tags .add').click();
  check((await h('#lb-panel .tags input')).join() === '26', 'the tag field is not 26 px high');
  await p.locator('#lb-panel .tags input').fill('Str');
  await p.locator('#lb-panel .pface button.add').nth(1).focus();
  await p.clock.runFor(400);
  check(await p.locator('#lb-panel .tags input').count() === 0 && await p.locator('#lb-panel .tags .add').count() === 1, 'the tag field did not turn back into "+ Tag" when it lost the focus');
  await p.locator('#lb-panel .pface button.add').nth(1).click();
  check((await h('#lb-panel .pfield input')).join() === '26', 'the "Who is it?" field is not 26 px high: ' + await h('#lb-panel .pfield input'));
  await p.locator('#lb-panel .pfield input').fill('Ant');
  await p.locator('#lb-panel .tags .add').focus();
  await p.clock.runFor(400);
  check(await p.locator('#lb-panel .pfield').count() === 0 && await p.locator('#lb-panel .pface button.add').count() === 2, 'the field did not turn back into the badge when it lost the focus');
}));

// The faces in the viewer's panel: named people in a grid with their first
// name (the initial of the last name, or the whole name, where two share it)
// and the whole name as the quick label; suggestions, then unnamed faces in rows
// under "Needs a decision"; ignored faces only once "2 ignored" folds them out,
// for this photo. Tags and faces each have a heading of their own and a line.
await test('the viewer\'s faces: a grid of named people with short names, rows to decide, ignored ones folded away', await on('', (lib) => {
  lib.photos = [{ id: 1, version: 'orig0001' }, { id: 2, version: 'orig0002' }];
  lib.photoTags = { 1: [{ id: 1, name: 'Strand', source: 'user' }] };
  const named = ['Stefan Fritsch', 'Anna Müller', 'Anna Möller', 'Anna Schmidt', 'Jonas'].map((n) => lib.person(n, { faces: 1 }));
  const face = (id, file, extra) => ({ id, file, kind: 'jpeg', version: 'orig000' + file, x: 0.1, y: 0.1, w: 0.1, h: 0.1, roll: 0, px: 100, score: 0.9, small: false, state: null, species: null, person: null, similarity: null, rejected: [], landmarks: [], ...extra });
  const who = (k) => ({ state: 'confirmed', person: { id: named[k].id, name: named[k].name } });
  lib.photoFaces = {
    1: [face(101, 1), face(102, 1, who(0)), face(103, 1, { state: 'ignored' }), face(104, 1, who(1)), face(105, 1, { state: 'suggested', person: { id: named[4].id, name: 'Jonas' } }),
      face(106, 1, who(2)), face(107, 1, who(3)), face(108, 1, { state: 'ignored' })],
    2: [face(201, 2, who(0)), face(202, 2, { state: 'ignored' })],
  };
}, async (p, lib) => {
  await p.locator('#sizer a.cell').nth(0).click();
  await p.locator('#lb-info').click();
  await p.locator('#lb-panel .pcell').first().waitFor();
  const texts = (sel) => p.locator(sel).evaluateAll((xs) => xs.map((x) => x.textContent));
  const heads = await texts('#lb-panel .psec-head');
  check(JSON.stringify(heads) === JSON.stringify([await tr(p, 'info.tags'), await tr(p, 'info.people')]), `the headings: ${heads}`);
  const lines = await p.locator('#lb-panel .psec').evaluateAll((xs) => xs.map((x) => getComputedStyle(x).borderTopWidth));
  check(lines.join() === '1px,1px', `the lines above the sections: ${lines}`);
  check(await p.locator('#lb-panel dl .badge-chip, #lb-panel dl .pfaces').count() === 0, 'tags or faces are still rows of the details');
  const names = await texts('#lb-panel .pcell-name');
  check(names.join('|') === 'Stefan|Anna Müller|Anna Möller|Anna S.', `the grid: ${names.join('|')}`);
  const tips = await p.locator('#lb-panel .pcell .pshow').evaluateAll((xs) => xs.map((x) => x.dataset.tip));
  check(tips.join('|') === 'Stefan Fritsch|Anna Müller|Anna Möller|Anna Schmidt', `the quick labels: ${tips.join('|')}`);
  // Pointing at a named person shows their face's box (the index in the photo's faces).
  await p.locator('#lb-panel .pcell').nth(1).hover();
  check(await p.evaluate(() => lb.hover) === 3, `pointing at Anna Müller: ${await p.evaluate(() => lb.hover)}`);
  // The rows: the suggestion first, then the unnamed face; no ignored one.
  check(await p.locator('#lb-panel .psub').textContent() === await tr(p, 'info.to_decide'), 'no "Needs a decision"');
  const rows = () => p.locator('#lb-panel .pface').evaluateAll((xs) => xs.map((x) => (x.classList.contains('ignored') ? 'ignored' : x.querySelector('.guess') ? 'guess' : 'unnamed')).join());
  check(await rows() === 'guess,unnamed', `the rows: ${await rows()}`);
  check(await p.locator('#lb-panel .pface .guess + .yes + .no + .add').textContent() === await tr(p, 'info.other_person'), 'the suggestion has no Other chip');
  check(await p.locator(`#lb-panel .ptools button[data-tip^="${await tr(p, 'info.select')}"]`).count() === 1, 'no Select for two faces to decide');
  const fold = p.locator('#lb-panel .ptools .pignored');
  check(await fold.textContent() === await p.evaluate(() => trn('count.ignored', 2)), `the link: ${await fold.textContent()}`);
  await fold.click();
  check(await rows() === 'guess,unnamed,ignored,ignored', `folded out: ${await rows()}`);
  check(await fold.textContent() === await tr(p, 'info.hide_ignored'), `the link once folded out: ${await fold.textContent()}`);
  // Another photo has its ignored faces folded away; nothing is left to decide there.
  await p.keyboard.press('ArrowRight');
  await until('the next photo', async () => (await texts('#lb-panel .pcell-name')).join() === 'Stefan');
  check(await rows() === '' && await p.locator('#lb-panel .psub').count() === 0, `the second photo's rows: ${await rows()}`);
  check(await p.locator(`#lb-panel .ptools button[data-tip^="${await tr(p, 'info.select')}"]`).count() === 0, 'Select with nothing to decide');
  check(await fold.textContent() === await p.evaluate(() => trn('count.ignored', 1)), `the second photo's link: ${await fold.textContent()}`);
  // The ⋯ of a named person has the face's menu; a click on the person shows their photos.
  await p.locator('#lb-panel .pcell').first().hover();
  await p.locator('#lb-panel .pcell .more-btn').first().click();
  await p.getByRole('menuitem', { name: await tr(p, 'menu.use_as', { name: 'Stefan Fritsch' }), exact: true }).waitFor();
  await p.keyboard.press('Escape');
  await p.locator('#lb-panel .pcell .pshow').first().click();
  await until('the person\'s photos', () => p.evaluate(() => document.getElementById('lightbox').hidden && state.filter.people.length === 1));
}));

await test('pills come in two heights: chips and badges among text are 26 px, labels on a photo 18 px', await on('', hiddenPhotos, async (p, lib) => {
  await cellCount(p, 3);
  const heights = await p.evaluate(() => {
    const host = document.createElement('div');
    host.innerHTML = '<div class="filters"><span class="chip">Familie<button>✕</button></span><span class="chip more">+</span><span class="chip clear">x</span></div>'
      + '<span class="badge-chip">Aurelia</span><div class="copy" style="display:flex;flex-direction:column"><div class="badge-chip">Bearbeitet</div><div class="badge-chip worse">Kleiner</div></div>'
      + '<div class="cell" style="width:200px;height:200px"><span class="badge">RAW</span><span class="snip">Kartoffeln</span><span class="est"></span></div>';
    document.body.appendChild(host);
    // Not rounded: a label of 17.95 px is what the font happened to give, not the height.
    const h = (sel) => [...host.querySelectorAll(sel)].map((e) => +e.getBoundingClientRect().height.toFixed(2));
    const out = { text: [...h('.chip'), ...h('.badge-chip')], photo: [...h('.badge'), ...h('.snip'), ...h('.est')] };
    host.remove();
    return out;
  });
  check(heights.text.length === 6 && heights.text.every((x) => x === 26), `chips and badges among text are ${heights.text} px high, expected 26`);
  check(heights.photo.length === 3 && heights.photo.every((x) => x === 18), `labels on a photo are ${heights.photo} px high, expected 18`);
}));

await test('the search box is a text field like the settings\' fields: same look, a border on hover, the accent ring on focus', await on('view=settings', (lib) => { lib.copyFolders = ['Links']; }, async (p, lib) => {
  await p.locator('.copy-folder-row input').first().waitFor();
  await settle(p, lib);
  const look = (sel) => p.locator(sel).first().evaluate((e) => { const c = getComputedStyle(e); return [Math.round(e.getBoundingClientRect().height), c.borderTopColor, c.borderRadius, c.backgroundColor, c.outlineStyle, c.outlineColor, c.outlineOffset].join('|'); });
  const field = '.copy-folder-row input', search = '#search';
  check((await look(search)) === (await look(field)), `at rest the search box ${await look(search)} differs from a settings field ${await look(field)}`);
  const rest = await look(search);
  await p.locator(field).first().hover();
  const fieldHover = await look(field);
  await p.locator(search).hover();
  const searchHover = await look(search);
  check(searchHover === fieldHover && searchHover !== rest, `on hover the search box ${searchHover} differs from a settings field ${fieldHover} (at rest ${rest})`);
  await p.locator(field).first().focus();
  const fieldFocus = await look(field);
  await p.locator(search).focus();
  await p.mouse.move(0, 0);
  const searchFocus = await look(search);
  const accent = await p.evaluate(() => { const e = document.createElement('i'); e.style.color = 'var(--accent)'; document.body.appendChild(e); const c = getComputedStyle(e).color; e.remove(); return c; });
  check(searchFocus === fieldFocus && searchFocus.includes('|solid|' + accent + '|2px'), `on focus the search box ${searchFocus} differs from a settings field ${fieldFocus}, expected the ring in ${accent}`);
}));

await test('tabs look the same everywhere: a person\'s tabs and the drives\' tabs share one style', await on('view=people', (lib) => lib.person('Tabea Tab', { suggested: 2 }), async (p, lib, tabea) => {
  await p.evaluate((id) => { location.hash = `view=person&id=${id}&tab=suggested`; }, tabea.id);
  await p.locator('#person-tabs .tab.active').waitFor();
  const style = (sel) => p.evaluate((q) => { const e = document.querySelector(q); const c = getComputedStyle(e); return [c.textTransform, c.paddingTop, c.paddingLeft, c.borderBottomWidth, c.fontWeight, c.letterSpacing].join('|'); }, sel);
  await p.evaluate(() => { const d = document.createElement('div'); d.className = 'tabs underline'; d.innerHTML = '<button class="on">x</button><button>y</button>'; d.id = 'probe-tabs'; document.body.appendChild(d); });
  check((await style('#person-tabs .tab.active')) === (await style('#probe-tabs .on')), `the active tabs differ: ${await style('#person-tabs .tab.active')} vs ${await style('#probe-tabs .on')}`);
  check((await style('#person-tabs .tab:not(.active)')) === (await style('#probe-tabs button:not(.on)')), 'the other tabs differ');
  check((await style('#person-tabs .tab.active')).startsWith('uppercase'), 'the tabs are not in the drives\' style');
}));

await test('one badge style: the labels on photos in the timeline, the duplicates cards and the map share look and size', await on('', hiddenPhotos, async (p, lib) => {
  await cellCount(p, 3);
  const look = await p.evaluate(() => {
    const host = document.createElement('div');
    host.innerHTML = '<div class="cell"><span class="badge">RAW</span></div><div class="card"><div class="thumb"><div class="badges"><span class="badge">JPG</span></div></div></div><div class="geo-cell"><span class="badge small">V</span></div>';
    document.body.appendChild(host);
    const of = (sel) => { const c = getComputedStyle(host.querySelector(sel)); return [c.backgroundColor, c.color, c.borderRadius, c.fontWeight, c.fontSize, c.paddingLeft, c.lineHeight].join('|'); };
    const out = { cell: of('.cell .badge'), card: of('.card .badge'), geo: of('.geo-cell .badge') };
    host.remove();
    return out;
  });
  check(look.cell === look.card, `the timeline's and the duplicates' badges differ: ${look.cell} vs ${look.card}`);
  const [bg, fg, radius, weight] = look.cell.split('|'), g = look.geo.split('|');
  check(g[0] === bg && g[1] === fg && g[2] === radius && g[3] === weight && g[4] === '10px', `the map's badge is not the small one: ${look.geo}`);
}));

await test('a caret in the sidebar goes from muted to full strength on hover, with no background', await on('', hiddenPhotos, async (p, lib) => {
  await cellCount(p, 3);
  await p.evaluate(() => {
    const host = document.createElement('div');
    host.innerHTML = '<div class="tree"><div class="row"><button class="toggle" id="c1">▸</button></div></div>';
    host.style.cssText = 'position:fixed;left:300px;top:300px;z-index:99';
    document.body.appendChild(host);
  });
  const text = await p.evaluate(() => getComputedStyle(document.body).color);
  for (const id of ['c1']) {
    const rest = await p.locator('#' + id).evaluate((e) => getComputedStyle(e).color);
    check(rest !== text, `${id} is not muted at rest`);
    await p.locator('#' + id).hover();
    const on = await p.locator('#' + id).evaluate((e) => { const c = getComputedStyle(e); return [c.color, c.backgroundColor, c.outlineStyle].join('|'); });
    check(on === `${text}|rgba(0, 0, 0, 0)|none`, `${id} on hover: ${on}, expected ${text} without background or outline`);
  }
}));

await test('the page starts only once every script is loaded: a late export.js does not break the first paint', await on('tag=1', (lib) => {
  lib.photos = [1, 2].map((id) => ({ id, version: `orig000${id}` }));
  lib.tags = [{ id: 1, name: 'Strand', count: 2, kind: 'own' }];
  lib.lateScripts = { '/export.js': 700 };
}, async (p, lib) => {
  await cellCount(p, 2);
  check(await p.locator('#filters .export-chip').count() === 1, 'the Export button next to the tag chip is missing');
}));

// ---- the year strip right of the grid

// Photos newest first: `n(y)` in each of the months `months(y)` of every year from `to` back to `from`.
function years(lib, from, to, n, months = () => [12, 9, 6, 3]) {
  let id = 0;
  lib.photos = [];
  for (let y = to; y >= from; y--) {
    for (const m of months(y)) {
      for (let i = 0; i < n(y); i++) { id++; lib.photos.push({ id, version: String(id).padStart(8, '0'), day: y * 10000 + m * 100 + 1 }); }
    }
  }
}
// Busy years since 2015 (four months of 25 photos), one photo a year before, back to 1990.
const longTimeline = (lib) => years(lib, 1990, 2026, (y) => (y >= 2015 ? 25 : 1), (y) => (y >= 2015 ? [12, 9, 6, 3] : [7]));
const ALL_MONTHS = () => [12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1];

// The strip as it is drawn: its labels (text, top in the track, marked as current), the marker, the bubble.
const strip = (p) => p.evaluate(() => {
  const box = $('scrub'), q = (s) => box.querySelector(s), tip = q('.scrub-tip');
  return {
    hidden: box.hidden, bar: $('scroller').classList.contains('has-scrub'), h: q('.scrub-track').getBoundingClientRect().height,
    labels: [...box.querySelectorAll('.scrub-lbl')].map((e) => ({ text: e.textContent, top: parseFloat(e.style.top), cur: e.classList.contains('cur') })),
    now: parseFloat(q('.scrub-now').style.top), tip: getComputedStyle(tip).display === 'none' ? null : tip.textContent,
    value: box.getAttribute('aria-valuetext'), valueNow: box.getAttribute('aria-valuenow'),
  };
});
const shown = (p) => until('the year strip', async () => !(await strip(p)).hidden && (await strip(p)).labels.length);
// The month at the top of the timeline (the top bar names it), once the page has drawn the scroll.
const topMonth = async (p) => { await p.clock.runFor(50); return p.evaluate(() => $('current-month').textContent); };
const label = (p, text) => p.locator('#scrub .scrub-lbl', { hasText: new RegExp(`^${text}$`) });
const trackPoint = async (p, at) => {
  const r = await p.locator('#scrub .scrub-track').boundingBox();
  return { x: r.x + r.width / 2, y: r.y + r.height * at, top: r.y };
};

await test('the year strip: every year where it starts, the busy ones with more room, at least 10 px each, no label closer than 14 px to the one above', await on('', longTimeline, async (p) => {
  await shown(p);
  const s = await strip(p);
  check(s.bar, 'the scroller still shows its scroll bar next to the strip');
  check(!(await p.locator('#years').count()), 'the Year drop-down is still in the header');
  const ys = s.labels.map((l) => Number(l.text));
  check(ys[0] === 2026 && ys.every((y, i) => !i || y < ys[i - 1]), `the labels are not the years newest first: ${ys}`);
  for (let y = 2026; y >= 2015; y--) check(ys.includes(y), `the busy year ${y} has no label: ${ys}`);
  check(ys.length < 37, 'every one of the quiet years has a label, though they lie closer than a label is high');
  s.labels.forEach((l, i) => {
    if (!i) return;
    const gap = l.top - s.labels[i - 1].top, between = ys[i - 1] - ys[i];
    check(gap >= 14, `${l.text} is ${gap} px below ${s.labels[i - 1].text}`);
    check(gap >= 10 * between - 0.5, `${between} years between ${s.labels[i - 1].text} and ${l.text} take ${gap} px, less than 10 px each`);
  });
  const at = (y) => s.labels.find((l) => l.text === String(y)).top;
  check(at(2019) - at(2020) > 4 * (at(2008) - at(2010)) / 2, 'a busy year has no more room than a year of one photo');
  check(s.h - s.labels[s.labels.length - 1].top < 14, 'the oldest year is not at the end of the strip');
  // 50 px wide: the labels 12 px from the edge and 30 px wide, 8 px from the photos.
  const box = await p.locator('#scrub').boundingBox(), lbl = await label(p, 2020).evaluate((e) => { const r = e.getBoundingClientRect(); return [r.left, r.right]; });
  check(box.width === 50 && Math.round(box.x + box.width - lbl[1]) === 12 && Math.round(lbl[0] - box.x) === 8, `strip ${box.width} px wide, label from ${lbl[0] - box.x} to ${box.x + box.width - lbl[1]} px of its edges`);
}));

await test('the year strip: a click on a year puts its first month at the top; the marker and the current year follow the scroll, to the end of the strip at the end of the list', await on('', longTimeline, async (p) => {
  await shown(p);
  // The upper half of the label lies above the line where 2020 starts: it is still 2020.
  await label(p, 2020).click({ position: { x: 15, y: 2 } });
  check(await topMonth(p) === await p.evaluate(() => monthLabel(202012)), 'a click on 2020 did not bring December 2020 to the top: ' + await topMonth(p));
  let s = await strip(p);
  const l2020 = s.labels.find((l) => l.text === '2020'), l2019 = s.labels.find((l) => l.text === '2019');
  check(Math.abs(s.now - l2020.top) < 1, `the marker is at ${s.now}, 2020 at ${l2020.top}`);
  check(s.labels.filter((l) => l.cur).map((l) => l.text).join() === '2020', 'the current year is not 2020 alone');
  check(s.value === await topMonth(p), `the slider says ${s.value}`);
  // The wheel over the photos: the marker moves on within the year, no bubble.
  await p.mouse.move(600, 700);
  await p.mouse.wheel(0, 900);
  await until('the wheel to scroll', async () => { await p.clock.runFor(50); return (await strip(p)).now > l2020.top; });
  s = await strip(p);
  check(s.now > l2020.top && s.now < l2019.top && s.tip === null, `after the wheel the marker is at ${s.now} (2020 at ${l2020.top}, 2019 at ${l2019.top}), bubble ${s.tip}`);
  // The scroll event comes with the browser's next frame, then the page's (fake) one draws.
  const scrolledTo = async (top) => {
    const at = await p.evaluate((t) => { $('scroller').scrollTop = t; return $('scroller').scrollTop; }, top);
    const want = await p.evaluate((t) => headAt(t).label, at);
    await until(`the top bar to name ${want}`, async () => { await p.clock.runFor(50); return (await p.evaluate(() => $('current-month').textContent)) === want; });
    return strip(p);
  };
  s = await scrolledTo(1e9);
  check(Math.abs(s.now - s.h) < 1, `at the end of the list the marker is at ${s.now} of ${s.h}`);
  s = await scrolledTo(0);
  check(s.now === 0 && s.labels[0].cur, `at the top the marker is at ${s.now}`);
}));

await test('the year strip: pointing names the month there and the jump goes to that month; dragging scrolls along; the wheel over the strip scrolls the photos', await on('', longTimeline, async (p) => {
  await shown(p);
  const a = await trackPoint(p, 0.4);
  await p.mouse.move(a.x, a.y);
  let s = await strip(p);
  check(s.tip && /\d{4}$/.test(s.tip), `pointing at the strip shows no month: ${s.tip}`);
  const widths = await p.evaluate(() => ['.scrub-now', '.scrub-hov'].map((c) => document.querySelector('#scrub ' + c).getBoundingClientRect().width));
  check(Math.abs(widths[0] * 2 - widths[1]) < 1, `the marker is ${widths[0]} px, the line under the pointer ${widths[1]} px: not half`);
  const named = s.tip;
  await p.mouse.down();
  check(await topMonth(p) === named, `the bubble named ${named}, the jump went to ${await topMonth(p)}`);
  const before = await p.evaluate(() => $('scroller').scrollTop);
  const b = await trackPoint(p, 0.7);
  await p.mouse.move(b.x, b.y, { steps: 4 });
  s = await strip(p);
  check(await p.evaluate(() => $('scroller').scrollTop) > before, 'dragging down did not scroll down');
  check(Math.abs(s.now - (b.y - b.top)) < 1, `while dragging the marker is at ${s.now}, the pointer at ${b.y - b.top}`);
  await p.mouse.up();
  check(!(await p.locator('#scrub.drag').count()), 'the strip still drags after the button is up');
  const y0 = await p.evaluate(() => $('scroller').scrollTop);
  await p.mouse.wheel(0, 500);
  await until('the wheel over the strip to scroll', async () => (await p.evaluate(() => $('scroller').scrollTop)) === y0 + 500);
  await p.mouse.move(600, 700);
  check((await strip(p)).tip === null, 'the bubble stays when the pointer leaves the strip');
}));

await test('the year strip by keyboard: ↓ ↑ a month, Page Down / Page Up a year, Home and End the ends', await on('', longTimeline, async (p) => {
  await shown(p);
  const month = (m) => p.evaluate((x) => monthLabel(x), m);
  await p.locator('#scrub').focus();
  const steps = [['ArrowDown', 202609], ['ArrowDown', 202606], ['ArrowUp', 202609], ['PageDown', 202512], ['PageDown', 202412], ['PageUp', 202512], ['Home', 202612]];
  for (const [key, m] of steps) {
    await p.keyboard.press(key);
    const want = await month(m);
    check(await topMonth(p) === want, `${key}: ${await topMonth(p)} at the top, expected ${want}`);
    check((await strip(p)).value === want, `${key}: the slider says ${(await strip(p)).value}`);
  }
  check((await strip(p)).valueNow === '0', 'the first month is not the slider\'s 0');
  await p.keyboard.press('End');
  await p.clock.runFor(50);
  check(await p.evaluate(() => { const s = $('scroller'); return s.scrollTop + s.clientHeight >= s.scrollHeight - 1; }), 'End is not the end of the list');
}));

await test('the year strip of three years or fewer shows months, three letters (Safari\'s März and Sept. too), each year at its first month; four years show years again', await on('', (lib) => years(lib, 2024, 2026, () => 6, ALL_MONTHS), async (p, lib) => {
  await shown(p);
  let s = await strip(p);
  const texts = s.labels.map((l) => l.text);
  check(texts[0] === '2026' && texts.includes('2025') && texts.includes('2024'), `no year at the start of each year: ${texts}`);
  check(texts.indexOf('2025') === texts.indexOf(await p.evaluate(() => I18n.date(new Date(2026, 0, 1), { month: 'short' }).slice(0, 3))) + 1, `2025 does not follow January 2026: ${texts}`);
  check(texts.every((t) => /^\d{4}$/.test(t) || /^\p{L}{3}$/u.test(t)), `a label is neither a year nor three letters: ${texts}`);
  check(s.labels.every((l, i) => !i || l.top - s.labels[i - 1].top >= 14), 'two month labels are closer than 14 px');
  // A fourth year: the same page, a new list, years again at once.
  years(lib, 2023, 2026, () => 6, ALL_MONTHS);
  await p.evaluate(() => loadTimeline(false));
  await settle(p, lib);
  s = await strip(p);
  check(s.labels.map((l) => l.text).join() === '2026,2025,2024,2023', 'four years: ' + s.labels.map((l) => l.text));
  // Safari's German short months (Chromium's are three letters already): März, Juni, Sept. become Mär, Jun, Sep.
  years(lib, 2025, 2026, () => 6, ALL_MONTHS);
  await p.evaluate(() => {
    const date = I18n.date, safari = ['Jan.', 'Feb.', 'März', 'Apr.', 'Mai', 'Juni', 'Juli', 'Aug.', 'Sept.', 'Okt.', 'Nov.', 'Dez.'];
    I18n.date = (d, opts) => (opts && opts.month === 'short' && !opts.year ? safari[d.getMonth()] : date(d, opts));
    loadTimeline(false);
  });
  await settle(p, lib);
  const de = (await strip(p)).labels.map((l) => l.text);
  check(['Mär', 'Jun', 'Jul', 'Sep', 'Okt'].every((m) => de.includes(m)) && de.every((t) => t.length <= 4), 'Safari\'s German months: ' + de);
}));

await test('the year strip is only there for a timeline that scrolls: not for a short list, not on other pages, back with the timeline', await on('', hiddenPhotos, async (p, lib) => {
  await cellCount(p, 3);
  let s = await strip(p);
  check(s.hidden && !s.bar, 'three photos have a strip, or no scroll bar');
  longTimeline(lib);
  await p.evaluate(() => loadTimeline(false));
  await settle(p, lib);
  await shown(p);
  await p.evaluate(() => showView('duplicates'));
  await settle(p, lib);
  s = await strip(p);
  check(s.hidden && !s.bar, 'the Duplicates page has the strip');
  // A window that changes size lays the timeline out again behind the page: still no strip.
  await p.setViewportSize({ width: 1200, height: 1400 });
  await settle(p, lib);
  check((await strip(p)).hidden, 'the Duplicates page has the strip after the window changed size');
  await p.evaluate(() => setFilter({}));
  await settle(p, lib);
  check(!(await strip(p)).hidden, 'the strip did not come back with the timeline');
}));

await finish();
