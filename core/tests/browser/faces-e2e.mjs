// The Unnamed page and the people list against the real server: a library of
// make-faces.py, faces from the fake recognizer, served by `shoebox serve`
// (run.sh). A few tests only, for what a fake API cannot show: that the
// sidebar built from the server's replies is what the server then lists, that
// the server's grouping after a name ends in one refresh of the page, a name
// given in the viewer (which asks for /files), "Use as picture" in its three
// places, a group the server removes when its last person leaves, and the
// search box's one request and its list. The rest is in faces-mocked.mjs (see "Browser tests" in docs/plan.md).
//
//   node core/tests/browser/faces-e2e.mjs <url>     (ONLY=text: only those tests)
//
// Every test works on folders and names no other test uses. Waits are on the
// server (the grouping is done), the page's state and its requests; the one
// time window (`nothingFor`) is for checking that something does not happen.
// While the page's answers to /info are held back (`holdInfo`) it cannot know
// that a grouping is done, so what it asks for then comes from the action.
import { browser, check, until, tr, test, finish, requests, noListRequests, clusterRounds, sidebarIs, groupNames, menu, typeName, card, nameCard } from './faces-common.mjs';

const url = process.argv[2];
const T = 60000;
const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 } });
await ctx.addInitScript(() => { try { localStorage.setItem('shoebox.lang', 'en'); } catch (e) {} });
ctx.setDefaultTimeout(T);
// APP_JS=<file>: the page runs this app.js instead of the one built into shoebox.
if (process.env.APP_JS) await ctx.route((u) => new URL(u).pathname === '/app.js', (r) => r.fulfill({ path: process.env.APP_JS, contentType: 'text/javascript' }));

const LIB = '/api/lib/' + (await (await fetch(new URL('/api/libraries', url))).json())[0].id;

async function api(path, body) {
  const r = await fetch(new URL(LIB + path, url), body === undefined ? {} : {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-Shoebox': '1' },
    body: JSON.stringify(body),
  });
  if (!r.ok) throw new Error(`${path}: HTTP ${r.status} ${await r.text()}`);
  return r.json();
}

// The server's clusters once nothing is being grouped any more.
const grouped = () => until('the grouping to finish', async () => {
  const info = await api('/info');
  const c = info.clusters;
  return !info.busy && !c.pending && !c.stale && !c.running && !c.embedding && c;
});

// The cluster of `folder` (each folder of make-faces.py is one), its faces as
// the page shows them first.
async function clusterOf(folder) {
  for (const c of (await api('/clusters?limit=500&samples=8')).clusters) {
    if ((await api(`/files/${c.faces[0].file}`)).path.split('/')[0] === folder) return c;
  }
  throw new Error(`no cluster of ${folder}`);
}

// Setting up through the API: the faces of `folder` are named `name`.
async function named(folder, name) {
  const c = await clusterOf(folder);
  const r = await api(`/clusters/${c.id}/name`, { name, generation: c.generation });
  await grouped();
  return personNamed(r.person.name);
}

async function personNamed(name) {
  const p = (await api('/people')).find((x) => x.name === name);
  check(p, `nobody named ${name}`);
  return p;
}

const server = async () => ({ groups: await api('/groups'), people: await api('/people'), clusters: (await api('/info')).clusters.clusters });

// ---- the page's requests

// Its requests to the library's API (not pictures, not the event stream) in
// order, `{ method, path, query }` relative to the library.
function track(p) {
  const t = { log: [], open: new Map(), last: Date.now() };
  const ours = (u) => {
    const { pathname, searchParams } = new URL(u);
    if (!pathname.startsWith(LIB + '/')) return null;
    const path = pathname.slice(LIB.length);
    if (path === '/events' || /\/(crop|thumb|view|original)$/.test(path)) return null;
    return { path, query: searchParams };
  };
  p.on('request', (r) => {
    const o = ours(r.url());
    if (!o) return;
    t.log.push({ ...o, method: r.method() });
    t.open.set(r, o.path);
    t.last = Date.now();
  });
  const done = (r) => { if (t.open.delete(r)) t.last = Date.now(); };
  p.on('requestfinished', done);
  p.on('requestfailed', done);
  return t;
}

