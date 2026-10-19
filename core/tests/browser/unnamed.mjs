// The Unnamed page (PR #142): a name given on a card or in the single-faces
// bar comes back with the person (`person_data`), so the page takes them into
// its people list without asking for the list again, and asks for its
// clusters once, when the grouping that follows the name is done (not at once,
// not at each step). A name given in the viewer over the page refreshes it at
// once. And, only on a page with `growGrid` (PR #143), a long card's faces
// come as they are scrolled to, and its tiles stay across Select and a refresh.
// usage: node unnamed.mjs <url>, with the library of make-faces.py (run.sh).
import { test, finish, api, check, clusters, clusterOf, nameFolder, personNamed, settled, until, quiet, nothingFor, holdInfo, fakeGrouping, posted, tr, sidebarIsRight, noListRequests } from './faces.mjs';

// A card, by one of the faces it shows; a tile of the single faces.
const face = (p, id) => p.locator(`img[src*="/faces/${id}/crop"]`);
const card = (p, id) => p.locator('#un-box .cluster', { has: face(p, id) });
const single = (p, id) => p.locator('.singles .sface-grid .cface', { has: face(p, id) });
const singles = async () => (await clusters()).filter((c) => c.folder === 'singles').map((c) => c.faces[0].id);

// A name given on the Unnamed page by `act` (which returns the reply): while
// the page cannot learn that the grouping is done it asks for no clusters;
// afterwards it asks once (the large and the single faces together), and
// never for the people list or the groups.
async function namedOnUnnamed(p, t, act) {
  const from = t.mark();
  const release = await holdInfo(p);
  let reply;
  try {
    reply = await act();
    await nothingFor(p);
    check(!t.count(from, '/clusters'), 'the clusters were asked for at once, before the grouping was done');
    // The person is in the list already, from the reply alone.
    await quiet(t, 300, '/info');
    noListRequests(t, from, 'a name on the Unnamed page');
    await sidebarIsRight(p, 'from the reply, before the grouping', false);
  } finally {
    await release();
  }
  await settled(p, t);
  const sizes = t.requests(from, '/clusters').map((r) => r.query.get('size')).sort().join(', ');
  check(sizes === 'large, small', `after the grouping the clusters were asked for as [${sizes}], expected one refresh (large, small)`);
  noListRequests(t, from, 'a name on the Unnamed page');
  check(reply.person_data && reply.person_data.id === reply.person.id, 'the reply carries the person (person_data)');
  return reply;
}

// Typed into a card's name field, Enter.
async function nameCard(p, id, name) {
  const field = card(p, id).locator('.pfield input');
  await field.fill(name);
  const reply = posted(p, /^\/clusters\/\d+\/name$/);
  await field.press('Enter');
  return reply;
}

await test('a card named with a new name: the person comes from the reply, the clusters once the grouping is done', { hash: 'view=unnamed' }, async (p, t) => {
  const c = await clusterOf('c01');
  const r = await namedOnUnnamed(p, t, () => nameCard(p, c.faces[0].id, 'Nora Card'));
  check(r.person_data.name === 'Nora Card' && r.person_data.photos === c.size, `the reply's person: ${JSON.stringify(r.person_data)}`);
  await card(p, c.faces[0].id).waitFor({ state: 'detached' });
  await sidebarIsRight(p, 'after the name');
});

await test('a card named for someone the list has (typed): their entry is replaced, no list request', {
  hash: 'view=unnamed',
  setup: async () => { await nameFolder('c02', 'Olga Typed'); return personNamed('Olga Typed'); },
}, async (p, t, olga) => {
  const c = await clusterOf('c03');
  const r = await namedOnUnnamed(p, t, () => nameCard(p, c.faces[0].id, 'Olga Typed'));
  check(r.person.id === olga.id, `named ${JSON.stringify(r.person)}, not Olga (${olga.id})`);
  check((await personNamed('Olga Typed')).photos === olga.photos + c.size, 'Olga has the photos of both cards');
  await card(p, c.faces[0].id).waitFor({ state: 'detached' });
  await sidebarIsRight(p, 'after the name');
});

