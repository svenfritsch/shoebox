// Groups and people in the sidebar (PR #142): moving a person to a group, a
// new group, renaming, ordering and deleting groups, "Use as picture" (photo
// grid, a person's page, the viewer), renaming, merging and deleting a person
// change the people list from the reply, without asking for the list or the
// groups again. After each the sidebar (Faces) shows what the server has:
// groups in their order with their counts, empty ones gone, people in order
// with their photos and pictures.
// usage: node people.mjs <url>, with the library of make-faces.py (run.sh).
import { test, finish, api, check, nameFolder, personNamed, quiet, holdInfo, settled, posted, tr, sidebarIsRight, noListRequests } from './faces.mjs';

// `act` changes something and returns the reply. Until the page learns that
// the grouping the server does after every change to people is done (its
// /info is held), it asks for neither the people list nor the groups, and the
// sidebar is right from the reply alone. After the grouping only the People
// page asks for the list, once (the suggestions changed), as it should.
async function fromReply(p, t, why, act) {
  const from = t.mark();
  const release = await holdInfo(p);
  let reply;
  try {
    reply = await act();
    await quiet(t, 1000, '/info');
    noListRequests(t, from, why);
    await sidebarIsRight(p, why, false);
  } finally {
    await release();
  }
  const held = t.mark();
  await settled(p, t);
  const n = t.count(held, '/people'), g = t.count(held, '/groups');
  if (await p.evaluate(() => state.filter.view === 'people')) check(n <= 1 && g <= 1, `${why}: after the grouping the People page asked for the list ${n} times, the groups ${g} times`);
  else noListRequests(t, held, `${why}, after the grouping`);
  await sidebarIsRight(p, `${why}, after the grouping`);
  return reply;
}

const named = async (folder, name) => { await nameFolder(folder, name); return personNamed(name); };
const group = (name, person) => api('/groups', { name, person_id: person.id });

// Opens the menu of `button` (a ⋯ button, or with `{ button: 'right' }` a
// photo). The page closes a menu when it scrolls, so the button is scrolled to
// first and the scroll events are let through before the click.
async function openMenu(p, button, opts) {
  await button.scrollIntoViewIfNeeded();
  await p.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  await button.click(opts);
  await p.locator('.menu').waitFor();
}

// An item of the menu that is open.
async function menu(p, key, vars) {
  await p.getByRole('menuitem', { name: await tr(p, key, vars), exact: true }).click();
}

// A name typed into the dialog that is open, Enter.
async function typeName(p, name, re) {
  const reply = posted(p, re);
  await p.locator('#modal-body input').fill(name);
  await p.locator('#modal-body input').press('Enter');
  return reply;
}

const groupNames = (p) => p.locator('#people-tree > li > .row > .name').allTextContents();

await test('a person moved to a group, then both out of it: the empty group is gone', {
  hash: 'view=people',
  setup: async () => {
    const anton = await named('c07', 'Anton Move'), bea = await named('c08', 'Bea Move');
    await group('Move Group', bea);
    return { anton, bea };
  },
}, async (p, t, { anton, bea }) => {
  const move = async (who, to) => {
    await openMenu(p, p.locator(`#page .person[data-id="${who.id}"] .more-btn`));
    await menu(p, 'people.menu.move');
    const reply = posted(p, /^\/people\/\d+\/group$/);
    await p.locator('#modal .taglist button', { hasText: to }).click();
    return reply;
  };
  await fromReply(p, t, 'Anton to Move Group', () => move(anton, 'Move Group'));
  check(await p.locator('#page .pgroup', { hasText: 'Move Group' }).locator(`.person[data-id="${anton.id}"]`).count() === 1, 'the People page shows Anton in Move Group');
  const none = await tr(p, 'people.no_group');
  await fromReply(p, t, 'Bea to no group', () => move(bea, none));
  await fromReply(p, t, 'Anton to no group', () => move(anton, none));
  check(!(await groupNames(p)).includes('Move Group'), 'the empty group left the sidebar');
});

await test('a new group with a person', {
  hash: 'view=people',
  setup: () => named('c09', 'Cleo New'),
}, async (p, t, cleo) => {
  await fromReply(p, t, 'a new group', async () => {
    await openMenu(p, p.locator(`#page .person[data-id="${cleo.id}"] .more-btn`));
    await menu(p, 'people.menu.new_group');
    return typeName(p, 'Cleo Group', /^\/groups$/);
  });
  check((await groupNames(p)).includes('Cleo Group'), 'the sidebar has the new group');
});