// No request open (but those to `except`, held by `holdInfo`) and none started for `ms`.
const quiet = (t, ms = 500, except = null) => until('the page to stop asking the server', () => ![...t.open.values()].some((path) => path !== except) && Date.now() - t.last >= ms);

// A check that something does not happen: the page is left alone this long.
const nothingFor = (p) => p.waitForTimeout(700);

// The server is done grouping, the page has heard so (its status says what the
// server's does), and asks nothing more.
async function settled(p, t) {
  const c = await grouped();
  const fields = ['finished_at', 'clusters', 'unnamed', 'people', 'suggested', 'stale', 'running', 'embedding'];
  await p.waitForFunction(([want, fields]) => {
    const c = window.people && people.info && people.info.clusters;
    return c && fields.map((k) => c[k]).join() === want;
  }, [fields.map((k) => c[k]).join(), fields], { timeout: T });
  await quiet(t);
}

// The page's answers to /info wait until `release()`.
async function holdInfo(p) {
  let open;
  const gate = new Promise((r) => { open = r; });
  const pattern = (u) => new URL(u).pathname === LIB + '/info';
  const handler = async (route) => { await gate; await route.continue().catch(() => {}); };
  await p.route(pattern, handler);
  return async () => { open(); await p.unroute(pattern, handler); };
}

// The reply to the next POST whose path (relative to the library) matches.
const posted = (p, re) => p.waitForResponse((r) => r.request().method() === 'POST' && re.test(new URL(r.url()).pathname.slice(LIB.length)), { timeout: T })
  .then(async (r) => { check(r.ok(), `POST ${r.url()}: HTTP ${r.status()} ${await r.text()}`); return r.json(); });

// `fn(p, t, s)` on a new page at `hash` (or `hash(s)`), settled, with `t` its
// request log and `s` what `setup()` (through the API, first) returned.
const on = (hash, setup, fn) => async (keep) => {
  await grouped();
  const s = setup ? await setup() : undefined;
  const p = keep(await ctx.newPage());
  const errors = [];
  p.on('pageerror', (e) => errors.push(e.message));
  const t = track(p);
  await p.goto(new URL('#' + (typeof hash === 'function' ? hash(s) : hash), url).href);
  await p.waitForFunction(() => window.people && people.info && !peopleFlight, null, { timeout: T });
  await settled(p, t);
  await fn(p, t, s);
  check(!errors.length, 'error in the page: ' + errors.join('; '));
};

// `act` changes something and returns the reply. While /info is held: no
// people list or groups asked for, and the sidebar is what the server lists.
// After the grouping that follows, only the People page asks for the list
// (once at most); the sidebar is still right.
async function fromReply(p, t, why, act) {
  const from = t.log.length;
  const release = await holdInfo(p);
  let reply;
  try {
    reply = await act();
    await quiet(t, 500, '/info');
    noListRequests(t.log, from, why);
    await sidebarIs(p, { ...(await server()), clusters: null }, why);
  } finally {
    await release();
  }
  const after = t.log.length;
  await settled(p, t);
  const n = requests(t.log, after, '/people').length, g = requests(t.log, after, '/groups').length;
  if (await p.evaluate(() => state.filter.view === 'people')) check(n <= 1 && g <= 1, `${why}: after the grouping the People page asked for the list ${n} times, the groups ${g} times`);
  else noListRequests(t.log, after, `${why}, after the grouping`);
  await sidebarIs(p, await server(), `${why}, after the grouping`);
  return reply;
}

// Opens the menu of `button` (a ⋯ button, or with `{ button: 'right' }` a
// photo). The page closes a menu when it scrolls, so the button is scrolled to
// first and the scroll events are let through before the click.
async function openMenu(p, button, opts) {
  await button.scrollIntoViewIfNeeded();
  await p.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  await button.click(opts);
  await p.locator('.menu').waitFor();
}

