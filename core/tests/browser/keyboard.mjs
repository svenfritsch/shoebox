// Keyboard use of the photo app: the arrow keys in the grid and the viewer, the
// selection bar as one toolbar (one Tab stop, arrows, Home, End, B there and
// back), and dialogs (the focus goes in, Tab stays in, and comes back where it
// was when the dialog closes), and selecting photos by dragging the mouse
// across them while selecting. usage: node keyboard.mjs <url>
// Needs Playwright with a Chromium (CHROME=path) and a library served by
// `shoebox serve` (see run.sh). Nothing here confirms a dialog: no photo is
// moved, tagged or trashed.
import { createRequire } from 'node:module';
const { chromium } = createRequire(import.meta.url)('playwright'); // honours NODE_PATH
const [url] = process.argv.slice(2);
const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {});
const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 } });
await ctx.addInitScript(() => { try { localStorage.setItem('shoebox.lang', 'en'); } catch (e) {} });

let failures = 0;
async function test(name, fn) {
  const p = await ctx.newPage();
  const errors = [];
  p.on('pageerror', (e) => errors.push(e.message));
  try {
    await p.goto(url);
    await p.waitForSelector('.cell');
    await fn(p);
    if (errors.length) throw new Error('error in the page: ' + errors.join('; '));
    console.log(`ok    ${name}`);
  } catch (e) {
    failures++;
    console.log(`FAIL  ${name}\n      ${e.message.split('\n')[0]}`);
  } finally {
    await p.close();
  }
}

// What has the focus, in words: "cell:3" for a photo, "#id", or a tag and the
// nearest element with an id around it.
const focused = (p) => p.evaluate(() => {
  const a = document.activeElement;
  if (!a || a === document.body) return 'body';
  if (a.classList.contains('cell')) return 'cell:' + a.dataset.index;
  if (a.id) return '#' + a.id;
  const around = a.parentElement && a.parentElement.closest('[id]');
  return a.tagName.toLowerCase() + ' in #' + (around ? around.id : '?');
});

// Waits (briefly) until the focus is where it should be.
async function expectFocus(p, want, why) {
  let got;
  for (let i = 0; i < 40; i++) {
    got = await focused(p);
    if (typeof want === 'function' ? want(got) : got === want) return got;
    await p.waitForTimeout(50);
  }
  throw new Error(`${why}: focus on ${got}, expected ${typeof want === 'function' ? 'another place' : want}`);
}

function check(ok, what) { if (!ok) throw new Error(what); }

const visible = (p, sel) => p.locator(sel).isVisible();
const cellOf = (f) => (f.startsWith('cell:') ? parseInt(f.slice(5), 10) : -1);

// The selection bar's tools in keyboard order: the buttons that are shown and
// not disabled (a dimmed tool, nothing selected yet, stays in the order).
const barTools = (p) => p.evaluate(() => [...document.querySelectorAll('#selbar button')]
  .filter((b) => !b.hidden && !b.disabled && b.getClientRects().length).map((b) => '#' + b.id));

// Selecting, with photo `pick` selected (or none) and the focus on photo `at`.
async function startSelecting(p, pick, at) {
  await p.locator('#select').focus();
  await p.keyboard.press('Enter');
  check(await visible(p, '#selbar'), 'Enter on the select button shows the selection bar');
  if (pick != null) {
    await p.locator(`.cell[data-index="${pick}"]`).focus();
    await p.keyboard.press(' ');
    check(await p.locator(`.cell[data-index="${pick}"]`).evaluate((c) => c.classList.contains('sel')), 'Space selects the focused photo');
  }
  await p.locator(`.cell[data-index="${at}"]`).focus();
}

await test('grid: the arrow keys go from photo to photo', async (p) => {
  await p.locator('.cell[data-index="0"]').focus();
  await p.keyboard.press('ArrowRight'); await expectFocus(p, 'cell:1', 'ArrowRight');
  await p.keyboard.press('ArrowLeft'); await expectFocus(p, 'cell:0', 'ArrowLeft');
  await p.keyboard.press('ArrowLeft'); await expectFocus(p, 'cell:0', 'ArrowLeft on the first photo');
  await p.keyboard.press('ArrowDown');
  const below = await expectFocus(p, (f) => cellOf(f) > 1, 'ArrowDown');
  const y = (i) => p.locator(`.cell[data-index="${i}"]`).evaluate((c) => c.getBoundingClientRect().top);
  check(await y(cellOf(below)) > await y(0), `ArrowDown goes to the row below (${below})`);
  await p.keyboard.press('ArrowUp'); await expectFocus(p, 'cell:0', 'ArrowUp back to the same column');
});

