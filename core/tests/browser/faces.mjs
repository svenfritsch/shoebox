// What unnamed.mjs and people.mjs share: the browser, the library's API (for
// setting up and for what the server says now), a log of the requests a page
// makes, and the checks of the sidebar's people list. The library is the one
// make-faces.py draws, served by `shoebox serve` (see run.sh).
//
// Every test has a page of its own and works on clusters (folders) and names
// no other test uses, so the order of the tests does not matter. Nothing waits
// a fixed time for something to happen: the waits are on the server (the
// grouping is done), on the page's requests and on the page itself, with long
// timeouts, so a slow machine only makes the tests slower. The one exception
// is `nothingFor`: a check that something does NOT happen needs a time window.
import { createRequire } from 'node:module';
const { chromium } = createRequire(import.meta.url)('playwright'); // honours NODE_PATH

const url = process.argv[2];
const T = 60000; // for anything that waits on the server
const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {});

const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 } });
await ctx.addInitScript(() => { try { localStorage.setItem('shoebox.lang', 'en'); } catch (e) {} });
ctx.setDefaultTimeout(T);
// APP_JS=<file>: the page runs this app.js instead of the one built into shoebox
// (to see that a test fails when the page does something else, without a build).
if (process.env.APP_JS) await ctx.route((u) => new URL(u).pathname === '/app.js', (r) => r.fulfill({ path: process.env.APP_JS, contentType: 'application/javascript' }));

// The library's API, as the page uses it (`LIBAPI`).
const LIB = '/api/lib/' + (await (await fetch(new URL('/api/libraries', url))).json())[0].id;

export async function api(path, body) {
  const r = await fetch(new URL(LIB + path, url), body === undefined ? {} : {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-Shoebox': '1' },
    body: JSON.stringify(body),
  });
  if (!r.ok) throw new Error(`${path}: HTTP ${r.status} ${await r.text()}`);
  return r.json();
}

export function check(ok, what) { if (!ok) throw new Error(what); }

const pause = (ms) => new Promise((r) => setTimeout(r, ms));