// ---- tests

await test('a card named on the real server: no list request, the sidebar is what the server lists, one refresh after its grouping', on('view=unnamed', null, async (p, t) => {
  const c = await clusterOf('c01');
  const from = t.log.length;
  const release = await holdInfo(p);
  let reply;
  try {
    const answer = posted(p, /^\/clusters\/\d+\/name$/);
    await nameCard(p, c.faces[0].id, 'Nora Server');
    reply = await answer;
    await nothingFor(p);
    check(!requests(t.log, from, '/clusters').length, 'the clusters were asked for at once, before the grouping was done');
    await quiet(t, 300, '/info');
    noListRequests(t.log, from, 'a name on the Unnamed page');
    await sidebarIs(p, { ...(await server()), clusters: null }, 'from the reply, before the grouping');
  } finally {
    await release();
  }
  await settled(p, t);
  const rounds = clusterRounds(t.log, from);
  check(rounds === 'large, small', `after the grouping the clusters were asked for as [${rounds}], expected one refresh (large, small)`);
  noListRequests(t.log, from, 'a name on the Unnamed page');
  check(reply.person_data && reply.person_data.name === 'Nora Server', 'the reply carries the person (person_data)');
  await card(p, c.faces[0].id).waitFor({ state: 'detached' });
  await sidebarIs(p, await server(), 'after the grouping');
}));

// run.sh serves with SHOEBOX_REGROUP=2,1500,300: two cards or 1.5 s.
await test('the next grouping on the real server: the line after a name, then finding, gone once grouped; no button', on('view=unnamed', null, async (p, t) => {
  const c = await clusterOf('c07');
  await nameCard(p, c.faces[0].id, 'Lina Line');
  const text = await until('the line', () => p.evaluate(() => { const l = document.getElementById('un-next'); return l && !l.hidden && l.querySelector('.text').textContent; }));
  const expected = await p.evaluate(() => [0, 1, 2].map((s) => trn('unnamed.next', 1, { s })));
  check(expected.includes(text) || text === await tr(p, 'unnamed.regrouping'), `after the name the line says "${text}"`);
  check(!(await p.locator('#un-next button').count()), 'the line has no button');
  await grouped();
  await until('the line to go', () => p.evaluate(() => document.getElementById('un-next').hidden));
  await card(p, c.faces[0].id).waitFor({ state: 'detached' });
}));

// The viewer has no card that shows the change: the page asks for its
// clusters at once, while it cannot know that the grouping is done.
await test('a name given in the viewer over the Unnamed page refreshes the page at once', on('view=unnamed', null, async (p, t) => {
  const c = await clusterOf('c05');
  const id = c.faces[0].id;
  await card(p, id).locator(`img[src*="/faces/${id}/crop"]`).click();
  await p.locator('#lightbox').waitFor({ state: 'visible' });
  await p.locator('#lb-info').click();
  await p.locator('#lb-panel .pface button.add').click();
  const field = p.locator('#lb-panel .pface .pfield input');
  await field.fill('Vera Viewer');
  const from = t.log.length;
  const release = await holdInfo(p);
  try {
    const answer = posted(p, /^\/faces\/assign$/);
    await field.press('Enter');
    await answer;
    await until('the clusters to be asked for while the grouping is not known to be done', () => requests(t.log, from, '/clusters').length >= 2);
    await p.locator(`#page img[src*="/faces/${id}/crop"]`).first().waitFor({ state: 'detached' });
  } finally {
    await release();
  }
  await settled(p, t);
  check(await p.locator('#lightbox').isVisible(), 'the viewer stays open');
  await sidebarIs(p, await server(), 'after the name');
}));