await test('viewer: Space opens, the arrows step, Escape goes back to the last photo seen', async (p) => {
  await p.locator('.cell[data-index="0"]').focus();
  await p.keyboard.press(' ');
  check(await visible(p, '#lightbox'), 'Space on a photo opens the viewer');
  await p.keyboard.press('ArrowRight');
  await p.keyboard.press('ArrowRight');
  await p.keyboard.press('ArrowLeft');
  await p.keyboard.press('ArrowRight');
  await p.keyboard.press('Escape');
  check(!(await visible(p, '#lightbox')), 'Escape closes the viewer');
  await expectFocus(p, 'cell:2', 'after the viewer');
});

await test('selection bar: one Tab stop, the arrows, Home and End go from tool to tool', async (p) => {
  await startSelecting(p, 0, 0);
  const tools = await barTools(p);
  check(tools[0] === '#sel-done' && tools.includes('#sel-tag') && tools.includes('#sel-move'), `tools in the bar: ${tools}`);
  await p.keyboard.press('b'); await expectFocus(p, tools[0], 'B from a photo');
  const stops = await p.evaluate(() => [...document.querySelectorAll('#selbar button')].filter((b) => b.tabIndex === 0).length);
  check(stops === 1, `${stops} buttons of the bar are Tab stops, expected 1`);
  for (let i = 1; i < tools.length; i++) {
    await p.keyboard.press('ArrowRight'); await expectFocus(p, tools[i], `ArrowRight to tool ${i}`);
  }
  await p.keyboard.press('ArrowRight'); await expectFocus(p, tools[0], 'ArrowRight on the last tool wraps');
  await p.keyboard.press('ArrowLeft'); await expectFocus(p, tools[tools.length - 1], 'ArrowLeft on the first tool wraps');
  await p.keyboard.press('Home'); await expectFocus(p, tools[0], 'Home');
  await p.keyboard.press('End'); await expectFocus(p, tools[tools.length - 1], 'End');
  await p.keyboard.press('ArrowLeft'); await expectFocus(p, tools[tools.length - 2], 'ArrowLeft');
});

// Tab out after the bar and Shift+Tab back: the other way round lands in the
// grid, which renders more rows as the focus scrolls it, so Tab from there
// goes on into those.
await test('selection bar: Tab leaves it, Shift+Tab comes back to the last tool used', async (p) => {
  await startSelecting(p, 0, 0);
  await p.keyboard.press('b');
  await p.keyboard.press('ArrowRight');
  await p.keyboard.press('ArrowRight');
  await expectFocus(p, '#sel-tag', 'two tools on');
  const inBar = () => p.evaluate(() => document.getElementById('selbar').contains(document.activeElement));
  await p.keyboard.press('Tab');
  check(!(await inBar()), `Tab leaves the bar (focus on ${await focused(p)})`);
  await p.keyboard.press('Shift+Tab'); await expectFocus(p, '#sel-tag', 'Shift+Tab back into the bar');
  await p.keyboard.press('Shift+Tab');
  check(!(await inBar()), `Shift+Tab leaves the bar (focus on ${await focused(p)})`);
});

await test('B goes from the photos to the bar and back, and is a letter in a field', async (p) => {
  await startSelecting(p, 0, 3);
  await p.keyboard.press('b'); await expectFocus(p, '#sel-done', 'B from a photo');
  await p.keyboard.press('ArrowRight');
  await p.keyboard.press('ArrowRight');
  await expectFocus(p, '#sel-tag', 'two tools on');
  await p.keyboard.press('B'); await expectFocus(p, 'cell:3', 'B (with Shift) from the bar back to the photo');
  await p.keyboard.press('b'); await expectFocus(p, '#sel-tag', 'B again: the bar keeps the last tool used');
  await p.locator('#search').focus();
  await p.keyboard.press('b');
  await expectFocus(p, '#search', 'B in the search field');
  check(await p.locator('#search').inputValue() === 'b', 'B in the search field is typed');
});