await test('a card named from its suggestion (✓): no list request, one refresh', {
  hash: 'view=unnamed',
  setup: () => nameFolder('look', 'Pia Look'),
}, async (p, t, pia) => {
  const c = await clusterOf('lookalike');
  check(c.suggestion && c.suggestion.person.id === pia.id, `the look-alikes are suggested for Pia: ${JSON.stringify(c.suggestion)}`);
  const sugg = card(p, c.faces[0].id).locator('.csugg');
  check((await sugg.locator('b').textContent()) === 'Pia Look', 'the card says who they look like');
  await namedOnUnnamed(p, t, () => {
    const reply = posted(p, /^\/clusters\/\d+\/name$/);
    return sugg.locator('button').first().click().then(() => reply);
  });
  await card(p, c.faces[0].id).waitFor({ state: 'detached' });
  await sidebarIsRight(p, 'after the name');
});

await test('single faces named with a new name from the bar: no list request, one refresh', { hash: 'view=unnamed' }, async (p, t) => {
  const [a, b] = await singles();
  await single(p, a).click();
  await single(p, b).click();
  const bar = p.locator('.singles-bar');
  check(await bar.isVisible(), 'picking faces shows the bar');
  const r = await namedOnUnnamed(p, t, async () => {
    await bar.locator('.pfield input').fill('Sam Single');
    const reply = posted(p, /^\/faces\/assign$/);
    await bar.locator('.pfield input').press('Enter');
    return reply;
  });
  check(r.person_data.name === 'Sam Single' && r.person_data.photos === 2, `the reply's person: ${JSON.stringify(r.person_data)}`);
  await single(p, a).waitFor({ state: 'detached' });
  await single(p, b).waitFor({ state: 'detached' });
  check(await bar.isHidden(), 'the bar goes with the picked faces');
  await sidebarIsRight(p, 'after the name');
});

await test('a single face named for someone the list has (Name button): no list request, one refresh', {
  hash: 'view=unnamed',
  setup: () => nameFolder('c04', 'Rita Button'),
}, async (p, t, rita) => {
  const a = (await singles())[2];
  await single(p, a).click();
  const bar = p.locator('.singles-bar');
  const r = await namedOnUnnamed(p, t, async () => {
    await bar.locator('.pfield input').fill('Rita Button');
    await bar.locator('.pfield input').press('Escape'); // the list folds away
    const reply = posted(p, /^\/faces\/assign$/);
    await bar.locator(':scope > button.btn:not(.quiet)').click();
    return reply;
  });
  check(r.person.id === rita.id, `named ${JSON.stringify(r.person)}, not Rita (${rita.id})`);
  await single(p, a).waitFor({ state: 'detached' });
  await sidebarIsRight(p, 'after the name');
});

// The page hears of each step as the event stream would tell it (`loadInfo`).
await test('the steps of the grouping after a name (asked for, running, done): one refresh, at the end', { hash: 'view=unnamed' }, async (p, t) => {
  const c = await clusterOf('c16');
  const grouping = await fakeGrouping(p);
  const from = t.mark();
  grouping.step = 'stale';
  await nameCard(p, c.faces[0].id, 'Stefan Steps');
  const step = async (name, flag) => {
    grouping.step = name;
    await p.evaluate(() => loadInfo());
    await p.waitForFunction((k) => people.info.clusters[k] === true, flag);
    await quiet(t, 300);
    check(!t.count(from, '/clusters'), `the clusters were asked for while the grouping was ${name}`);
  };
  await step('stale', 'stale');
  await step('running', 'running');
  grouping.step = null;
  await p.evaluate(() => loadInfo());
  await settled(p, t);
  const sizes = t.requests(from, '/clusters').map((r) => r.query.get('size')).sort().join(', ');
  check(sizes === 'large, small', `once the grouping was done the clusters were asked for as [${sizes}], expected one refresh (large, small)`);
  noListRequests(t, from, 'a name on the Unnamed page');
});