// Each time a face of another photo than the picture's.
await test('"Use as picture" in the photo grid, on the person\'s page and in the viewer', on((finn) => `person=${finn.id}`, () => named('c04', 'Finn Picture'), async (p, t, finn) => {
  const faces = (await api(`/people/${finn.id}/faces?state=confirmed`)).faces;
  const cover = async () => (await personNamed('Finn Picture')).cover;
  const otherFile = async () => { const c = await cover(); return faces.find((f) => f.id !== c).file; };
  const useAs = await tr(p, 'menu.use_as', { name: 'Finn Picture' });
  const changed = async (why, act) => {
    const before = await cover();
    await fromReply(p, t, why, async () => {
      const reply = posted(p, /^\/people\/\d+\/cover$/);
      await act();
      await p.getByRole('menuitem', { name: useAs, exact: true }).click();
      return reply;
    });
    check((await cover()) !== before, `${why}: the picture did not change`);
  };
  const cellOf = async (file) => {
    await p.locator('.cell').first().waitFor();
    return p.locator(`.cell[data-index="${await p.evaluate((f) => state.data.ids.indexOf(f), file)}"]`);
  };

  await changed('grid', async () => openMenu(p, await cellOf(await otherFile()), { button: 'right' }));

  await p.evaluate((id) => { location.hash = `view=person&id=${id}&tab=confirmed`; }, finn.id);
  await p.locator('#pp-grid .face').first().waitFor();
  await quiet(t);
  const now = await cover();
  const pick = faces.find((f) => f.id !== now);
  await changed('person page', () => openMenu(p, p.locator(`#pp-grid .face[data-key="f${pick.id}"] .more-btn`)));

  await p.evaluate((id) => { location.hash = `person=${id}`; }, finn.id);
  const cell = await cellOf(await otherFile());
  await quiet(t);
  await cell.focus();
  await p.keyboard.press(' ');
  await p.locator('#lightbox').waitFor({ state: 'visible' });
  await p.locator('#lb-info').click();
  await changed('viewer', () => openMenu(p, p.locator('#lb-panel .pcell .more-btn')));
}));

await test('a person moved to a group and out again on the real server: the group is gone', on('view=people', async () => {
  const anton = await named('c02', 'Anton Move'), bea = await named('c06', 'Bea Move');
  await api('/groups', { name: 'Move Group', person_id: bea.id });
  return { anton, bea };
}, async (p, t, { anton, bea }) => {
  const move = (x, to) => fromReply(p, t, `${x.name} to ${to}`, async () => {
    await openMenu(p, p.locator(`#page .person[data-id="${x.id}"] .more-btn`));
    await menu(p, 'people.menu.move');
    const reply = posted(p, /^\/people\/\d+\/group$/);
    await p.locator('#modal .taglist button', { hasText: to }).click();
    return reply;
  });
  await move(anton, 'Move Group');
  const none = await tr(p, 'people.no_group');
  await move(bea, none);
  await move(anton, none);
  check(!(await groupNames(p)).includes('Move Group'), 'the empty group left the sidebar');
  check(!(await api('/groups')).some((g) => g.name === 'Move Group'), 'and the server');
}));

