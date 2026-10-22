// The Unnamed page, groups, the people list and the trash page against a fake API
// (fake-api.mjs) on a fake clock: which requests the page makes after each
// action and when, and what the sidebar shows from the replies (PRs #142 and
// #143). No shoebox is needed: the page is core/web as it is on disk.
//
//   node core/tests/browser/faces-mocked.mjs     (ONLY=text: only those tests)
//
// Time only moves when a test moves it (`settle`), so "nothing is asked for
// until the grouping is done" is checked over minutes of the page's time in a
// few milliseconds, and the steps of a grouping come one at a time. The real
// server tells the page of each step by its event stream; here the test does,
// by calling the page's own `loadInfo` (`tell`), as the stream's event does.
import { browser, check, until, tr, test, finish, requests, noListRequests, clusterRounds, sidebarIs, groupNames, menu, typeName, face, card, single, nameCard } from './faces-common.mjs';
import { fakeLibrary, ORIGIN } from './fake-api.mjs';

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
  const want = await words(['people.menu.show', ...edit]);
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

await test('a new group with a person', await on('view=people', (lib) => lib.person('Cleo New'), async (p, lib, cleo) => {
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

await finish();