await test('dialog: the focus goes in, Tab stays in, Escape gives it back to the tool', async (p) => {
  await startSelecting(p, 0, 0);
  await p.keyboard.press('b');
  const tools = await barTools(p);
  while (await focused(p) !== '#sel-move') {
    check(tools.includes(await focused(p)), 'the arrows stay in the bar');
    await p.keyboard.press('ArrowRight');
  }
  await p.keyboard.press('Enter');
  check(await visible(p, '#modal'), 'Enter on Move opens its dialog');
  const inDialog = () => p.evaluate(() => document.querySelector('#modal .dialog').contains(document.activeElement));
  check(await inDialog(), `the focus goes into the dialog (it is on ${await focused(p)})`);
  const first = await focused(p);
  const controls = await p.evaluate(() => [...document.querySelectorAll('#modal .dialog button, #modal .dialog input')].filter((e) => e.getClientRects().length).length);
  const seen = new Set([first]);
  for (let i = 0; i < controls + 2; i++) {
    await p.keyboard.press('Tab');
    check(await inDialog(), `Tab ${i + 1} left the dialog (focus on ${await focused(p)})`);
    seen.add(await p.evaluate(() => { const a = document.activeElement; return a.tagName + ':' + (a.type || '') + ':' + a.textContent; }));
  }
  check(seen.size >= 3, `Tab goes round the dialog's controls (${seen.size} seen)`);
  await p.locator('#modal .dialog input').first().focus();
  await p.keyboard.press('Shift+Tab');
  check(await inDialog(), 'Shift+Tab on the first control stays in the dialog');
  const last = await p.evaluate(() => { const b = [...document.querySelectorAll('#modal .dialog button')].filter((e) => e.getClientRects().length); return b[b.length - 1] === document.activeElement; });
  check(last, 'Shift+Tab on the first control goes to the last one');
  await p.keyboard.press('Escape');
  check(!(await visible(p, '#modal')), 'Escape closes the dialog');
  await expectFocus(p, '#sel-move', 'after the dialog');
  check(await visible(p, '#selbar'), 'the selection is still on');
});

await test('dialog: Cancel from the keyboard also gives the focus back', async (p) => {
  await startSelecting(p, 0, 0);
  await p.keyboard.press('b');
  await p.keyboard.press('ArrowRight');
  await p.keyboard.press('ArrowRight');
  await expectFocus(p, '#sel-tag', 'two tools on');
  await p.keyboard.press('Enter');
  check(await visible(p, '#modal'), 'Enter on Add tag opens its dialog');
  const cancel = p.locator('#modal-actions button').first();
  await cancel.focus();
  await p.keyboard.press('Enter');
  check(!(await visible(p, '#modal')), 'Enter on Cancel closes the dialog');
  await expectFocus(p, '#sel-tag', 'after the dialog');
});

await test('a dimmed tool (nothing selected) keeps its place and does nothing', async (p) => {
  await startSelecting(p, null, 0);
  await p.keyboard.press('b');
  while (await focused(p) !== '#sel-move') await p.keyboard.press('ArrowRight');
  check(await p.locator('#sel-move').getAttribute('aria-disabled') === 'true', 'Move is dimmed with nothing selected');
  await p.keyboard.press('Enter');
  check(!(await visible(p, '#modal')), 'Enter on a dimmed tool opens nothing');
  await expectFocus(p, '#sel-move', 'on the dimmed tool');
});

await test('Escape in the bar ends the selection, the focus goes back to the select button', async (p) => {
  await startSelecting(p, 0, 0);
  await p.keyboard.press('b');
  await p.keyboard.press('ArrowRight');
  await p.keyboard.press('Escape');
  check(!(await visible(p, '#selbar')), 'Escape hides the selection bar');
  check(await p.locator('.cell.sel').count() === 0, 'Escape clears the selection');
  await expectFocus(p, '#select', 'after the selection');
});

await test('Enter on Done ends the selection, the focus goes back to the select button', async (p) => {
  await startSelecting(p, 0, 0);
  await p.keyboard.press('b');
  await expectFocus(p, '#sel-done', 'B from a photo');
  await p.keyboard.press('Enter');
  check(!(await visible(p, '#selbar')), 'Done hides the selection bar');
  await expectFocus(p, '#select', 'after the selection');
});

// Mouse drags while selecting: press on photo `from`, move over the photos in
// `over` (indices, or a function that moves on its own), let go.
const centre = async (p, i) => {
  const b = await p.locator(`.cell[data-index="${i}"]`).boundingBox();
  return [b.x + b.width / 2, b.y + b.height / 2];
};
async function dragAcross(p, from, over) {
  await p.mouse.move(...await centre(p, from));
  await p.mouse.down();
  for (const i of over) await p.mouse.move(...await centre(p, i), { steps: 4 });
  await p.mouse.up();
}
const selectedCells = (p) => p.evaluate(() => [...document.querySelectorAll('.cell.sel')]
  .map((c) => parseInt(c.dataset.index, 10)).sort((a, b) => a - b).join(','));