// The viewer has no card that shows the change: the page asks for its
// clusters at once (and the people list, which is not part of #142 here).
await test('a name given in the viewer over the Unnamed page refreshes the page at once', { hash: 'view=unnamed' }, async (p, t) => {
  const c = await clusterOf('c05');
  const id = c.faces[0].id;
  await card(p, id).locator(`img[src*="/faces/${id}/crop"]`).click();
  await p.locator('#lightbox').waitFor({ state: 'visible' });
  await p.locator('#lb-info').click();
  const add = p.locator('#lb-panel .pface button.add');
  await add.click();
  const field = p.locator('#lb-panel .pface .pfield input');
  await field.fill('Vera Viewer');
  const from = t.mark();
  const release = await holdInfo(p);
  try {
    const reply = posted(p, /^\/faces\/assign$/);
    await field.press('Enter');
    await reply;
    await until('the clusters to be asked for while the grouping is not known to be done', () => t.count(from, '/clusters') >= 2);
    await p.locator(`#page img[src*="/faces/${id}/crop"]`).first().waitFor({ state: 'detached' });
  } finally {
    await release();
  }
  await settled(p, t);
  check(await p.locator('#lightbox').isVisible(), 'the viewer stays open');
  await sidebarIsRight(p, 'after the name');
});

// PR #143. A card shows at most `clusters::MAX_CLUSTER` (100) faces, fewer than
// GROW_STEP (120): with the page's own step all faces come at once. The parts
// are tried with a smaller step (set in the page) and faces as wide as the
// screen, so the end of a part is far below it.
await test('PR #143: a long card adds its faces as they are scrolled to, its tiles stay across Select and a refresh', { hash: 'view=unnamed' }, async (p, t) => {
  if (!(await p.evaluate(() => typeof growGrid === 'function'))) return 'skip: the page has no growGrid (PR #143 is not in this build)';
  const big = await clusterOf('big');
  check(big.size === 100, `the big card has ${big.size} faces`);
  const k = card(p, big.faces[0].id);
  const tiles = () => k.locator('.cfaces .cface').count();
  // All of them with the page's step.
  await k.locator('.cmore').click();
  await until('all faces of the big card', async () => (await tiles()) === 100);
  check(!(await k.locator('.grow-end').count()), 'no end marker once all are in');
  await k.locator('.cclose').click();
  await until('the card closed again', async () => (await tiles()) === 8);

  await p.evaluate(() => { GROW_STEP = 40; document.getElementById('page').style.setProperty('--face-size', '600px'); });
  await k.locator('.cmore').click();
  await until('the first part', async () => (await tiles()) === 40 && (await k.locator('.grow-end').count()) === 1);
  // Marks on the tiles (the same elements keep them).
  await k.evaluate((c) => c.querySelectorAll('.cfaces .cface').forEach((f, i) => { f.dataset.testMark = i; }));
  await k.locator('.grow-end').scrollIntoViewIfNeeded();
  await until('the second part', async () => (await tiles()) === 80);
  await k.locator('.grow-end').scrollIntoViewIfNeeded();
  await until('the rest', async () => (await tiles()) === 100 && !(await k.locator('.grow-end').count()));

  const marked = () => k.evaluate((c) => [...c.querySelectorAll('.cfaces .cface')].filter((f) => f.dataset.testMark != null).length);
  await k.locator('.cbar button', { hasText: await tr(p, 'facecheck.select') }).click();
  await until('Select', async () => (await k.getAttribute('class')).includes('picking'));
  await settled(p, t);
  check((await marked()) === 40, `after Select ${await marked()} of the 40 marked tiles are left`);
  check((await tiles()) === 100, `after Select the card shows ${await tiles()} faces`);

  const other = await clusterOf('c06');
  await namedOnUnnamed(p, t, () => nameCard(p, other.faces[0].id, 'Uwe Elsewhere'));
  check(await k.count() === 1 && (await k.getAttribute('class')).includes('picking'), 'the big card is still there, still selecting');
  check((await marked()) === 40, `after a name elsewhere ${await marked()} of the 40 marked tiles are left`);
  check((await tiles()) === 100, `after a name elsewhere the card shows ${await tiles()} faces`);
});

await finish();
