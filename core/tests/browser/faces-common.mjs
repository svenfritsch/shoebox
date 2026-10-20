// What faces-mocked.mjs (fake API) and faces-e2e.mjs (the real server) share:
// the browser, a runner, the checks of the sidebar's people list, and ways to
// work the page's menus and dialogs. UI texts are never written out here: they
// come from the page's own `tr`.
import { createRequire } from 'node:module';
const { chromium } = createRequire(import.meta.url)('playwright'); // honours NODE_PATH

export const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {});

export function check(ok, what) { if (!ok) throw new Error(what); }

// Asks again every 50 ms (real time) until `get` returns something, at most 60 s.
export async function until(what, get) {
  const end = Date.now() + 60000;
  for (;;) {
    const v = await get();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out: ${what}`);
    await new Promise((r) => setTimeout(r, 50));
  }
}

// A UI text in the page's language.
export const tr = (p, key, vars) => p.evaluate(([k, v]) => tr(k, v), [key, vars || {}]);

// ---- running tests

let failures = 0;
const only = process.env.ONLY; // run only the tests whose name has this in it

// `fn()` as one test; `close` (the page) afterwards whatever happened.
export async function test(name, fn) {
  if (only && !name.includes(only)) return;
  let page;
  try {
    await fn((p) => { page = p; return p; });
    console.log(`ok    ${name}`);
  } catch (e) {
    failures++;
    console.log(`FAIL  ${name}\n      ${e.message.split('\n').join('\n      ')}`);
  } finally {
    if (page) await page.close();
  }
}

export async function finish() {
  await browser.close();
  if (failures) { console.log(`${failures} failed`); process.exit(1); }
  console.log('all passed');
}

// ---- requests

// `log`: the page's requests to the library's API in order, `{ method, path }`
// relative to it (`/people`, `/clusters`, ...). Counts are of those after `from`.
export const requests = (log, from, path, method = 'GET') => log.slice(from).filter((e) => e.path === path && e.method === method);

export function noListRequests(log, from, why) {
  const n = requests(log, from, '/people').length, g = requests(log, from, '/groups').length;
  const seen = log.slice(from).map((e) => (e.path ? `${e.method} ${e.path}` : `(status: ${e.info ? (e.info.stale || e.info.running ? 'grouping' : 'grouped') : '?'})`)).join(', ');
  check(!n && !g, `${why}: the people list was asked for ${n} times, the groups ${g} times (the page asked for: ${seen})`);
}

// The sizes of the clusters asked for since `from`: 'large, small' is one refresh.
export const clusterRounds = (log, from) => requests(log, from, '/clusters').map((r) => r.query.get('size')).sort().join(', ');

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

// The sidebar shows `groups` and `people` (as the server lists them, in its
// order) and, unless `clusters` is null, that many unnamed clusters.
export async function sidebarIs(p, { groups, people, clusters }, why) {
  const picture = (x) => (x.cover != null ? `${x.cover}/crop` : x.cover_manual != null ? `manual/${x.cover_manual}/crop` : null);
  const entry = (x) => ({ name: x.name, photos: String(x.photos), picture: picture(x) });
  const sections = groups.map((g) => ({ id: g.id, name: g.name, people: people.filter((x) => x.group_id === g.id).map(entry) }));
  const none = people.filter((x) => !groups.some((g) => g.id === x.group_id)).map(entry);
  if (none.length) sections.push({ id: null, name: await tr(p, 'people.no_group'), people: none });
  for (const g of groups) check(g.people === sections.find((s) => s.id === g.id).people.length, `the server counts ${g.people} in ${g.name}`);
  const want = sections.map((s) => ({ name: s.name, count: String(s.people.length), people: s.people }));
  const got = await sidebar(p);
  if (JSON.stringify(want) !== JSON.stringify(got)) {
    const show = (x) => x.map((s) => `${s.name} (${s.count}): ${s.people.map((q) => `${q.name} ${q.photos} [${q.picture}]`).join(', ')}`).join('\n  ');
    throw new Error(`${why}: the sidebar shows\n  ${show(got)}\nbut the server has\n  ${show(want)}`);
  }
  if (clusters == null) return;
  const label = await p.evaluate((n) => tr('people.unnamed_n', { count: trn('count.clusters', n) }), clusters);
  const shown = await p.locator('#people-tree [data-view="unnamed"]').textContent();
  check(shown === label, `${why}: the sidebar says "${shown}", the server has ${clusters} clusters`);
}

export const groupNames = (p) => p.locator('#people-tree > li > .row > .name').allTextContents();

// ---- menus and dialogs

// An item of the menu that is open.
export async function menu(p, key, vars) {
  await p.getByRole('menuitem', { name: await tr(p, key, vars), exact: true }).click();
}

// A name typed into the dialog that is open, then Enter.
export async function typeName(p, name) {
  await p.locator('#modal-body input').fill(name);
  await p.locator('#modal-body input').press('Enter');
}

// A card on the Unnamed page by one of the faces it shows; a tile of the
// single faces.
export const face = (p, id) => p.locator(`img[src*="/faces/${id}/crop"]`);
export const card = (p, id) => p.locator('#un-box .cluster', { has: face(p, id) });
export const single = (p, id) => p.locator('.singles .sface-grid .cface', { has: face(p, id) });

// Typed into a card's name field, then Enter.
export async function nameCard(p, id, name) {
  const field = card(p, id).locator('.pfield input');
  await field.fill(name);
  await field.press('Enter');
}