await test('dragging across photos selects them, back again leaves them as they were', async (p) => {
  await p.locator('#select').click();
  await dragAcross(p, 0, [3, 6, 4]);
  check(!(await visible(p, '#lightbox')), 'a drag opens no photo');
  check(await selectedCells(p) === '0,1,2,3,4', `0 to 6, then back to 4, selects 0 to 4 (got ${await selectedCells(p)})`);
  check((await p.locator('#sel-count').textContent()).includes('5'), 'the bar counts the photos dragged across');
  // Starting on a selected photo deselects; photos outside the drag keep theirs.
  await dragAcross(p, 2, [3]);
  check(await selectedCells(p) === '0,1,4', `a drag from a selected photo deselects (got ${await selectedCells(p)})`);
  // A drag backwards, from an unselected photo, over a mix: all of them selected.
  await dragAcross(p, 3, [1, 0]);
  check(await selectedCells(p) === '0,1,2,3,4', `a drag backwards selects (got ${await selectedCells(p)})`);
  // A plain click still toggles one photo after a drag.
  await p.locator('.cell[data-index="8"]').click();
  check(await selectedCells(p) === '0,1,2,3,4,8', `a click after a drag toggles one photo (got ${await selectedCells(p)})`);
});

await test('a drag selects only what its rectangle covers, and gives back what it leaves', async (p) => {
  await p.locator('#select').click();
  // Two rows of one month, columns 1 and 2 of each: four photos, not the rows between.
  const at = await p.evaluate(() => {
    const rows = state.rows;
    for (let i = 0; i + 1 < rows.length; i++) {
      if (rows[i].type === 'r' && rows[i + 1].type === 'r' && rows[i].end - rows[i].start >= 4 && rows[i + 1].end - rows[i + 1].start >= 4) return [rows[i].start, rows[i + 1].start];
    }
    return null;
  });
  check(at, 'the fixtures have two full rows in a month');
  const [r0, r1] = at;
  const rect = (from, to) => dragAcross(p, from, [to]);
  await rect(r0 + 1, r1 + 2);
  check(await selectedCells(p) === [r0 + 1, r0 + 2, r1 + 1, r1 + 2].join(','), `a rectangle of 2 by 2 selects 4 photos (got ${await selectedCells(p)})`);
  check(await p.locator('.marquee').count() === 0, 'the rectangle is gone after the drag');
  // Starting on a selected photo deselects what the rectangle covers, nothing else.
  await p.locator(`.cell[data-index="${r0}"]`).click();
  await rect(r0 + 1, r1 + 1);
  check(await selectedCells(p) === [r0, r0 + 2, r1 + 2].join(','), `a drag from a selected photo deselects its rectangle only (got ${await selectedCells(p)})`);
  // Moving the pointer back shrinks the rectangle: what it left is as it was.
  await p.mouse.move(...await centre(p, r0 + 3));
  await p.mouse.down();
  await p.mouse.move(...await centre(p, r1 + 3), { steps: 4 });
  await p.mouse.move(...await centre(p, r0 + 3), { steps: 4 });
  await p.mouse.up();
  check(await selectedCells(p) === [r0, r0 + 2, r0 + 3, r1 + 2].join(','), `a drag back leaves only the first row's photo added (got ${await selectedCells(p)})`);
});

await test('a drag to the bottom edge scrolls along and selects what comes into view', async (p) => {
  await p.locator('#select').click();
  const r = await p.locator('#scroller').boundingBox();
  await p.mouse.move(...await centre(p, 0));
  await p.mouse.down();
  const x = r.x + r.width / 3;
  await p.mouse.move(x, r.y + r.height - 5, { steps: 10 });
  await p.waitForTimeout(800);
  await p.mouse.up();
  check(await p.locator('#scroller').evaluate((s) => s.scrollTop) > 0, 'the timeline scrolled');
  // The rectangle spans the columns from the first photo to the pointer, in every row down to the last one reached.
  const bad = await p.evaluate((x) => {
    const sizer = document.getElementById('sizer').getBoundingClientRect();
    const last = Math.min(state.cols - 1, Math.floor((x - sizer.left) / state.cell));
    const rows = state.rows.filter((row) => row.type === 'r');
    const sel = (i) => !!state.selected[state.data.ids[i]];
    let first = -1, end = -1;
    rows.forEach((row, k) => { if (sel(row.start)) { if (first < 0) first = k; end = k; } });
    if (first !== 0 || end < 5) return `rows ${first} to ${end}`;
    for (let k = first; k <= end; k++) {
      const row = rows[k];
      for (let i = row.start; i < row.end; i++) {
        if (sel(i) !== (i - row.start <= last)) return `photo ${i} (column ${i - row.start}) is ${sel(i) ? '' : 'not '}selected, columns 0 to ${last} should be`;
      }
    }
    return '';
  }, x);
  check(!bad, `the photos that came into view are selected, only in the columns covered (${bad})`);
});

await test('without the selection a drag selects nothing', async (p) => {
  await dragAcross(p, 0, [3]);
  check(!(await visible(p, '#selbar')), 'no selection started');
  check(await p.locator('.cell.sel').count() === 0, 'nothing selected');
});

await browser.close();
if (failures) { console.log(`${failures} failed`); process.exit(1); }
console.log('all passed');
