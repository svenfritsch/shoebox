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
// (150 ms), and for the photos after 300 ms. Both wait for the drive's one
// connection, so what was asked for an earlier text is called off when a newer
// one is asked for (see "Search" under Performance in docs/plan.md).
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
  const badges = await p.locator('.dup-row').first().locator('.badge.worse').allTextContents();
  check(JSON.stringify(badges) === JSON.stringify([await tr(p, 'dups.smaller_file'), await tr(p, 'dups.lower_quality')]), `badges ${badges}`);
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

await test('hiding in the viewer (key h): the photo stays until the viewer closes, then the grid loses it; the Hidden row and chip show it', await on('', hiddenPhotos, async (p, lib) => {
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
  check(await p.locator('#hidden-row .count').textContent() === '1', 'the Hidden row counts the hidden photo');
  // The Hidden row: only the hidden photos, as a chip.
  await p.locator('#hidden-row .name').click();
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
  await p.locator('#hidden-row .name').click();
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
  await p.evaluate(() => { $('years').hidden = false; if (!$('years').options.length) $('years').add(new Option('2020', '2020')); });
  const years = await box('#years'), lang = await box('#lang');
  check(years[1] === lang[1], `the year and language drop-downs are ${years[1]} and ${lang[1]} px high`);
  check((await p.locator('#years').evaluate((e) => getComputedStyle(e).cursor)) === 'pointer', 'the year drop-down has no pointer cursor');
  check((await p.locator('#select').evaluate((e) => getComputedStyle(e).borderRadius)) === '50%', 'the select icon is not a circle');
  check(!(await p.locator('#types-btn').textContent()).includes('▾'), 'the Type button has its own text arrow');
  const arrows = await p.evaluate(() => ['years', 'lang', 'types-btn'].map((id) => getComputedStyle(document.getElementById(id)).backgroundImage));
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

await finish();