// Asks again every 50 ms until `get` returns something, at most T.
export async function until(what, get) {
  const end = Date.now() + T;
  for (;;) {
    const v = await get();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out: ${what}`);
    await pause(50);
  }
}

const idle = (c) => !c.stale && !c.running && !c.embedding;

// The server's clusters once nothing is being grouped any more.
export function grouped() {
  return until('the grouping to finish', async () => {
    const info = await api('/info');
    return !info.busy && idle(info.clusters) && info.clusters;
  });
}

// The unnamed clusters with the folder of their faces (each folder of
// make-faces.py is one cluster), the faces as the page shows them first.
const folders = {};
export async function clusters(samples = 8) {
  const r = await api(`/clusters?limit=500&samples=${samples}`);
  for (const c of r.clusters) {
    const file = c.faces[0].file;
    if (!folders[file]) folders[file] = (await api(`/files/${file}`)).path.split('/')[0];
    c.folder = folders[file];
  }
  return r.clusters;
}

export async function clusterOf(folder, samples) {
  const c = (await clusters(samples)).find((x) => x.folder === folder);
  check(c, `no cluster of ${folder}`);
  return c;
}

// Setting up through the API: the faces of `folder` are named `name`.
export async function nameFolder(folder, name) {
  const c = await clusterOf(folder);
  const r = await api(`/clusters/${c.id}/name`, { name, generation: c.generation });
  await grouped();
  return r.person;
}

export async function personNamed(name) {
  const p = (await api('/people')).find((x) => x.name === name);
  check(p, `nobody named ${name}`);
  return p;
}

// ---- the requests of a page

// The page's requests to the library's API (not pictures, not the event
// stream) in order, with what /info answered in between: `{ path, query,
// method }` relative to the library (`/people`, `/clusters`, ...) and
// `{ info }` (the clusters part) when an answer to /info arrives.
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
  p.on('response', (res) => {
    const o = ours(res.url());
    if (!o || o.path !== '/info') return;
    const entry = { info: null };
    t.log.push(entry);
    res.json().then((j) => { entry.info = j.clusters; }).catch(() => {});
  });
  // Where the log is now; the counts below are of what came after.
  t.mark = () => t.log.length;
  t.requests = (from, path, method = 'GET') => t.log.slice(from).filter((e) => e.path === path && e.method === method);
  t.count = (from, path, method) => t.requests(from, path, method).length;
  return t;
}

// No request of ours open (but those to `except`, held by `holdInfo`) and none
// started for `ms`.
export async function quiet(t, ms = 1000, except = null) {
  await until('the page to stop asking the server', () => ![...t.open.values()].some((path) => path !== except) && Date.now() - t.last >= ms);
}

// A check that something does not happen: the page is left alone this long.
export const nothingFor = (p, ms = 700) => p.waitForTimeout(ms);

// The server is done grouping, the page has heard so (its last status says
// what the server says now: when the grouping finished, in seconds, and the
// counts), and asks nothing more.
export async function settled(p, t) {
  const c = await grouped();
  const sig = (c) => [c.finished_at, c.clusters, c.unnamed, c.people, c.suggested, c.stale, c.running, c.embedding].join();
  await p.waitForFunction(([want, fields]) => {
    const c = window.people && people.info && people.info.clusters;
    return c && fields.map((k) => c[k]).join() === want;
  }, [sig(c), ['finished_at', 'clusters', 'unnamed', 'people', 'suggested', 'stale', 'running', 'embedding']], { timeout: T });
  await quiet(t);
  return c;
}

// The page's answers to /info wait until `release()`: the page cannot learn that
// a grouping is done, so whatever it does meanwhile it does at once.
export async function holdInfo(p) {
  let open;
  const gate = new Promise((r) => { open = r; });
  const pattern = (u) => new URL(u).pathname === LIB + '/info';
  const handler = async (route) => { await gate; await route.continue().catch(() => {}); };
  await p.route(pattern, handler);
  return async () => { open(); await p.unroute(pattern, handler); };
}

// The page's answers to /info show the grouping at `step` ('stale': asked
// for, 'running') until it is set to null: its steps one at a time, however
// fast the server is.
export async function fakeGrouping(p) {
  const s = { step: null };
  await p.route((u) => new URL(u).pathname === LIB + '/info', async (route) => {
    const res = await route.fetch();
    const j = await res.json();
    if (s.step) j.clusters = { ...j.clusters, stale: s.step === 'stale', running: s.step === 'running' };
    await route.fulfill({ response: res, json: j });
  });
  return s;
}

// The reply to the next POST whose path (relative to the library) matches.
export function posted(p, re) {
  return p.waitForResponse((r) => r.request().method() === 'POST' && re.test(new URL(r.url()).pathname.slice(LIB.length)), { timeout: T })
    .then(async (r) => { check(r.ok(), `POST ${r.url()}: HTTP ${r.status()} ${await r.text()}`); return r.json(); });
}

// A UI text in the page's language (never written out here).
export const tr = (p, key, vars) => p.evaluate(([k, v]) => tr(k, v), [key, vars || {}]);

// ---- tests

let failures = 0;
const only = process.env.ONLY; // run only the tests whose name has this in it

// `fn(p, t, s)` on a new page, opened at `hash` (a string, or a function of
// what `setup` returned) and settled (see `settled`), with `t` its request log
// and `s` what `setup()` (run first, through the API) returned. Errors in the
// page fail the test; `fn` returns 'skip: why' to skip it.
export async function test(name, { hash, setup }, fn) {
  if (only && !name.includes(only)) return;
  const p = await ctx.newPage();
  const errors = [];
  p.on('pageerror', (e) => errors.push(e.message));
  const t = track(p);
  try {
    await grouped();
    const s = setup ? await setup() : undefined;
    await p.goto(new URL('#' + (typeof hash === 'function' ? hash(s) : hash), url).href);
    await p.waitForFunction(() => window.people && people.info && !peopleFlight, null, { timeout: T });
    await settled(p, t);
    const r = await fn(p, t, s);
    if (errors.length) throw new Error('error in the page: ' + errors.join('; '));
    console.log(typeof r === 'string' && r.startsWith('skip') ? `skip  ${name}\n      ${r.slice(5).trim()}` : `ok    ${name}`);
  } catch (e) {
    failures++;
    console.log(`FAIL  ${name}\n      ${e.message.split('\n').join('\n      ')}`);
  } finally {
    await p.close();
  }
}

export async function finish() {
  await browser.close();
  if (failures) { console.log(`${failures} failed`); process.exit(1); }
  console.log('all passed');
}

// ---- the sidebar's people list

// The sidebar (Faces) as a list of groups, every group unfolded: name, the
// number shown, and its people (name, photos, picture).
async function sidebar(p) {
  // Unfold one folded group at a time (each click draws the list anew).
  while (await p.evaluate(() => {
    // A group with people has an arrow; unfolded, its people are listed.
    const arrow = (li) => li.querySelector(':scope > .row > button.toggle');
    const folded = [...document.querySelectorAll('#people-tree > li')].find((li) => !li.querySelector('ul') && arrow(li) && arrow(li).firstChild);
    if (folded) arrow(folded).click();
    return !!folded;
  }));
  return p.evaluate(() => [...document.querySelectorAll('#people-tree > li')]
    .filter((li) => !li.querySelector('[data-view]'))
    .map((li) => ({
      name: li.querySelector(':scope > .row > .name').textContent,
      count: li.querySelector(':scope > .row > .count').textContent,
      people: [...li.querySelectorAll(':scope > ul .row')].map((r) => {
        const img = r.querySelector('.avatar img');
        return { name: r.querySelector('.name').textContent, photos: r.querySelector('.count').textContent.replace(/\D/g, ''), picture: img ? img.getAttribute('src').replace(/^.*\/faces\//, '').replace(/[?#].*$/, '') : null };
      }),
    })));
}

// The same as the server sees it now: groups in their order, then those
// without one ("No group", only when someone is in it).
async function expectedSidebar(p) {
  const [groups, list] = [await api('/groups'), await api('/people')];
  const picture = (x) => (x.cover != null ? `${x.cover}/crop` : x.cover_manual != null ? `manual/${x.cover_manual}/crop` : null);
  const entry = (x) => ({ name: x.name, photos: String(x.photos), picture: picture(x) });
  const out = groups.map((g) => ({ id: g.id, name: g.name, people: list.filter((x) => x.group_id === g.id).map(entry) }));
  const none = list.filter((x) => !groups.some((g) => g.id === x.group_id)).map(entry);
  if (none.length) out.push({ id: null, name: await tr(p, 'people.no_group'), people: none });
  for (const g of groups) check(g.people === out.find((s) => s.id === g.id).people.length, `the server counts ${g.people} in ${g.name}`);
  return out.map((s) => ({ name: s.name, count: String(s.people.length), people: s.people }));
}

// The sidebar shows the people and groups as the server has them, and (unless
// `clusters` is false: the page has not heard of the last grouping yet) the
// number of unnamed clusters.
export async function sidebarIsRight(p, why, clusters = true) {
  const want = await expectedSidebar(p), got = await sidebar(p);
  if (JSON.stringify(want) !== JSON.stringify(got)) {
    const show = (x) => x.map((s) => `${s.name} (${s.count}): ${s.people.map((q) => `${q.name} ${q.photos} [${q.picture}]`).join(', ')}`).join('\n  ');
    throw new Error(`${why}: the sidebar shows\n  ${show(got)}\nbut the server has\n  ${show(want)}`);
  }
  if (!clusters) return;
  const c = (await api('/info')).clusters;
  const label = await p.evaluate((n) => tr('people.unnamed_n', { count: trn('count.clusters', n) }), c.clusters);
  const shown = await p.locator('#people-tree [data-view="unnamed"]').textContent();
  check(shown === label, `${why}: the sidebar says "${shown}", the server has ${c.clusters} clusters`);
}

// No request for the people list or the groups since `from`.
export function noListRequests(t, from, why) {
  const n = t.count(from, '/people'), g = t.count(from, '/groups');
  const seen = t.log.slice(from).map((e) => (e.path ? `${e.method} ${e.path}` : `(status: ${e.info ? (e.info.stale || e.info.running || e.info.embedding ? 'grouping' : 'grouped') : '?'})`)).join(', ');
  check(!n && !g, `${why}: the people list was asked for ${n} times, the groups ${g} times (the page asked for: ${seen})`);
}