// The server answers a rename or a merge from the people view it keeps
// between requests (`App::people_view`), read again after the change: the
// person in the reply is the one it lists right after, and the sidebar built
// from the reply is what the server has.
await test('a person renamed and merged on the real server: the replies are what the server then lists', on('view=people', async () => {
  // Two faces of the card to a misspelt name, the rest to the right one.
  const c = await clusterOf('c03');
  await api(`/clusters/${c.id}/name`, { name: 'Ida Typo', generation: c.generation, faces: c.faces.slice(0, 2).map((f) => f.id) });
  await grouped();
  const ida = await named('c03', 'Ida Server');
  const typo = await personNamed('Ida Typo');
  check(typo.faces === 2 && ida.faces === c.size - 2, `set up: ${typo.faces} and ${ida.faces} faces`);
  return { ida, typo, all: c.size };
}, async (p, t, { ida, typo, all }) => {
  const listed = async (id) => (await api('/people')).find((x) => x.id === id);
  const same = async (reply, why) => {
    const want = await listed(reply.id);
    check(JSON.stringify(reply) === JSON.stringify(want), `${why}: the reply ${JSON.stringify(reply)} is not what the server lists, ${JSON.stringify(want)}`);
  };
  const renamed = await fromReply(p, t, 'renamed', async () => {
    await openMenu(p, p.locator(`#page .person[data-id="${ida.id}"] .more-btn`));
    await menu(p, 'people.menu.rename');
    const reply = posted(p, /^\/people\/\d+\/rename$/);
    await typeName(p, 'Ida Renamed');
    return reply;
  });
  check(renamed.name === 'Ida Renamed' && renamed.faces === ida.faces && renamed.cover === ida.cover, `renamed: ${JSON.stringify(renamed)}`);
  await same(renamed, 'renamed');
  const merged = await fromReply(p, t, 'merged', async () => {
    await openMenu(p, p.locator(`#page .person[data-id="${typo.id}"] .more-btn`));
    await menu(p, 'people.menu.merge');
    await p.locator('#modal-body .pfield input').fill('Ida Renamed');
    await p.locator('#modal-body .pfield input').press('Enter');
    const reply = posted(p, /^\/people\/\d+\/merge$/);
    await p.locator('#modal-actions button').last().click();
    return reply;
  });
  check(merged.id === ida.id && merged.name === 'Ida Renamed' && merged.faces === all && merged.photos === all, `merged: ${JSON.stringify(merged)}`);
  await same(merged, 'merged');
  check(!(await listed(typo.id)), 'Ida Typo is gone from the server');
}));

// The search box against the real server: the fake's `/suggest` (fake-api.mjs)
// answers as the server does, and the page sends one request per pause, not
// the sections one by one (the timing and calling off are checked on a fake
// clock in faces-mocked.mjs). The tag is this test's own.
await test('the search box on the real server: one request per pause, the list is what the server suggests', on('', async () => {
  const ids = (await api('/timeline')).ids.slice(0, 2);
  await api('/tags/add', { ids, name: 'Suchprobe' });
}, async (p, t) => {
  await p.focus('#search');
  await quiet(t);
  let from = t.log.length;
  await p.keyboard.type('suchpr'); // no pause between the letters
  await until('the photos for "suchpr"', () => requests(t.log, from, '/timeline').length > 0);
  await p.locator('#suggest .item', { hasText: 'Suchprobe' }).waitFor();
  await quiet(t);
  const asked = t.log.slice(from).map((e) => e.path + (e.path === '/suggest' || e.path === '/timeline' ? '?q=' + e.query.get('q') : ''));
  check(asked.join() === '/suggest?q=suchpr,/timeline?q=suchpr', `while typing the page asked for [${asked}]`);
  const reply = await api('/suggest?q=suchpr');
  check(reply.tags.length === 1 && reply.tags[0].name === 'Suchprobe' && reply.tags[0].count === 2, `the server suggests ${JSON.stringify(reply.tags)}`);
  const rows = await p.locator('#suggest .item').evaluateAll((bs) => bs.map((b) => b.querySelector('.label').textContent + '=' + b.querySelector('.count').textContent));
  check(rows.includes('# Suchprobe=2'), `the list shows [${rows}]`);
  // date: the server answers only dates; the list shows as many rows.
  from = t.log.length;
  await p.fill('#search', 'date:');
  await until('the dates', () => requests(t.log, from, '/suggest').length === 1);
  await quiet(t);
  const dates = (await api('/suggest?q=date:')).dates;
  check(dates.length > 0, 'the server has no years to offer');
  const shown = await p.locator('#suggest .item').count();
  check(shown === dates.length, `the list shows ${shown} rows, the server suggests ${dates.length} dates`);
  check(t.log.slice(from).every((e) => e.path === '/suggest'), `date: asked for [${t.log.slice(from).map((e) => e.path)}]`);
}));

await finish();