await test('groups ordered, renamed and deleted in the groups dialog', {
  hash: 'view=people',
  setup: async () => {
    await group('Order One', await named('c10', 'Dan Groups'));
    await group('Order Two', await named('c11', 'Eva Groups'));
  },
}, async (p, t) => {
  await p.locator('#page .toolbar button', { hasText: await tr(p, 'people.groups_btn') }).click();
  const row = (name) => p.locator('#modal .grow-row', { has: p.locator('.gname', { hasText: name }) });
  await fromReply(p, t, 'Order Two moved up', async () => {
    const reply = posted(p, /^\/groups\/reorder$/);
    await row('Order Two').locator('.grip').focus();
    await p.keyboard.press('Alt+ArrowUp');
    return reply;
  });
  const names = await groupNames(p);
  check(names.indexOf('Order Two') < names.indexOf('Order One'), `the sidebar's groups: ${names}`);
  const rename = await tr(p, 'people.rename'), del = await tr(p, 'app.delete');
  await fromReply(p, t, 'Order One renamed', async () => {
    await row('Order One').locator(`button[title="${rename}"]`).click();
    return typeName(p, 'Order Renamed', /^\/groups\/\d+\/rename$/);
  });
  await row('Order Renamed').waitFor(); // the groups dialog again, with the new name
  await fromReply(p, t, 'Order Two deleted', async () => {
    await row('Order Two').locator(`button[title="${del}"]`).click();
    const reply = posted(p, /^\/groups\/\d+\/delete$/);
    await p.locator('#modal-actions .btn.danger').click();
    return reply;
  });
  const after = await groupNames(p);
  check(after.includes('Order Renamed') && !after.includes('Order One') && !after.includes('Order Two'), `the sidebar's groups: ${after}`);
});

// Each time a face of another photo than the picture's.
await test('"Use as picture" in the photo grid, on the person\'s page and in the viewer', {
  hash: (finn) => `person=${finn.id}`,
  setup: () => named('c12', 'Finn Picture'),
}, async (p, t, finn) => {
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
  await changed('viewer', () => openMenu(p, p.locator('#lb-panel .pface .more-btn')));
});

// From the person's photos (the grid), not the People page: there a grouping
// that ends asks for the list on purpose (the suggestions changed).
await test('a person renamed', {
  hash: (gus) => `person=${gus.id}`,
  setup: () => named('c13', 'Gus Rename'),
}, async (p, t, gus) => {
  await fromReply(p, t, 'renamed', async () => {
    await openMenu(p, p.locator('.person-head .more-btn'));
    await menu(p, 'people.menu.rename');
    return typeName(p, 'Aaron Renamed', /^\/people\/\d+\/rename$/);
  });
  check((await personNamed('Aaron Renamed')).id === gus.id, 'renamed on the server');
});

await test('a person merged into someone without a group: they take the group', {
  hash: (s) => `person=${s.hana.id}`,
  setup: async () => {
    const hana = await named('c14', 'Hana Merge'), ida = await named('c15', 'Ida Merge');
    await group('Merge Group', hana);
    return { hana, ida };
  },
}, async (p, t, { hana, ida }) => {
  await fromReply(p, t, 'merged', async () => {
    await openMenu(p, p.locator('.person-head .more-btn'));
    await menu(p, 'people.menu.merge');
    await p.locator('#modal-body .pfield input').fill('Ida Merge');
    await p.locator('#modal-body .pfield input').press('Enter');
    const reply = posted(p, /^\/people\/\d+\/merge$/);
    await p.locator('#modal-actions button').last().click();
    return reply;
  });
  const after = await personNamed('Ida Merge');
  check(after.id === ida.id && after.group_id != null, `Ida after the merge: ${JSON.stringify(after)}`);
  check(!(await api('/people')).some((x) => x.id === hana.id), 'Hana is gone');
});

await test('a person without faces deleted: their group goes with them', {
  hash: 'view=people',
  setup: async () => {
    const zed = await api('/people', { name: 'Zed Delete' });
    await group('Delete Group', zed);
    return zed;
  },
}, async (p, t, zed) => {
  await fromReply(p, t, 'deleted', async () => {
    await openMenu(p, p.locator(`#page .person[data-id="${zed.id}"] .more-btn`));
    await menu(p, 'people.menu.delete');
    const reply = posted(p, /^\/people\/\d+\/delete$/);
    await p.locator('#modal-actions .btn.danger').click();
    return reply;
  });
  check(!(await groupNames(p)).includes('Delete Group'), 'the group left the sidebar');
});

await finish();
