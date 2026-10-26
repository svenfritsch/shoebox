// shoebox web UI: virtualised timeline grid, folder tree, search, lightbox,
// and the changes: move, trash, import, folder rename, duplicates, tags,
// faces and people.
// Plain ES2017 without a build step, so it runs on older iPads too.
'use strict';

var $ = function (id) { return document.getElementById(id); };
var el = function (tag, cls, text) {
  var e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
};

// Stop the pictures under `node` that are still loading, before it leaves the
// page. A picture taken off the page keeps its request otherwise, and the
// browser sends only six at a time to this server: the pictures now on the
// screen would wait behind ones nobody sees any more.
function dropPending(node) {
  var imgs = node.querySelectorAll('img');
  for (var i = 0; i < imgs.length; i++) {
    if (imgs[i].complete) continue;
    imgs[i].onload = imgs[i].onerror = null;
    imgs[i].removeAttribute('src');
  }
}

// Empty a page (or the viewer's stage) for what comes next, `dropPending` first.
function clearNode(node) {
  dropPending(node);
  node.textContent = '';
}

// A long list of tiles (all faces of a group) goes onto the page a part at a
// time: GROW_STEP now, the next part when the end of the list comes near the
// screen. However long the list, the page holds only what was scrolled to,
// and only the pictures near the screen are asked for. Drawn again (the same
// grid, tiles that `tile` keeps), it shows at least as many as before, so the
// page does not jump, and tiles already in their place stay there (and keep
// the focus). `tail` (a "+n" button) goes last, once everything is in.
var GROW_STEP = 120;

function growGrid(grid, items, tile, tail) {
  if (grid.grower) grid.grower.disconnect();
  grid.grower = null;
  var shown = window.IntersectionObserver ? Math.min(items.length, Math.max(GROW_STEP, grid.grown || 0)) : items.length;
  var nodes = [];
  for (var k = 0; k < shown; k++) nodes.push(tile(items[k]));
  var end = shown < items.length ? el('span', 'grow-end') : tail;
  if (end) nodes.push(end);
  nodes.forEach(function (node, k) { if (grid.children[k] !== node) grid.insertBefore(node, grid.children[k] || null); });
  while (grid.children.length > nodes.length) grid.lastElementChild.remove();
  grid.grown = shown;
  if (shown >= items.length) return;
  grid.grower = new IntersectionObserver(function (entries) {
    if (!entries.some(function (e) { return e.isIntersecting; })) return;
    var frag = document.createDocumentFragment();
    for (var stop = Math.min(items.length, shown + GROW_STEP); shown < stop; shown++) frag.appendChild(tile(items[shown]));
    grid.insertBefore(frag, end);
    grid.grown = shown;
    if (shown < items.length) {
      // Still near the screen (a short part): observing again reports it again.
      grid.grower.unobserve(end);
      grid.grower.observe(end);
      return;
    }
    grid.grower.disconnect();
    grid.grower = null;
    end.remove();
    if (tail) grid.appendChild(tail);
  }, { root: $('scroller'), rootMargin: '1500px 0px' });
  grid.grower.observe(end);
}

var HEADER_H = 44;
var OVERSCAN = 600; // px above and below the viewport that stay rendered
var KIND_BADGE = { v: '▶', h: '', j: '', p: '' };

var state = {
  filter: { folder: null, tags: [], people: [], manual: false, q: '', view: null, id: null, tab: null },
  selecting: false,
  selected: {},      // id -> true
  anchor: null,      // index of the last photo clicked while selecting (Shift-click ranges)
  data: null,        // timeline columns from /api/timeline
  favs: {},          // still id -> true, for the loaded timeline (empty on pages that open before it)
  hids: {},          // still id -> true: hidden from the timeline, among those shown (the Hidden chip, a hidden folder)
  live: {},          // still id -> video id
  snips: {},         // id -> the line a text search found in it (shown over its thumbnail)
  textLines: 0,      // lines of recognized text the search can find (0: a text search finds nothing)
  folders: [],
  folderById: {},
  rows: [],          // layout rows: {top, h, type: 'h'|'r', ...}
  rendered: {},      // row index -> element
  cols: 1,
  cell: 100,
  open: -1,          // index of the item in the lightbox
  indexVersion: null,
  loadSeq: 0,
  loadCtrl: null,    // stops the timeline request a newer one replaces
  reveal: null,      // label of "show in the file manager"; set only on the shoebox computer
};

// ------------------------------------------------------------------ api

// Every library has an id and all its routes live under /api/lib/<id>
// (file ids are per database). Set in the start code below, from
// /api/libraries, before the first library request.
var LIBAPI = null;

// The common timeline of all drives ("scope all"): an item's id is
// `drive * SPAN + id in that drive`, the drive being its position in the
// list the server sent with the timeline. fileBase() turns it back into the
// route of that drive's file.
var SPAN = 1099511627776; // 2^40, as on the server
function isAll() { return !!(typeof drives !== 'undefined' && drives.scope === 'all'); }
function fileBase(id) {
  if (!isAll()) return LIBAPI + '/files/' + id;
  var lib = drives.allLibs[Math.floor(id / SPAN)];
  return '/api/lib/' + lib.id + '/files/' + (id % SPAN);
}

function api(path, opts) {
  return fetch(path, Object.assign({ credentials: 'same-origin' }, opts)).then(function (r) {
    if (r.status === 401) { showLogin(); throw new Error('login required'); }
    if (!r.ok) {
      return r.json().catch(function () { return {}; }).then(function (body) {
        var e = new Error(body.error || path + ': HTTP ' + r.status);
        e.status = r.status;
        throw e;
      });
    }
    return r.json();
  });
}

// Every change is a POST with the X-Shoebox header (the server refuses it
// otherwise, so other web pages cannot make changes).
function post(path, body) {
  return api(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-Shoebox': '1' },
    body: JSON.stringify(body || {}),
  });
}

// Arrays repeat their key (`tag=1&tag=2`).
function query(params) {
  var parts = [];
  Object.keys(params).forEach(function (k) {
    [].concat(params[k]).forEach(function (v) {
      if (v !== null && v !== undefined && v !== '') parts.push(k + '=' + encodeURIComponent(v));
    });
  });
  return parts.length ? '?' + parts.join('&') : '';
}

// ------------------------------------------------------------------ login

function showLogin() {
  $('login').hidden = false;
  $('pin').focus();
}

$('login-form').addEventListener('submit', function (ev) {
  ev.preventDefault();
  $('login-error').textContent = '';
  fetch('/api/login', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json', 'X-Shoebox': '1' },
    body: JSON.stringify({ pin: $('pin').value }),
  }).then(function (r) {
    if (r.ok) { location.reload(); return; }
    return r.json().then(function (body) { $('login-error').textContent = body.error || tr('login.wrong'); });
  }).catch(function (e) { $('login-error').textContent = String(e); });
});

// ------------------------------------------------------------------ filters (in the URL hash)

var TYPES = ['photo', 'video', 'live', 'screenshot'];
var COMPARES = ['backup', 'original', 'different'];
var VIEWS = ['duplicates', 'trash', 'faces', 'people', 'unnamed', 'person', 'drives', 'settings', 'pets', 'locations', 'text'];
// Pet search terms (`pet=` in the URL and the API): a species or any pet.
var PET_TERMS = {
  cat: { get label() { return tr('pet.term.cat'); } },
  dog: { get label() { return tr('pet.term.dog'); } },
  pet: { get label() { return tr('pet.term.pet'); } },
};

// Line icons of the text search: the document scanner for words read in
// pictures, the page for file names (drawn with the text colour).
var ICONS = {
  scan: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 8V5a2 2 0 0 1 2-2h3M16 3h3a2 2 0 0 1 2 2v3M21 16v3a2 2 0 0 1-2 2h-3M8 21H5a2 2 0 0 1-2-2v-3"/><rect x="7" y="6.5" width="10" height="11" rx="2"/><path d="M9.5 10h5M9.5 12.5h5M9.5 15h5"/></svg>',
  draft: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6 3h8l5 5v11a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"/><path d="M14 3v5h5"/></svg>',
  copy: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/></svg>',
  // A head with shoulders: one person.
  person: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="8" r="3.6"/><path d="M4.5 20c.5-4 3.5-6.2 7.5-6.2s7 2.2 7.5 6.2"/></svg>',
  // A sitting cat with its tail up.
  cat: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M8.5 4.5l1.3 1.8h3.4l1.3-1.8v4.2a3.3 3.3 0 0 1-6 0z"/><path d="M8.7 11.5C6.5 13 5.5 15.5 5.5 18c0 1.2.8 2 2 2h6.5c2.6 0 4.5-1.6 4.5-3.5V8.5"/><path d="M18.5 8.5a1.5 1.5 0 0 1 3 0"/></svg>',
  // A bone: a dog.
  dog: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path transform="rotate(-45 12 12) translate(12 12) scale(1.15) translate(-12 -12)" d="M7.19 10H16.81A2.4 2.4 0 1 1 20.39 10Q19.6 12 20.39 14A2.4 2.4 0 1 1 16.81 14H7.19A2.4 2.4 0 1 1 3.61 14Q4.4 12 3.61 10A2.4 2.4 0 1 1 7.19 10z"/></svg>',
  // A paw print: a pet in general.
  paw: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="5.5" cy="11.5" r="1.7" fill="currentColor"/><circle cx="9.3" cy="7" r="1.7" fill="currentColor"/><circle cx="14.7" cy="7" r="1.7" fill="currentColor"/><circle cx="18.5" cy="11.5" r="1.7" fill="currentColor"/><path d="M8.3 17.5c0-2.2 1.7-4 3.7-4s3.7 1.8 3.7 4c0 1.5-1 2.2-2.2 2.2-.6 0-1-.3-1.5-.3s-.9.3-1.5.3c-1.2 0-2.2-.7-2.2-2.2z"/></svg>',
  // The map marker of the selection bar, without its plus.
  place: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 21C9.5 18.2 6 14.8 6 10.8a6 6 0 0 1 12 0c0 4-3.5 7.4-6 10.2z"/><circle cx="12" cy="10.8" r="1.4" fill="currentColor"/></svg>',
  // Four filled squares in a grid: the photos of a place in the timeline.
  tiles: '<svg viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round" aria-hidden="true"><rect x="4" y="4" width="6.5" height="6.5" rx="1.4"/><rect x="13.5" y="4" width="6.5" height="6.5" rx="1.4"/><rect x="4" y="13.5" width="6.5" height="6.5" rx="1.4"/><rect x="13.5" y="13.5" width="6.5" height="6.5" rx="1.4"/></svg>',
  // A flat drive with its light: the disk a folder is on (the guide's symbol).
  drive: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="7" width="18" height="10" rx="2"/><circle cx="17" cy="12" r="1"/><path d="M6 12h6"/></svg>',
  // A pencil: change a name.
  pencil: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M5 19l1-4L16.5 4.5a2.1 2.1 0 0 1 3 3L9 18z"/><path d="M14.5 6.5l3 3"/></svg>',
  // The calendar page of the selection bar (a date of your own).
  date: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 8.5A2.5 2.5 0 0 1 6.5 6h11A2.5 2.5 0 0 1 20 8.5v9a2.5 2.5 0 0 1-2.5 2.5h-11A2.5 2.5 0 0 1 4 17.5z"/><path d="M8.5 3.5v5M15.5 3.5v5" stroke-width="2.2"/><path d="M7.5 14.5c1.5-2.3 3-2.3 4.5 0s3 2.3 4.5 0"/></svg>',
  // Three pawns, the middle one taller: a group of people.
  group: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="5.5" cy="9.5" r="2"/><path d="M2.5 20v-1.5C2.5 16 3.8 14.5 5.5 14.5S8.5 16 8.5 18.5V20z"/><circle cx="18.5" cy="9.5" r="2"/><path d="M15.5 20v-1.5c0-2.5 1.3-4 3-4s3 1.5 3 4V20z"/><circle cx="12" cy="6.5" r="2.4"/><path d="M8.5 20v-2.5c0-3 1.5-5 3.5-5s3.5 2 3.5 5V20z"/></svg>',
};
// The icon of a species in chips and suggestions: the sitting cat, the bone for a dog, else the paw.
function speciesIcon(species) { return icon(species === 'cat' ? 'cat' : species === 'dog' ? 'dog' : 'paw'); }
function icon(name, cls) {
  var s = document.createElement('span');
  s.className = 'ic' + (cls ? ' ' + cls : '');
  s.innerHTML = ICONS[name];
  return s;
}

// The search box's two shortcuts: `text:` (words read in pictures) and
// `name:` (file names only).
var PREFIX = /^\s*(text|name):\s*/i;
function prefixOf(v) {
  var m = PREFIX.exec(v);
  return m ? { kind: m[1].toLowerCase(), term: v.slice(m[0].length).trim() } : null;
}

function readHash() {
  var f = { folder: null, tags: [], people: [], groups: [], pets: [], texts: [], names: [], types: [], fav: false, hidden: false, manual: false, place: null, dates: [], nodate: false, compare: '', q: '', view: null, id: null, tab: null };
  location.hash.replace(/^#/, '').split('&').forEach(function (kv) {
    var i = kv.indexOf('=');
    if (i < 0) return;
    var k = kv.slice(0, i), v = decodeURIComponent(kv.slice(i + 1));
    if (k === 'folder') f.folder = parseInt(v, 10) || null;
    // Over all drives tags and people are named: ids differ from drive to drive.
    var all = isAll();
    if (k === 'tag') { var t = all ? v : parseInt(v, 10); if (t && f.tags.indexOf(t) < 0) f.tags.push(t); }
    if (k === 'person') { var pp = all ? v : parseInt(v, 10); if (pp && f.people.indexOf(pp) < 0) f.people.push(pp); }
    // A group: the photos of any of its people.
    if (k === 'group') { var gg = all ? v : parseInt(v, 10); if (gg && f.groups.indexOf(gg) < 0) f.groups.push(gg); }
    if (k === 'type' && TYPES.indexOf(v) >= 0 && f.types.indexOf(v) < 0) f.types.push(v);
    // A kind of pet (all cats, all dogs, any pet): the same on every drive.
    if (k === 'pet' && PET_TERMS[v] && f.pets.indexOf(v) < 0) f.pets.push(v);
    if (k === 'text' && v.trim() && f.texts.indexOf(v) < 0) f.texts.push(v);
    if (k === 'name' && v.trim() && f.names.indexOf(v) < 0) f.names.push(v);
    if (k === 'fav') f.fav = v === '1';
    if (k === 'hidden') f.hidden = v === '1';
    if (k === 'manual') f.manual = v === '1';
    if (k === 'place') f.place = parseInt(v, 10) || null;
    // Photos dated inside a year, month or day (several: all must match), and those that need a date.
    if (k === 'date' && /^\d{4}(-\d{2}(-\d{2})?)?$/.test(v) && f.dates.indexOf(v) < 0) f.dates.push(v);
    if (k === 'nodate') f.nodate = v === '1';
    // Only on the backup / only on the original / different on the backup (Manage drives).
    if (k === 'compare' && COMPARES.indexOf(v) >= 0) f.compare = v;
    if (k === 'q') f.q = v;
    if (k === 'view' && VIEWS.indexOf(v) >= 0) f.view = v;
    if (k === 'id') f.id = parseInt(v, 10) || null;
    if (k === 'tab') f.tab = v;
  });
  // Over all drives only these pages exist (the others belong to one drive).
  if (isAll() && f.view && f.view !== 'duplicates' && f.view !== 'drives') f.view = null;
  if (isAll()) f.place = null; // places belong to one drive
  if (isAll() || !f.tags.length) f.manual = false; // "manual tags only" belongs to the tags of one drive
  return f;
}

// A filter without a view shows the grid. Its terms all have to match: a
// folder, any number of tags and people (`tag`, `person` repeated in the
// URL) and free text.
function setFilter(f) {
  var h = query({ view: f.view, id: f.id, tab: f.tab, folder: f.folder, tag: f.tags || [], person: f.people || [], group: f.groups || [], pet: f.pets || [], text: f.texts || [], name: f.names || [], type: f.types || [], fav: f.fav ? 1 : null, hidden: f.hidden ? 1 : null, manual: f.manual && (f.tags || []).length ? 1 : null, place: f.place, date: f.dates || [], nodate: f.nodate ? 1 : null, compare: f.compare || null, q: f.q }).replace(/^\?/, '');
  if (h === location.hash.replace(/^#/, '')) { applyFilter(); return; }
  location.hash = h; // triggers hashchange -> applyFilter
}

// The current filter with some terms changed.
function withFilter(changes) {
  var f = state.filter;
  // A place picked on the Locations page goes with the search to the timeline.
  var place = f.view === 'locations' ? f.id : f.place;
  return Object.assign({ folder: f.folder, tags: f.tags.slice(), manual: f.manual && !('tags' in changes), people: f.people.slice(), groups: f.groups.slice(), pets: f.pets.slice(), texts: f.texts.slice(), names: f.names.slice(), types: f.types.slice(), fav: f.fav, hidden: f.hidden, place: place, dates: f.dates.slice(), nodate: f.nodate, compare: f.compare, q: f.q }, changes);
}

function showView(view, id, tab) { setFilter({ view: view, id: id, tab: tab, folder: null, tags: [], people: [], q: '' }); }

// The hash the page is showing; navigating away from a selection asks first.
var shownHash = location.hash, revertingHash = false, shownView = null;

function applyFilter() {
  if (revertingHash) { revertingHash = false; return; }
  var moved = location.hash !== shownHash;
  var wanted = location.hash;
  // Another list while photos are selected: put the old page back and ask. Typing
  // in the search box is not asked about (it would ask on every key).
  if (moved && state.selecting && selectedIds().length && readHash().q === state.filter.q) {
    revertingHash = true;
    location.hash = shownHash;
    var n = trn('count.photos', selectedIds().length);
    openModal(tr('sel.leave_title'), tr('sel.leave_text', { count: n }), [
      { label: tr('sel.leave_stay'), cls: 'quiet', focus: true },
      { label: tr('sel.leave_go'), onclick: function () { endSelection(); location.hash = wanted; } },
    ]);
    return;
  }
  shownHash = location.hash;
  state.filter = readHash();
  $('search').value = state.filter.q;
  renderChips();
  markTypes();
  // Still typing: offer what narrows the new search down (not again when only
  // the typed text became the search: the list for it is there or on its way).
  if (document.activeElement === $('search') && !sugg.picked && suggestKey($('search').value) !== sugg.key) suggest($('search').value);
  markActiveFolder();
  closeSidebarOnPhone();
  var view = state.filter.view;
  if (shownView === 'unnamed' && view !== 'unnamed') leftUnnamed();
  shownView = view;
  $('page').hidden = !view;
  $('sizer').hidden = !!view;
  $('current-month').textContent = view && VIEW_LABELS[view] ? tr(VIEW_LABELS[view]) : '';
  var f = state.filter;
  $('all').classList.toggle('active', !view && !f.folder && !f.tags.length && !f.people.length && !f.groups.length && !f.pets.length && !f.texts.length && !f.names.length && !f.place && !f.fav && !f.hidden && !f.dates.length && !f.nodate && !f.compare && !f.q);
  $('nav-nodate').classList.toggle('active', !view && f.nodate);
  if (isAll()) $('nav-nodate').hidden = true;
  $('nav-dups').classList.toggle('active', view === 'duplicates');
  $('nav-trash').classList.toggle('active', view === 'trash');
  $('nav-settings').classList.toggle('active', view === 'settings' || view === 'faces' || view === 'pets' || view === 'text');
  $('nav-drives').classList.toggle('active', view === 'drives');
  markFacesSection();
  updateSections();
  document.body.classList.toggle('geo-view', view === 'locations');
  if (view !== 'locations') geoLeave();
  renderLocationsSection();
  // Another tag, folder, search or page is another list: the selection (ids and
  // bar) belongs to the one it was made in.
  if (moved) endSelection();
  if (view) {
    $('empty').hidden = true;
    $('filters').hidden = true;
    $('scroller').scrollTop = 0;
    loadView(view);
    return;
  }
  loadTimeline(true);
}

window.addEventListener('hashchange', applyFilter);

// The filter as chips (all must match), with ✕ on each and "+" to add a term.
function renderChips() {
  var box = $('filters');
  box.textContent = '';
  var f = state.filter;
  var add = function (label, clear, ic, title) {
    // `ic` is an icon element, or the name of one of our own icons (text and file name chips).
    var named = typeof ic === 'string';
    var c = el('span', 'chip' + (named ? ' ' + (ic === 'scan' ? 'text' : 'name') : ''), '');
    if (ic) c.appendChild(named ? icon(ic) : ic);
    c.appendChild(document.createTextNode(label));
    if (title) setTip(c, title);
    var x = el('button', '', '✕');
    setTip(x, tr('chip.remove'));
    x.onclick = clear;
    c.appendChild(x);
    box.appendChild(c);
  };
  if (f.folder && state.folderById[f.folder]) {
    var folder = state.folderById[f.folder];
    add(folder.path || folder.name, function () { setFilter(withFilter({ folder: null })); }, folderIcon());
    // The eye: hide the folder (and what is below it) from the timeline, or show it again. A flag
    // in the index, so it stays in a library that is only read.
    if (folder.path) {
      var eye = el('button', 'eye' + (folder.hidden ? ' on' : ''));
      eye.innerHTML = folder.hidden ? SHOW_ICON : HIDE_ICON;
      var above = folder.hidden && !folder.hidden_own;
      setTip(eye, tr(above ? 'hidden.folder_by_parent' : folder.hidden ? 'hidden.folder_show' : 'hidden.folder_hide'));
      eye.setAttribute('aria-label', eye.getAttribute('data-tip'));
      eye.disabled = above;
      eye.onclick = function () { setFolderHidden(folder, !folder.hidden); };
      box.lastChild.classList.add('acts');
      box.lastChild.insertBefore(eye, box.lastChild.lastChild);
    }
    if (folder.path && !readOnly) {
      var edit = el('button', 'edit');
      edit.appendChild(icon('pencil'));
      setTip(edit, tr('chip.rename_folder'));
      edit.onclick = function () { renameFolder(folder); };
      box.lastChild.insertBefore(edit, box.lastChild.lastChild);
    }
  }
  f.tags.forEach(function (id) {
    add('# ' + (state.tagNames[id] || id) + (f.manual ? ' · ' + tr('side.manual_only_short') : ''), function () {
      setFilter(withFilter({ tags: f.tags.filter(function (t) { return t !== id; }) }));
    });
  });
  f.people.forEach(function (id) {
    var kind = state.personSpecies[id];
    add((state.personNames[id] || id), function () {
      setFilter(withFilter({ people: f.people.filter(function (p) { return p !== id; }) }));
    }, kind ? speciesIcon(kind) : icon('person'));
  });
  f.groups.forEach(function (id) {
    add(state.groupNames[id] || id, function () {
      setFilter(withFilter({ groups: f.groups.filter(function (g) { return g !== id; }) }));
    }, icon('group'), tr('chip.group_hint', { name: state.groupNames[id] || id }));
  });
  f.pets.forEach(function (species) {
    add(PET_TERMS[species].label, function () {
      setFilter(withFilter({ pets: f.pets.filter(function (p) { return p !== species; }) }));
    }, speciesIcon(species));
  });
  if (f.fav) add('♥ ' + tr('fav.label'), function () { setFilter(withFilter({ fav: false })); });
  if (f.hidden) {
    add(tr('hidden.label'), function () { setFilter(withFilter({ hidden: false })); }, icon('hide'), tr('hidden.filter_hint'));
    box.lastChild.classList.add('hid');
  }
  // The Type drop-down's choice as a chip too, like the heart's.
  if (f.types.length) add(f.types.length === 1 ? tr('types.' + f.types[0]) : f.types.map(typeLabel).join(', '), function () { setFilter(withFilter({ types: [] })); }, typeIcon(f.types));
  if (f.place) {
    var spot = placeOf(f.place);
    // The marker is the way to the map (no second button beside the name).
    var mark = icon('place', 'link');
    mark.setAttribute('role', 'button');
    mark.tabIndex = 0;
    mark.setAttribute('aria-label', tr('chip.show_on_map'));
    setTip(mark, tr('chip.show_on_map'));
    var toMap = function () { showView('locations', f.place); };
    mark.onclick = toMap;
    mark.onkeydown = function (ev) { if (ev.key === 'Enter' || ev.key === ' ') { ev.preventDefault(); toMap(); } };
    add(spot ? spot.name : '…', function () { setFilter(withFilter({ place: null })); }, mark);
  }
  f.names.forEach(function (term) {
    add(term, function () { setFilter(withFilter({ names: f.names.filter(function (n) { return n !== term; }) })); }, 'draft', tr('chip.name_hint', { term: term }));
  });
  f.texts.forEach(function (term) {
    add(term, function () { setFilter(withFilter({ texts: f.texts.filter(function (n) { return n !== term; }) })); }, 'scan', tr('chip.text_hint', { term: term }));
  });
  f.dates.forEach(function (term) {
    add(dateTermLabel(term), function () { setFilter(withFilter({ dates: f.dates.filter(function (t) { return t !== term; }) })); }, icon('date'));
    setTip(box.lastChild, tr('dates.chip_hint', { date: dateTermLabel(term) }));
  });
  if (f.nodate) add(dates.needs ? tr('side.needs_date_n', { n: dates.needs }) : tr('side.needs_date'), function () { setFilter(withFilter({ nodate: false })); }, icon('date'));
  if (f.compare) {
    add('⇄ ' + compareLabel(f.compare), function () { setFilter(withFilter({ compare: '' })); });
    box.lastChild.classList.add('cmp');
  }
  if (f.q) add('“' + f.q + '”', function () { setFilter(withFilter({ q: '' })); });
  var terms = box.children.length;
  if (terms) {
    var more = el('button', 'chip more', '+');
    setTip(more, tr('chip.add'));
    more.onclick = function () { $('search').value = ''; $('search').focus(); suggest(''); };
    box.appendChild(more);
  }
  // One click for the whole search; the ✕ on a chip drops just that term.
  if (terms >= 2) {
    var clear = el('button', 'chip clear', tr('chip.clear_all'));
    setTip(clear, tr('chip.clear_all_hint'));
    clear.onclick = function () { setFilter({ folder: null, tags: [], people: [], groups: [], pets: [], texts: [], names: [], types: [], fav: false, hidden: false, place: null, compare: '', q: '' }); };
    box.appendChild(clear);
  }
  // How many the search finds, at the right; filled in when the timeline is in.
  if (terms) {
    box.appendChild(el('span', 'filter-count'));
    showCount();
  }
  exportChip(box, f);
  if (f.nodate) box.appendChild(el('p', 'filter-note', tr('side.needs_date_note')));
  personHead(box, f);
  box.hidden = !box.firstChild;
}

// "48 photos · 3 videos" for the list on screen, the number in bold; with one
// type ticked in its own word ("356 Live Photos"), and "No photos" for none.
// Only once the timeline for this very search has arrived (state.countFor).
function showCount() {
  markExportChip();
  var box = document.querySelector('#filters .filter-count');
  if (!box) return;
  box.textContent = '';
  var d = state.data;
  if (!d || state.countFor !== location.hash) return;
  var types = state.filter.types;
  var noun = types.length === 1 ? { photo: 'photos', video: 'videos', live: 'live', screenshot: 'screenshots' }[types[0]] : 'photos';
  if (!d.count) { box.textContent = tr('filter.none.' + noun); return; }
  var videos = 0;
  for (var i = 0; i < d.count; i++) if (d.kinds[i] === 'v') videos++;
  var parts = types.length === 1 ? [trn('count.' + noun, d.count)]
    : [d.count - videos ? trn('count.photos', d.count - videos) : '', videos ? trn('count.videos', videos) : ''].filter(Boolean);
  parts.forEach(function (text, k) {
    if (k) box.appendChild(document.createTextNode(' · '));
    var num = text.match(/^(.*?)([\d.,\u00a0\u202f']+)(.*)$/);
    if (!num) { box.appendChild(document.createTextNode(text)); return; }
    box.appendChild(document.createTextNode(num[1]));
    box.appendChild(el('strong', '', num[2]));
    box.appendChild(document.createTextNode(num[3]));
  });
}
state.tagNames = {};
state.personNames = {};
state.personSpecies = {}; // id -> cat, dog or pet, for people who are pets
state.groupNames = {}; // id -> name of a group (its own name over all drives)

// ------------------------------------------------------------------ type filter

// A check box drop-down: Photos, Videos (stand-alone only), Live Photos and
// Screenshots; every item is exactly one of them (browse::MediaType).
// Any ticked type matches; it combines with the rest of the search.
var typeBoxes = Array.prototype.slice.call($('types-menu').querySelectorAll('input'));
function typeLabel(t) { return tr(t === 'live' ? 'types.live_short' : 'types.' + t); }

// The Type chip's icon: one per type, the four squares for several.
var TYPE_ICONS = {
  photo: lineIcon('<rect x="3" y="7" width="18" height="13" rx="2.5"/><path d="M8.5 7l1.4-2.6h4.2L15.5 7"/><circle cx="12" cy="13.5" r="3.3"/>'),
  video: lineIcon('<rect x="3" y="6.5" width="12" height="11" rx="2"/><path d="M15 10.5l5.5-3v9l-5.5-3z"/>'),
  live: lineIcon('<circle cx="12" cy="12" r="9"/><path d="M10 8.5v7l5.3-3.5z"/>'),
  screenshot: lineIcon('<path d="M4 9V5.5A1.5 1.5 0 0 1 5.5 4H9M15 4h3.5A1.5 1.5 0 0 1 20 5.5V9M20 15v3.5a1.5 1.5 0 0 1-1.5 1.5H15M9 20H5.5A1.5 1.5 0 0 1 4 18.5V15"/>'),
};
var TYPE_ICON_MANY = lineIcon('<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>');
function typeIcon(types) {
  var i = el('span', 'ficon');
  i.setAttribute('aria-hidden', 'true');
  i.innerHTML = types.length === 1 ? TYPE_ICONS[types[0]] : TYPE_ICON_MANY;
  return i;
}
// The same icons in the drop-down, after each check box.
typeBoxes.forEach(function (b) { b.parentNode.insertBefore(typeIcon([b.value]), b.nextSibling); });

function markTypes() {
  markFavFilter();
  var on = state.filter.types;
  typeBoxes.forEach(function (b) { b.checked = on.indexOf(b.value) >= 0; });
  var btn = $('types-btn');
  btn.textContent = (on.length ? on.map(typeLabel).join(', ') : tr('types.label'));
  btn.classList.toggle('on', on.length > 0);
  $('types').hidden = !!state.filter.view;
}

function setTypesOpen(open) {
  $('types-menu').hidden = !open;
  $('types-btn').setAttribute('aria-expanded', open ? 'true' : 'false');
}

$('types-btn').onclick = function () { setTypesOpen($('types-menu').hidden); };
typeBoxes.forEach(function (b) {
  b.onchange = function () {
    setFilter(withFilter({ types: typeBoxes.filter(function (x) { return x.checked; }).map(function (x) { return x.value; }) }));
  };
});
document.addEventListener('click', function (ev) { if (!$('types').contains(ev.target)) setTypesOpen(false); });
document.addEventListener('keydown', function (ev) { if (ev.key === 'Escape') setTypesOpen(false); });

// ------------------------------------------------------------------ favorites

// The heart: an own tag called "favorite" in the database (`tags.rs`), shown
// as a heart everywhere. The tag itself is left out of tag lists and the info
// panel. The button next to the type filter shows only favorites; typing
// "favorite" / "favorit" in the search box offers the same as a suggestion.
var FAV_TAG = 'favorite';
function isFavTag(name) { return String(name).trim().toLowerCase() === FAV_TAG; }
// What typing in a tag field means the heart (the server maps these to the tag too).
var FAV_TYPED = [FAV_TAG, 'favorites', 'favourite', 'favourites', 'favorit', 'favoriten'];
function meansFav(name) { return FAV_TYPED.indexOf(String(name).trim().toLowerCase()) >= 0; }
// Our own drawn icons (no icon set): a 24 grid, round 2 px lines in the colour
// of the text. The fill sits in the style so that rules like ".icon svg" do
// not fill the outline.
function lineIcon(paths, fill, sw) {
  return '<svg viewBox="0 0 24 24" width="1em" height="1em" aria-hidden="true" style="fill:' + (fill ? 'currentColor' : 'none') +
    '" stroke="currentColor" stroke-width="' + (sw || 2) + '" stroke-linecap="round" stroke-linejoin="round">' + paths + '</svg>';
}
var HEART_PATH = '<path stroke-width="2.2" d="M12 19.5C8 16.8 4 13.6 4 9.6 4 7.1 5.9 5.2 8.2 5.2c1.6 0 3 .9 3.8 2.3.8-1.4 2.2-2.3 3.8-2.3 2.3 0 4.2 1.9 4.2 4.4 0 4-4 7.2-8 9.9z"/>';
var HEART_ON = lineIcon(HEART_PATH, true);
var HEART_OFF = lineIcon(HEART_PATH);
// The tools under the faces of the info panel and the select button of the top
// bar: our own drawings (frame with an eye, checkbox, face with a plus).
var FRAME_ICON = lineIcon('<path d="M3 9V6.5A2.5 2.5 0 0 1 5.5 4H9M15 4h3.5A2.5 2.5 0 0 1 21 6.5V9M3 15v2.5A2.5 2.5 0 0 0 5.5 20H9"/><path d="M12.5 17c1.2-2 3.3-3 4.5-3s3.3 1 4.5 3c-1.2 2-3.3 3-4.5 3s-3.3-1-4.5-3z"/><circle cx="17" cy="17" r="1.1"/>');
var SELECT_ICON = lineIcon('<rect x="4" y="4" width="16" height="16" rx="3.5"/><path d="M8.5 12.2l2.5 2.5 4.8-5"/>');
var ADD_FACE_ICON = lineIcon('<circle cx="11" cy="13" r="8"/><circle cx="8.3" cy="11.5" r=".6"/><circle cx="13.7" cy="11.5" r=".6"/><path d="M7.4 15.6a4.2 4.2 0 0 0 7.2 0"/><path d="M19 2.5v5M16.5 5h5"/>');
// The icons of the face and pet actions (Unnamed cards, the singles bar, the photo's face panel).
var NAME_ICON = lineIcon('<circle cx="10" cy="8" r="3.4"/><path d="M3.5 20c.6-3.6 3.2-5.6 6.5-5.6 1 0 1.9.2 2.7.5"/><path d="M14.5 17.5l2.3 2.3 4.2-4.6"/>');
var IGNORE_ICON = lineIcon('<path d="M2.5 12C5 7.6 8.3 6 12 6s7 1.6 9.5 6c-2.5 4.4-5.8 6-9.5 6s-7-1.6-9.5-6z"/><circle cx="12" cy="12" r="2.6"/><path d="M4.5 20L19.5 4"/>');
// "Hide from timeline" (phase 20) is the icon of the face action "Ignore": the eye with a slash.
// The same eye without the slash is the way back (show in timeline).
var HIDE_ICON = IGNORE_ICON;
var SHOW_ICON = IGNORE_ICON.replace('<path d="M4.5 20L19.5 4"/>', '');
ICONS.hide = HIDE_ICON;
ICONS.show = SHOW_ICON;
var NOT_FACE_ICON = lineIcon('<circle cx="12" cy="12" r="8.5"/><circle cx="9.3" cy="10.5" r=".6"/><circle cx="14.7" cy="10.5" r=".6"/><path d="M8.5 14.8a4.2 4.2 0 0 0 7 0"/><path d="M4.5 19.5l15-15"/>');
var NOT_PET_ICON = lineIcon('<circle cx="5.5" cy="11.5" r="1.6" fill="currentColor" stroke-width="1"/><circle cx="9.3" cy="7" r="1.6" fill="currentColor" stroke-width="1"/><circle cx="14.7" cy="7" r="1.6" fill="currentColor" stroke-width="1"/><circle cx="18.5" cy="11.5" r="1.6" fill="currentColor" stroke-width="1"/><path stroke-width="1.8" d="M8.3 17.5c0-2.2 1.7-4 3.7-4s3.7 1.8 3.7 4c0 1.5-1 2.2-2.2 2.2-.6 0-1-.3-1.5-.3s-.9.3-1.5.3c-1.2 0-2.2-.7-2.2-2.2z"/><path d="M4.5 20.5l15-17"/>');
var CAT_ICON = lineIcon('<path d="M8.5 4.5l1.3 1.8h3.4l1.3-1.8v4.2a3.3 3.3 0 0 1-6 0z"/><path d="M8.7 11.5C6.5 13 5.5 15.5 5.5 18c0 1.2.8 2 2 2h6.5c2.6 0 4.5-1.6 4.5-3.5V8.5"/><path d="M18.5 8.5a1.5 1.5 0 0 1 3 0"/>', false, 1.8);
// A bone: the dog (the paw is the pet in general).
var DOG_ICON = lineIcon('<path transform="rotate(-45 12 12) translate(12 12) scale(1.15) translate(-12 -12)" d="M7.19 10H16.81A2.4 2.4 0 1 1 20.39 10Q19.6 12 20.39 14A2.4 2.4 0 1 1 16.81 14H7.19A2.4 2.4 0 1 1 3.61 14Q4.4 12 3.61 10A2.4 2.4 0 1 1 7.19 10z"/>', false, 1.8);
// The selection bar's tools (our own drawings): a cross, a label pointing
// left with a plus or a minus, a calendar page with the "~" of an estimated
// date, a map marker with a plus, an arrow into a folder, an arrow up out of
// a tray, and the photo view's trash can.
var TRASH_ICON = lineIcon('<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v6M14 11v6"/>');
var SEL_ICONS = {
  close: lineIcon('<path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/>'),
  check: lineIcon('<path d="M5 12.8l4.6 4.6L19 7.2"/>'),
  undo: lineIcon('<path d="M9 5.5L5 9.5l4 4"/><path d="M5 9.5h8.5a5 5 0 0 1 0 10H10"/>'),
  tag: lineIcon('<path d="M2 14l4.3-5H14.5A1.5 1.5 0 0 1 16 10.5v7a1.5 1.5 0 0 1-1.5 1.5H6.3z"/><circle cx="6.6" cy="14" r=".6"/><path d="M19 2.5v6.5M15.75 5.75h6.5"/>'),
  untag: lineIcon('<path d="M2 14l4.3-5H14.5A1.5 1.5 0 0 1 16 10.5v7a1.5 1.5 0 0 1-1.5 1.5H6.3z"/><circle cx="6.6" cy="14" r=".6"/><path d="M15.75 5.75h6.5"/>'),
  date: lineIcon('<path d="M4 8.5A2.5 2.5 0 0 1 6.5 6h11A2.5 2.5 0 0 1 20 8.5v9a2.5 2.5 0 0 1-2.5 2.5h-11A2.5 2.5 0 0 1 4 17.5z"/><path d="M8.5 3.5v5M15.5 3.5v5" stroke-width="2.2"/><path d="M7.5 14.5c1.5-2.3 3-2.3 4.5 0s3 2.3 4.5 0"/>'),
  geo: lineIcon('<path d="M10 21C7.5 18.2 4 14.8 4 10.8a6 6 0 0 1 12 0c0 4-3.5 7.4-6 10.2z"/><circle cx="10" cy="10.8" r="1.4" fill="currentColor"/><path d="M19.5 1.5v6M16.5 4.5h6"/>'),
  move: lineIcon('<path d="M6 9.5V7a2 2 0 0 1 2-2h3l2 2h6a2 2 0 0 1 2 2v8.5a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2V17.5"/><path d="M2 13.5h10M9 10.5l3 3-3 3"/>'),
  export: lineIcon('<path d="M4 13.5v4A2.5 2.5 0 0 0 6.5 20h11a2.5 2.5 0 0 0 2.5-2.5v-4"/><path d="M12 15.5V4.5M8 8.5l4-4 4 4"/>'),
  trash: TRASH_ICON
};
SEL_ICONS.hide = HIDE_ICON;
SEL_ICONS.show = SHOW_ICON;
// An icon-only button for the tools row: the tooltip is also its name.
function toolButton(icon, label) {
  var b = el('button', 'tool');
  b.innerHTML = icon;
  setTip(b, label);
  b.setAttribute('aria-label', label);
  return b;
}

// An icon button of the face actions: its quick label is the name of the action
// and, on a second line, what it does. With nothing ticked yet it is dimmed
// (aria-disabled, so that the label still shows) and says to tick faces first.
function actionButton(icon, act) {
  var b = el('button', 'abtn');
  b.innerHTML = icon;
  b.setAttribute('data-act', act);
  return b;
}

function actionOff(b) { return b.getAttribute('aria-disabled') === 'true'; }

function onAction(b, fn) {
  b.onclick = function (ev) { if (!actionOff(b)) fn(ev); };
}

function setAction(b, label, hint, off) {
  var text = off ? tr('unnamed.select_first') : hint;
  setTip(b, text ? label + '\n' + text : label);
  b.setAttribute('aria-label', label);
  if (off) b.setAttribute('aria-disabled', 'true'); else b.removeAttribute('aria-disabled');
}

function markFavFilter() {
  var on = !!state.filter.fav, b = $('fav-btn');
  b.classList.toggle('on', on);
  b.setAttribute('aria-pressed', on ? 'true' : 'false');
  b.innerHTML = on ? HEART_ON : HEART_OFF;
  b.hidden = !!state.filter.view;
}
$('fav-btn').onclick = function () { setFilter(withFilter({ fav: !state.filter.fav })); };

// Heart or un-heart photos (ids of the current list), then update what shows.
function setFavorite(ids, on) {
  return post(LIBAPI + '/favorites', { ids: ids, on: on }).then(function () {
    ids.forEach(function (id) { if (on) state.favs[id] = true; else delete state.favs[id]; });
    paintHearts();
    // Showing only favorites: the list changes (the open viewer keeps its photos until it closes).
    if (!on && state.filter.fav && state.open < 0) loadTimeline(false);
    loadOwnTags();
    if (ids.length === 1 && lb.details && lb.details.id === ids[0]) tagsChanged(ids[0]);
  }).catch(failed);
}

function heartButton(id, cls, readonly) {
  var b = el('button', cls), on = !!state.favs[id];
  b.type = 'button';
  b.innerHTML = on ? HEART_ON : HEART_OFF;
  b.classList.toggle('on', on);
  setTip(b, tr(readonly ? 'fav.readonly' : on ? 'fav.remove' : 'fav.add'));
  b.setAttribute('aria-label', b.getAttribute('data-tip'));
  b.setAttribute('aria-pressed', on ? 'true' : 'false');
  if (readonly) b.disabled = true; // the common timeline only shows hearts
  return b;
}

// Redraw the hearts that are on screen (grid cells and the viewer).
function paintHearts() {
  var d = state.data;
  if (d) Array.prototype.forEach.call(document.querySelectorAll('.cell .fav'), function (b) {
    var id = d.ids[parseInt(b.parentNode.dataset.index, 10)], on = !!state.favs[id];
    b.innerHTML = on ? HEART_ON : HEART_OFF;
    b.classList.toggle('on', on);
    setTip(b, tr(isAll() ? 'fav.readonly' : on ? 'fav.remove' : 'fav.add'));
    b.setAttribute('aria-label', b.getAttribute('data-tip'));
    b.setAttribute('aria-pressed', on ? 'true' : 'false');
  });
  markLightboxHeart();
}

function markLightboxHeart() {
  var d = state.data, i = state.open, b = $('lb-fav');
  if (!d || i < 0) return;
  var on = !!state.favs[d.ids[i]];
  b.innerHTML = on ? HEART_ON : HEART_OFF;
  b.classList.toggle('on', on);
  setTip(b, tr(isAll() ? 'fav.readonly' : on ? 'fav.remove_hint' : 'fav.add_hint'));
  b.setAttribute('aria-label', isAll() ? b.getAttribute('data-tip') : tr(on ? 'fav.remove' : 'fav.add'));
  b.disabled = isAll();
  if (isAll()) b.hidden = !on; // the common timeline shows a heart only on favorites
  b.setAttribute('aria-pressed', on ? 'true' : 'false');
}

function toggleFavoriteOpen() {
  var d = state.data, i = state.open;
  if (!d || i < 0 || isAll()) return;
  var id = d.ids[i];
  setFavorite([id], !state.favs[id]);
}
$('lb-fav').onclick = toggleFavoriteOpen;

// ------------------------------------------------------------------ hidden from the timeline

// Hidden (phase 20): the own tag "hidden" (`tags.rs`), shown as the eye button, never as a tag, and
// folders flagged in the index. Hidden photos are left out of every list unless the Hidden chip is
// on, the word is typed, or the selected folder is itself hidden.
var HIDDEN_TAG = 'hidden';
function isHiddenTag(name) { return String(name).trim().toLowerCase() === HIDDEN_TAG; }
// What typing in a tag field means "hidden" (the server maps these to the tag too).
var HIDDEN_TYPED = [HIDDEN_TAG, 'versteckt', 'verborgen', 'ausgeblendet'];
function meansHidden(name) { return HIDDEN_TYPED.indexOf(String(name).trim().toLowerCase()) >= 0; }

// After hiding or showing: what the lists show and count changes (the open viewer keeps its photos
// until it closes).
function hiddenChanged() {
  loadOwnTags();
  loadFolders();
  loadInfo();
  if (state.open >= 0) lb.hiddenDirty = true;
  else if (!state.filter.view) loadTimeline(false);
}

// Hide photos from the timeline, or show them again. `quiet`: the caller says what happened.
function setHidden(ids, on, quiet) {
  return post(LIBAPI + '/hidden', { ids: ids, on: on }).then(function (r) {
    // Showing a photo that is hidden by its folder does nothing: the folder says it.
    if (!on && !r.files) { if (!quiet) toast(tr('hidden.by_folder_hint')); return r; }
    ids.forEach(function (id) { if (on) state.hids[id] = true; else delete state.hids[id]; });
    if (!quiet && r.files) toast(trn(on ? 'hidden.toast_hidden' : 'hidden.toast_shown', r.files));
    hiddenChanged();
    markLightboxHide();
    if (ids.length === 1 && lb.details && lb.details.id === ids[0]) tagsChanged(ids[0]);
    return r;
  }).catch(failed);
}

// Hide a folder and everything below it, or show it again.
function setFolderHidden(folder, on) {
  return post(LIBAPI + '/hidden/folder', { path: folder.path, on: on }).then(function () {
    hiddenChanged();
  }).catch(failed);
}

function markLightboxHide() {
  var d = state.data, i = state.open, b = $('lb-hide');
  if (!d || i < 0) return;
  var id = d.ids[i], on = !!state.hids[id];
  var details = lb.details && lb.details.id === id ? lb.details : null;
  // Hidden by its folder only: the folder says when it shows again, the button cannot.
  var byFolder = !!(details && details.hidden && !details.hidden_tag && details.hidden_folder);
  b.innerHTML = on ? SHOW_ICON : HIDE_ICON;
  b.classList.toggle('on', on);
  b.disabled = isAll() || byFolder;
  b.hidden = isAll() && !on; // the common timeline shows the eye only on hidden photos
  setTip(b, tr(isAll() ? 'hidden.readonly' : byFolder ? 'hidden.by_folder_hint' : on ? 'hidden.show_hint' : 'hidden.hide_hint'));
  b.setAttribute('aria-label', b.getAttribute('data-tip'));
  b.setAttribute('aria-pressed', on ? 'true' : 'false');
}

function toggleHideOpen() {
  var d = state.data, i = state.open;
  if (!d || i < 0 || isAll() || $('lb-hide').disabled) return;
  var id = d.ids[i];
  setHidden([id], !state.hids[id]);
}
$('lb-hide').onclick = toggleHideOpen;

// The Hidden row of the sidebar's tag list, under Favorites, while something is hidden.
function paintHiddenRow() {
  var list = $('own-tags'), old = $('hidden-row');
  if (old) old.remove();
  $('tags-section').hidden = !list.children.length && !state.hiddenCount;
  if (!state.hiddenCount) return;
  var li = el('li'), r = el('div', 'row');
  li.id = 'hidden-row';
  var mark = el('span', 'toggle');
  mark.innerHTML = HIDE_ICON;
  r.appendChild(mark);
  var name = el('button', 'name', tr('hidden.label'));
  setTip(name, tr('hidden.filter_hint'));
  name.onclick = function () { setFilter({ folder: null, tags: [], hidden: true, q: '' }); };
  r.appendChild(name);
  r.appendChild(el('span', 'count', I18n.number(state.hiddenCount)));
  li.appendChild(r);
  var fav = $('fav-row');
  if (fav) fav.after(li); else list.insertBefore(li, list.firstChild);
  $('tags-section').hidden = false;
}

// Right-click on a folder in the sidebar: hide it from the timeline, or show it again.
$('tree').addEventListener('contextmenu', function (ev) {
  var b = ev.target.closest('button.name');
  var f = b && b.dataset.id ? state.folderById[b.dataset.id] : null;
  if (!f || !f.path) return;
  ev.preventDefault();
  var above = f.hidden && !f.hidden_own;
  showMenu(ev.clientX, ev.clientY, [{
    label: tr(above ? 'hidden.folder_by_parent' : f.hidden ? 'hidden.folder_show' : 'hidden.folder_hide'),
    run: function () { if (!above) setFolderHidden(f, !f.hidden); },
  }]);
});

// ------------------------------------------------------------------ search box

// Typing searches the text right away (words in paths and tag names); the
// list below the box offers tags (counted within the current search) and
// folders, and picking one adds it to the search as a chip.
var searchTimer = null;
// `ctrl` stops the requests of the list shown before (see suggestSoon); `key`
// is what the list shown or on its way is for (suggestKey).
var sugg = { items: [], at: -1, seq: 0, timer: null, ctrl: null, key: null, picked: false, wait: null };

// What a list of suggestions depends on: the text typed and the search but
// its own text.
function suggestKey(text) {
  return text + '\n' + JSON.stringify(Object.assign({}, state.filter, { q: '' }));
}

// While someone types, the list is asked for once they pause (SUGGEST_PAUSE),
// and what was asked for the letters before is called off: every suggestion
// request waits for the drive's one connection, so a request nobody waits for
// any more holds up the one that is wanted (see "Search" under Performance in
// docs/plan.md).
var SUGGEST_PAUSE = 150;
function suggestSoon(v) {
  clearTimeout(sugg.timer);
  stopSuggest();
  sugg.timer = setTimeout(function () { suggest(v); }, SUGGEST_PAUSE);
}

// Calls off the suggestion requests still on their way (a newer list replaces them).
function stopSuggest() {
  ++sugg.seq;
  sugg.key = null;
  if (sugg.ctrl) sugg.ctrl.abort();
  sugg.ctrl = null;
}

$('search').addEventListener('input', function () {
  sugg.picked = false;
  clearTimeout(searchTimer);
  var v = this.value;
  // On the Locations page typing looks for a place; the text only becomes a search with Enter.
  if (state.filter.view === 'locations' && geo.on) { suggest(v); return; }
  // `date:` looks for a date; what follows is not a text search.
  if (DATE_PREFIX.test(v)) { suggestSoon(v); return; }
  // `text:` and `name:` are a term of their own: nothing is searched until Enter or a pick.
  if (!prefixOf(v)) searchTimer = setTimeout(function () { setFilter(withFilter({ q: v.trim() })); }, 300);
  suggestSoon(v);
});
$('search').addEventListener('focus', function () {
  sugg.picked = false;
  // A search shows another list: with photos selected, ask once when the box is
  // entered (not on every key).
  if (state.selecting && selectedIds().length && $('modal').hidden) {
    this.blur();
    openModal(tr('sel.leave_title'), tr('sel.search_text', { count: trn('count.photos', selectedIds().length) }), [
      { label: tr('sel.leave_stay'), cls: 'quiet', focus: true },
      { label: tr('sel.search_go'), onclick: function () { endSelection(); setTimeout(function () { $('search').focus(); }, 0); } },
    ]);
    return;
  }
  suggest(this.value);
});
// A click in the box that already has the focus (after a pick closed the list, or
// Escape) opens the list again; the click that gives the focus does so through `focus`.
var searchHadFocus = false;
$('search').addEventListener('mousedown', function () { searchHadFocus = document.activeElement === this; });
$('search').addEventListener('click', function () {
  if (searchHadFocus && $('suggest').hidden) { sugg.picked = false; suggest(this.value); }
});
$('search').addEventListener('blur', function () { setTimeout(closeSuggest, 150); });
$('search').addEventListener('keydown', function (ev) {
  var open = !$('suggest').hidden && sugg.items.length;
  if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
    if (!open) return;
    ev.preventDefault();
    var n = sugg.items.length;
    sugg.at = ev.key === 'ArrowDown' ? (sugg.at + 1) % n : (sugg.at + n - 1) % n;
    markSuggest();
  } else if (ev.key === 'Enter') {
    ev.preventDefault();
    if (open && sugg.at >= 0) { pickSuggest(sugg.items[sugg.at]); return; }
    if (DATE_PREFIX.test(this.value)) { if (sugg.items.length) pickSuggest(sugg.items[0]); return; }
    clearTimeout(searchTimer);
    closeSuggest();
    var pre = prefixOf(this.value);
    if (pre) {
      // The shortcut becomes a chip; with nothing after it there is nothing to add yet.
      if (pre.term) { this.value = ''; addTerm(pre.kind, pre.term); }
      return;
    }
    setFilter(withFilter({ q: this.value.trim() }));
  } else if (ev.key === 'Escape') {
    closeSuggest();
  } else if (ev.key === 'Backspace' && this.value === '') {
    // Like a token field: the last chip goes.
    var f = state.filter;
    if (f.texts.length) setFilter(withFilter({ texts: f.texts.slice(0, -1) }));
    else if (f.names.length) setFilter(withFilter({ names: f.names.slice(0, -1) }));
    else if (f.fav) setFilter(withFilter({ fav: false }));
    else if (f.hidden) setFilter(withFilter({ hidden: false }));
    else if (f.place) setFilter(withFilter({ place: null }));
    else if (f.nodate) setFilter(withFilter({ nodate: false }));
    else if (f.dates.length) setFilter(withFilter({ dates: f.dates.slice(0, -1) }));
    else if (f.pets.length) setFilter(withFilter({ pets: f.pets.slice(0, -1) }));
    else if (f.groups.length) setFilter(withFilter({ groups: f.groups.slice(0, -1) }));
    else if (f.people.length) setFilter(withFilter({ people: f.people.slice(0, -1) }));
    else if (f.tags.length) setFilter(withFilter({ tags: f.tags.slice(0, -1) }));
    else if (f.folder) setFilter(withFilter({ folder: null }));
  }
});

// "♥ Favorites" when what is typed starts a word for it (favorite, favorit,
// the UI language's own word) and something has a heart.
function favSuggestion(needle, f) {
  var low = needle.toLowerCase();
  if (f.fav || !state.favCount || low.length < 2) return [];
  var words = ['favorite', 'favorites', 'favourite', 'favorit', 'favoriten', tr('fav.label').toLowerCase()];
  if (!words.some(function (w) { return w.indexOf(low) === 0; })) return [];
  return [{ kind: 'fav', id: 'fav', label: tr('fav.label'), count: state.favCount }];
}

// "Hidden" when what is typed starts a word for it (hidden, versteckt, verborgen, ausgeblendet,
// the UI language's own word) and something is hidden. Over all drives it has no number.
function hiddenSuggestion(needle, f) {
  var low = needle.toLowerCase();
  if (f.hidden || low.length < 3 || (!isAll() && !state.hiddenCount)) return [];
  var words = HIDDEN_TYPED.concat([tr('hidden.label').toLowerCase()]);
  if (!words.some(function (w) { return w.indexOf(low) === 0; })) return [];
  return [{ kind: 'hidden', id: 'hidden', label: tr('hidden.label'), count: isAll() ? undefined : state.hiddenCount }];
}

// ---- comparing the drives of a backup pair (Manage drives opens these views)

function compareLabel(kind) { return tr(kind === 'backup' ? 'cmp.only_backup' : kind === 'original' ? 'cmp.only_original' : 'cmp.different'); }

// The disk name of a drive (its folder's name only where that is all there is).
function driveLabel(library) {
  var d = ((drives.cmp && drives.cmp.drives) || []).filter(function (x) { return x.library === library; })[0];
  if (d) return d.volume || d.name;
  var l = drives.list.filter(function (x) { return x.id === library; })[0];
  return l ? l.name : '';
}

// For the open drive, when it is one half of a backup pair: what only it has
// (`kind` is the filter: `backup` on the backup, `original` on the drive it
// copies), how many photos, and the other drive.
function compareFor() {
  if (isAll() || !drives.cmp || !drives.current) return null;
  var id = drives.current.id, list = drives.cmp.backups;
  var mine = list.filter(function (b) { return b.library === id; })[0];
  if (mine) return mine.extra ? { kind: 'backup', count: mine.extra, other: mine.primary_library } : null;
  var of = list.filter(function (b) { return b.primary_library === id; })[0];
  return of && of.missing ? { kind: 'original', count: of.missing, other: of.library } : null;
}

// Counts of the drives and their backup checks, once per page load. A link
// with a comparison the open drive cannot show is opened without it, and
// the page says so.
function loadCompare() {
  drives.cmp = { backups: [], drives: [] };
  if (drives.list.length < 2) { checkCompare(); return Promise.resolve(); }
  // refreshDrives asked for the drives just before: not twice at the start.
  var roles = drives.roles ? Promise.resolve(drives.roles) : api('/api/all/drives');
  return Promise.all([api('/api/all/backups?limit=0'), roles]).then(function (r) {
    drives.cmp = { backups: r[0], drives: r[1] };
  }).catch(function () {}).then(checkCompare).then(loadOnlyHere);
}

// Which photos of the open drive only it has (for the badge in the info panel, always).
function loadOnlyHere() {
  drives.only = {};
  if (!compareFor()) return Promise.resolve();
  return api(LIBAPI + '/compare').then(function (r) {
    (r.ids || []).forEach(function (id) { drives.only[id] = true; });
  }).catch(function () {});
}

function compareProblem(kind) {
  var name = driveLabel(drives.current.id), backups = drives.cmp.backups;
  var pair = backups.filter(function (b) { return b.primary_library; });
  if (kind === 'different') {
    if (!isAll()) return tr('cmp.gone_one', { name: name });
    return pair.some(function (b) { return b.different; }) ? null : tr('cmp.gone_empty');
  }
  if (isAll()) return tr('cmp.gone_all');
  var mine = backups.filter(function (b) { return b.library === drives.current.id; })[0];
  var of = pair.filter(function (b) { return b.primary_library === drives.current.id; })[0];
  if (kind === 'backup') {
    if (!mine) return tr(of ? 'cmp.gone_original' : 'cmp.gone_nobackup', { name: name });
    return mine.extra ? null : tr('cmp.gone_none', { name: name });
  }
  if (mine) return tr('cmp.gone_backup', { name: name });
  if (!of) return tr('cmp.gone_nobackup', { name: name });
  return of.missing ? null : tr('cmp.gone_none', { name: name });
}

function checkCompare() {
  var kind = readHash().compare;
  if (!kind) return;
  var why = compareProblem(kind);
  if (!why) return;
  var h = location.hash.replace(/^#/, '').split('&').filter(function (kv) { return kv.indexOf('compare=') !== 0; }).join('&');
  history.replaceState(null, '', location.pathname + location.search + (h ? '#' + h : ''));
  toast(tr('cmp.removed', { filter: compareLabel(kind), reason: why }));
}

// The badge next to the name in the info panel for a photo that only this
// drive of the pair has (whatever is open): the sentence is its tooltip.
function compareBadge(fileId) {
  var c = compareFor();
  if (isAll() || !c || !drives.only || !drives.only[fileId]) return null;
  var kind = c.kind;
  var b = el('span', 'cmp-badge ' + kind, tr(kind === 'backup' ? 'cmp.only_backup' : 'cmp.not_backed_up'));
  setTip(b, tr(kind === 'backup' ? 'cmp.tip_only_backup' : 'cmp.tip_not_backed_up', { drive: driveLabel(drives.current.id), other: driveLabel(c.other) }));
  return b;
}

// "Only on backup" / "Only on original" in the search (typed as "only on",
// "nur auf" or "backup"), for a drive that is one half of a backup pair and
// has such photos. The photos are compared by content with the other drive.
function compareSuggestion(needle, f) {
  var low = needle.toLowerCase(), c = compareFor();
  if (!c || f.compare || low.length < 2) return [];
  var label = compareLabel(c.kind);
  // The label in this language and in the other one, and the word that is the same in both.
  var words = [label.toLowerCase(), 'only on ' + c.kind, 'nur auf ' + c.kind, 'backup'];
  if (!words.some(function (w) { return w.indexOf(low) === 0; })) return [];
  return [{ kind: 'compare', id: c.kind, label: label, count: c.count }];
}

function suggest(text) {
  clearTimeout(sugg.timer);
  stopSuggest();
  sugg.key = suggestKey(text);
  var seq = sugg.seq, f = state.filter;
  var ctrl = sugg.ctrl = new AbortController(), opts = { signal: ctrl.signal };
  // A list that takes a moment shows grey placeholder rows meanwhile (a quick one is not flashed).
  clearTimeout(sugg.wait);
  sugg.wait = setTimeout(function () { showWaiting(seq); }, SUGGEST_WAIT);
  // On the Locations page the search finds places: picking one shows it.
  if (f.view === 'locations' && geo.on) { showSuggest(placeSuggestions(text.trim(), f)); return; }
  // `text:` and `name:`: one row, the term as typed, with the photos it finds
  // (the server counts them); none when it finds nothing.
  var pre = prefixOf(text), row = null;
  if (pre) {
    if (!pre.term) { showSuggest([], tr(pre.kind === 'text' ? 'search.prefix_empty_text' : 'search.prefix_empty_name')); return; }
    row = { kind: pre.kind, id: pre.term, label: pre.term, prefixed: true, hint: tr(pre.kind === 'text' ? 'search.text_prefixed_hint' : 'search.name_prefixed_hint') };
    if (isAll()) { showSuggest([row]); return; } // over all drives without a number
  }
  // `date:` (or `datum:`): the server answers only dates, a month alone in every year.
  var typed = DATE_PREFIX.exec(text);
  if (isAll()) {
    if (typed) closeSuggest(); // the common timeline has no dates to offer
    else suggestAll(text, seq, opts);
    return;
  }
  var needle = text.trim();
  // Counted within the search as the list shows it (every term but the typed text).
  var within = { q: needle, tag: f.tags, manual: f.manual ? 1 : null, person: f.people, group: f.groups, pet: f.pets, text: f.texts, name: f.names, type: f.types, fav: f.fav ? 1 : null, hidden: f.hidden ? 1 : null, folder: f.folder, place: f.place, date: f.dates, nodate: f.nodate ? 1 : null };
  api(LIBAPI + '/suggest' + query(within), opts).then(function (r) {
    if (seq !== sugg.seq) return;
    if (pre) {
      var found = pre.kind === 'text' ? r.text : r.name;
      if (found && found.count) { row.count = found.count; showSuggest([row]); return; }
      showSuggest([], pre.kind === 'name' ? tr('search.name_none', { term: pre.term }) : state.textLines ? tr('search.text_none', { term: pre.term }) : tr('search.text_unread'));
      return;
    }
    if (typed) {
      var after = typed[1].trim();
      showSuggest(dateItems(r.dates), after && r.dates.length ? '' : tr('dates.hint_prefix'));
      return;
    }
    var items = compareSuggestion(needle, f).concat(favSuggestion(needle, f), hiddenSuggestion(needle, f));
    items = items.concat(r.people.map(function (p) {
      state.personNames[p.id] = p.name;
      state.personSpecies[p.id] = p.species;
      return { kind: 'person', id: p.id, label: p.name, count: p.photos, person: p };
    }));
    r.groups.forEach(function (g) {
      state.groupNames[g.id] = g.name;
      items.push({ kind: 'group', id: g.id, label: g.name, count: g.photos });
    });
    r.pets.forEach(function (p) { items.push({ kind: 'pet', id: p.species, label: PET_TERMS[p.species].label, count: p.photos }); });
    r.tags.forEach(function (t) {
      if (isFavTag(t.name)) return; // the heart, offered above
      state.tagNames[t.id] = t.name;
      items.push({ kind: 'tag', id: t.id, label: t.name, count: t.count, folderTag: t.kind === 'folder' });
    });
    items = items.concat(placeSuggestions(needle, f));
    if (needle && !f.folder) {
      var low = needle.toLowerCase();
      state.folders.filter(function (fo) { return fo.path && fo.count && fo.name.toLowerCase().indexOf(low) >= 0; })
        .slice(0, 4)
        .forEach(function (fo) { items.push({ kind: 'folder', id: fo.id, label: fo.path, count: fo.count }); });
    }
    items = items.concat(textItems(needle, r.text));
    // Last: a year, a month with a year or a day, when that is what was typed.
    items = items.concat(dateItems(r.dates));
    showSuggest(items);
  }).catch(function () { if (seq === sugg.seq) closeSuggest(); });
}

// Over all drives: tags, people and groups by name (the drives share names, not ids).
function suggestAll(text, seq, opts) {
  var needle = text.trim(), f = state.filter;
  // The suggestions narrow with the filter, as on a single drive: counts are
  // the photos the list would show, and what would show nothing is left out.
  var within = { q: needle, tag: f.tags, person: f.people, group: f.groups, pet: f.pets, text: f.texts, name: f.names, type: f.types, fav: f.fav ? 1 : null, hidden: f.hidden ? 1 : null, date: f.dates, nodate: f.nodate ? 1 : null };
  api('/api/all/suggest' + query(within), opts).then(function (r) {
    if (seq !== sugg.seq) return;
    var items = r.people.map(function (p) { return { kind: 'person', id: p.name, label: p.name, count: p.photos, person: { name: p.name, species: p.species } }; });
    r.groups.forEach(function (g) { state.groupNames[g.name] = g.name; items.push({ kind: 'group', id: g.name, label: g.name, count: g.photos }); });
    r.pets.forEach(function (p) { items.push({ kind: 'pet', id: p.species, label: PET_TERMS[p.species].label, count: p.photos }); });
    items = favSuggestion(needle, f).concat(hiddenSuggestion(needle, f), items);
    r.tags.filter(function (t) { return f.tags.indexOf(t.name) < 0 && !isFavTag(t.name); }).forEach(function (t) {
      items.push({ kind: 'tag', id: t.name, label: t.name, count: t.count, folderTag: t.kind === 'folder' });
    });
    items = items.concat(textItems(needle));
    showSuggest(items);
  }).catch(function () { if (seq === sugg.seq) closeSuggest(); });
}

// The last row of a plain search: look for the same word in the text of photos,
// with the photos it finds (`found`, counted by the server), left out when it
// finds none. Greyed while nothing has been read (the Text add-on, then
// "Recognize text"); over all drives without a number.
function textItems(needle, found) {
  if (needle.length < 2) return [];
  var row = { kind: 'text', id: needle, label: needle, hint: tr('search.text_hint', { term: needle }) };
  if (isAll()) return [row];
  if (!state.textLines) { row.off = true; return [row]; }
  if (!found || !found.count) return [];
  row.count = found.count;
  return [row];
}

// Add a text or name term to the search as a chip.
function addTerm(kind, term) {
  var f = state.filter, key = kind === 'text' ? 'texts' : 'names', ch = { q: '' };
  ch[key] = f[key].indexOf(term) < 0 ? f[key].concat([term]) : f[key];
  setFilter(withFilter(ch));
}

var SUGGEST_WAIT = 120;
var WAITING_ROWS = [62, 48, 70, 40, 56];

// Placeholder rows while the list is on its way, if nothing is shown yet (a
// list that is there stays until the new one replaces it).
function showWaiting(seq) {
  var box = $('suggest');
  if (seq !== sugg.seq || document.activeElement !== $('search') || (!box.hidden && box.querySelector('.item'))) return;
  box.textContent = '';
  sugg.items = [];
  sugg.at = -1;
  WAITING_ROWS.forEach(function (w) {
    var row = el('div', 'wait-row');
    row.setAttribute('aria-hidden', 'true');
    row.appendChild(el('span', 'wait-dot'));
    var bar = el('span', 'wait-bar');
    bar.style.width = w + '%';
    row.appendChild(bar);
    box.appendChild(row);
  });
  box.setAttribute('aria-busy', 'true');
  box.hidden = false;
}

function showSuggest(items, hint) {
  var box = $('suggest');
  clearTimeout(sugg.wait);
  box.removeAttribute('aria-busy');
  box.textContent = '';
  sugg.items = items;
  sugg.at = -1;
  var last = null;
  items.forEach(function (it, i) {
    if (it.kind !== last) {
      var f = state.filter, narrowed = f.tags.length || f.people.length || f.groups.length || f.pets.length || f.texts.length || f.names.length || f.folder || f.place;
      var head = it.kind === 'fav' ? tr('fav.label') : it.kind === 'hidden' ? tr('hidden.label') : it.kind === 'compare' ? tr('cmp.head') : it.kind === 'tag' ? tr(narrowed ? 'search.tags_here' : 'search.tags')
        : it.kind === 'person' ? tr(narrowed ? 'search.faces_here' : 'side.faces') : it.kind === 'group' ? tr('search.groups')
          : it.kind === 'pet' ? tr(narrowed ? 'search.pets_here' : 'search.pets') : it.kind === 'place' ? tr('search.places') : it.kind === 'date' ? tr('search.dates')
            : it.kind === 'text' ? tr('search.text') : it.kind === 'name' ? tr('search.names') : tr('search.folders');
      box.appendChild(el('div', 'head', head));
      last = it.kind;
    }
    var b = el('button', 'item' + (it.off ? ' off' : ''));
    b.type = 'button';
    b.setAttribute('role', 'option');
    // People as in the sidebar: their picture, and the dog or cat for a pet.
    var label = el('span', 'label');
    if (it.kind === 'person') label.appendChild(avatar(it.person, 'tiny'));
    if (it.kind === 'person' && it.person && it.person.species) label.appendChild(speciesIcon(it.person.species));
    if (it.kind === 'group') label.appendChild(icon('group'));
    if (it.kind === 'pet') label.appendChild(speciesIcon(it.id));
    if (it.kind === 'place') label.appendChild(icon('place'));
    if (it.kind === 'date') label.appendChild(icon('date'));
    if (it.kind === 'folder') label.appendChild(folderIcon());
    if (it.kind === 'hidden') label.appendChild(icon('hide'));
    if (it.kind === 'text' || it.kind === 'name') label.appendChild(icon(it.kind === 'text' ? 'scan' : 'draft'));
    label.appendChild(el('span', 'text', (it.kind === 'text' || it.kind === 'name' ? '“' : it.kind === 'fav' ? '♥ ' : it.kind === 'compare' ? '⇄ ' : (it.kind === 'pet' || it.kind === 'group' || it.kind === 'person' || it.kind === 'place' || it.kind === 'date' || it.kind === 'hidden') ? ''
        : it.kind === 'folder' ? '' : '# ') + it.label + (it.kind === 'text' || it.kind === 'name' ? '”' : '')));
    b.appendChild(label);
    b.appendChild(el('span', 'count', it.count === undefined ? '' : I18n.number(it.count)));
    b.onmousedown = function (ev) { ev.preventDefault(); }; // keep the focus in the box
    b.onclick = function () { pickSuggest(it); };
    b.dataset.i = i;
    box.appendChild(b);
    if (it.hint) box.appendChild(el('div', 'hint', it.hint));
  });
  if (hint) box.appendChild(el('div', 'hint', hint));
  box.hidden = !(items.length || hint) || document.activeElement !== $('search');
}

function markSuggest() {
  Array.prototype.forEach.call($('suggest').querySelectorAll('.item'), function (b) {
    b.classList.toggle('at', String(sugg.at) === b.dataset.i);
  });
}

function closeSuggest() { clearTimeout(sugg.timer); clearTimeout(sugg.wait); stopSuggest(); $('suggest').hidden = true; $('suggest').removeAttribute('aria-busy'); sugg.at = -1; }

function pickSuggest(it) {
  clearTimeout(searchTimer);
  // The list closes with a pick and the box keeps the focus; the new search does
  // not offer it again until the box is clicked or typed in.
  closeSuggest();
  sugg.picked = true;
  $('search').value = '';
  var f = state.filter;
  if (it.kind === 'fav') setFilter(withFilter({ fav: true, q: '' }));
  else if (it.kind === 'hidden') setFilter(withFilter({ hidden: true, q: '' }));
  else if (it.kind === 'compare') setFilter(withFilter({ compare: it.id, q: '' }));
  else if (it.kind === 'tag') setFilter(withFilter({ tags: f.tags.indexOf(it.id) < 0 ? f.tags.concat([it.id]) : f.tags, q: '' }));
  else if (it.kind === 'person') setFilter(withFilter({ people: f.people.indexOf(it.id) < 0 ? f.people.concat([it.id]) : f.people, q: '' }));
  else if (it.kind === 'group') setFilter(withFilter({ groups: f.groups.indexOf(it.id) < 0 ? f.groups.concat([it.id]) : f.groups, q: '' }));
  else if (it.kind === 'pet') setFilter(withFilter({ pets: f.pets.indexOf(it.id) < 0 ? f.pets.concat([it.id]) : f.pets, q: '' }));
  else if (it.kind === 'date') setFilter(withFilter({ dates: f.dates.indexOf(it.id) < 0 ? f.dates.concat([it.id]) : f.dates, q: '' }));
  else if (it.kind === 'place') pickPlace(it.id);
  else if (it.kind === 'text' || it.kind === 'name') {
    if (it.off) { toast(tr('search.text_unread')); return; }
    addTerm(it.kind, it.id);
  } else setFilter(withFilter({ folder: it.id, q: '' }));
}

var RECENT_MAX = 4; // how many "Recently used" the suggestion lists show

// Suggestions for the tag fields (info panel, "Add tag…"): with the field
// empty the tags used last, then the most used; while typing the matches,
// the recent ones first, then by use. A list under the field, not a
// <datalist>, so it can have headings.
var suggestSeq = 0;
function attachTagSuggestions(input) {
  var list = el('div', 'plist tlist');
  list.setAttribute('role', 'listbox');
  list.hidden = true;
  var items = [], at = -1;
  var close = function () { list.hidden = true; at = -1; };
  var mark = function () {
    Array.prototype.forEach.call(list.querySelectorAll('.item'), function (b, k) { b.classList.toggle('at', k === at); });
  };
  var choose = function (name) {
    input.value = name;
    close();
    // The field's own Enter handling adds the tag.
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
  };
  var show = function (tags, text) {
    list.textContent = '';
    items = [];
    at = -1;
    var low = fold(text);
    tags = tags.filter(function (t) { return !isFavTag(t.name); });
    // Tags a photo was last given (the index remembers when), newest first.
    var last = tags.filter(function (t) { return t.last_used && (!low || fold(t.name).indexOf(low) >= 0); })
      .sort(function (a, b) { return b.last_used - a.last_used; }).slice(0, RECENT_MAX);
    var rest = tags.filter(function (t) { return last.indexOf(t) < 0; });
    var section = function (title, ts) {
      if (!ts.length) return;
      if (title) list.appendChild(el('div', 'head', title));
      ts.forEach(function (t) {
        var b = el('button', 'item');
        b.type = 'button';
        b.appendChild(el('span', 'grow', t.name));
        b.appendChild(el('span', 'count', I18n.number(t.count)));
        b.onmousedown = function (ev) { ev.preventDefault(); };
        b.onclick = function () { choose(t.name); };
        items.push(t.name);
        list.appendChild(b);
      });
    };
    section(tr('suggest.recent'), last);
    section(last.length ? tr(text ? 'suggest.more' : 'suggest.most_used') : '', rest);
    list.hidden = !items.length || document.activeElement !== input;
  };
  var load = function () {
    var seq = ++suggestSeq, text = input.value.trim();
    // Own tags when empty (folder names are not what one types by hand).
    api(LIBAPI + '/tags' + query(text ? { q: text, limit: 50 } : { own: 1, limit: 500 })).then(function (tags) {
      if (seq !== suggestSeq) return;
      tags.forEach(function (t) { state.tagNames[t.id] = t.name; });
      show(tags, text);
    }).catch(function () {});
  };
  // Put the list under the field as soon as the field is in the page.
  var place = function () {
    if (input.parentNode && list.previousSibling !== input) input.parentNode.insertBefore(list, input.nextSibling);
  };
  input.addEventListener('focus', function () { place(); load(); });
  input.addEventListener('input', function () { place(); load(); });
  input.addEventListener('blur', function () { setTimeout(close, 150); });
  input.addEventListener('keydown', function (ev) {
    if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
      if (!items.length) return;
      ev.preventDefault();
      list.hidden = false;
      at = ev.key === 'ArrowDown' ? (at + 1) % items.length : (at + items.length - 1) % items.length;
      mark();
    } else if (ev.key === 'Enter' && at >= 0 && !list.hidden) {
      input.value = items[at]; // before the field's own handler reads it
      close();
    } else if (ev.key === 'Escape' && !list.hidden) {
      ev.stopImmediatePropagation(); // the first Escape closes the list only
      close();
    }
  });
}

// ------------------------------------------------------------------ folders

function loadFolders() {
  return api(LIBAPI + '/folders').then(function (folders) {
    state.folders = folders;
    state.folderById = {};
    folders.forEach(function (f) { state.folderById[f.id] = f; f.children = []; });
    var roots = [];
    folders.forEach(function (f) {
      var p = f.parent != null ? state.folderById[f.parent] : null;
      if (p) p.children.push(f); else roots.push(f);
    });
    var tree = $('tree');
    tree.textContent = '';
    var list = $('folder-list');
    list.textContent = '';
    folders.forEach(function (f) {
      if (!f.path) return;
      var o = el('option');
      o.value = f.path;
      list.appendChild(o);
    });
    // The library root itself is "All photos"; show its children.
    var top = roots.length === 1 && roots[0].path === '' ? roots[0].children : roots;
    top.forEach(function (f) { tree.appendChild(folderNode(f, 0)); });
    markActiveFolder();
    renderChips();
  });
}

// Material "keyboard arrow" icons for the expand/collapse buttons.
var ARROW_DOWN = lineIcon('<path d="M7 10l5 5 5-5"/>').replace(/1em/g, '20');
var ARROW_RIGHT = lineIcon('<path d="M10 7l5 5-5 5"/>').replace(/1em/g, '20');
function setArrow(btn, open) { btn.innerHTML = open ? ARROW_DOWN : ARROW_RIGHT; }

function folderNode(f, depth) {
  var li = el('li');
  var row = el('div', 'row');
  var kids = f.children.filter(function (c) { return c.count > 0; });
  var toggle = el('button', 'toggle');
  if (kids.length) setArrow(toggle, false);
  var name = el('button', 'name', f.name);
  name.dataset.id = f.id;
  name.title = f.path;
  var count = el('span', 'count', I18n.number(f.count));
  row.appendChild(toggle);
  row.appendChild(name);
  if (f.hidden) {
    // Hidden from the timeline: dimmed, with a small eye, and still a click away.
    row.classList.add('hidden-folder');
    var mark = el('span', 'ico');
    mark.innerHTML = HIDE_ICON;
    setTip(mark, tr('hidden.folder_row_hint'));
    row.appendChild(mark);
  }
  row.appendChild(count);
  li.appendChild(row);
  if (f.count === 0) li.hidden = true;
  var ul = null;
  var expand = function (open) {
    if (!kids.length) return;
    if (open && !ul) {
      ul = el('ul');
      kids.forEach(function (c) { ul.appendChild(folderNode(c, depth + 1)); });
      li.appendChild(ul);
      markActiveFolder();
    }
    if (ul) ul.hidden = !open;
    setArrow(toggle, open);
  };
  li._expand = expand;
  toggle.onclick = function () { expand(!ul || ul.hidden); };
  name.onclick = function () {
    expand(true);
    // Opening a folder replaces the search: its chip is the only one left.
    setFilter({ folder: f.id, tags: [], people: [], pets: [], types: state.filter.types, fav: false, hidden: false, q: '' });
  };
  return li;
}

function markActiveFolder() {
  var active = state.filter.folder;
  // Open the path to the active folder so it is visible.
  if (active && state.folderById[active]) {
    var chain = [];
    for (var f = state.folderById[active]; f; f = f.parent != null ? state.folderById[f.parent] : null) chain.unshift(f.id);
    chain.forEach(function (id) {
      var b = document.querySelector('.tree .name[data-id="' + id + '"]');
      if (b && id !== active) b.parentNode.parentNode._expand(true);
    });
  }
  Array.prototype.forEach.call(document.querySelectorAll('.tree .name'), function (b) {
    b.classList.toggle('active', String(active) === b.dataset.id);
  });
  var fl = state.filter, manualOnly = fl.manual && fl.tags.length === 1;
  Array.prototype.forEach.call(document.querySelectorAll('#own-tags .name[data-tag]'), function (b) {
    var on = fl.tags.indexOf(parseInt(b.dataset.tag, 10)) >= 0;
    // "Manual tags only" takes the fill; the tag keeps only its colour.
    b.classList.toggle('active', on && !manualOnly);
    b.classList.toggle('part', on && manualOnly);
    var sub = b.parentNode.parentNode.querySelector('.row.sub');
    if (sub) sub.hidden = !(on && fl.tags.length === 1);
  });
  Array.prototype.forEach.call(document.querySelectorAll('#own-tags .name[data-manual]'), function (b) {
    b.classList.toggle('active', manualOnly && fl.tags[0] === parseInt(b.dataset.manual, 10));
  });
}

$('all').onclick = function () { setFilter({ folder: null, tags: [], people: [], compare: '', q: '' }); };
$('nav-nodate').onclick = function () { setFilter({ folder: null, tags: [], people: [], compare: '', q: '', nodate: true }); };
$('nav-dups').onclick = function () { showView('duplicates'); };
$('nav-trash').onclick = function () { showView('trash'); };
$('nav-drives').onclick = function () { showView('drives'); };

$('menu').onclick = function () {
  if (window.matchMedia('(max-width: 760px)').matches) document.body.classList.toggle('side-open');
  else { document.body.classList.toggle('side-closed'); relayout(); render(); }
};

// Collapsible sidebar sections (Faces, Tags, Folders). A section holding
// the current selection stays open and its caret is disabled.
var sectionClosed = {};
try { sectionClosed = JSON.parse(localStorage.getItem('sectionClosed') || '{}') || {}; } catch (e) { sectionClosed = {}; }

function sectionHasActive(id) {
  var f = state.filter;
  if (id === 'folders-section') return !f.view && f.folder != null;
  if (id === 'tags-section') return !f.view && f.tags.length > 0;
  if (id === 'locations-section') return f.view === 'locations';
  return f.view === 'people' || f.view === 'unnamed' || f.view === 'person' || (!f.view && (f.people.length > 0 || f.groups.length > 0));
}

// A section holding the current selection cannot be folded (a safeguard).
function toggleSection(id) {
  if (sectionHasActive(id)) return;
  sectionClosed[id] = !sectionClosed[id];
  try { localStorage.setItem('sectionClosed', JSON.stringify(sectionClosed)); } catch (e) { /* ignore */ }
  updateSections();
}

// Faces and Locations: the title opens its page and unfolds the section.
function openSection(id) {
  if (!sectionClosed[id]) return;
  sectionClosed[id] = false;
  try { localStorage.setItem('sectionClosed', JSON.stringify(sectionClosed)); } catch (e) { /* ignore */ }
  updateSections();
}

function updateSections() {
  Array.prototype.forEach.call(document.querySelectorAll('.caret'), function (c) {
    var id = c.dataset.section;
    var locked = sectionHasActive(id);
    var open = locked || !sectionClosed[id];
    c.disabled = locked;
    setArrow(c, open);
    c.setAttribute('aria-expanded', String(open));
    $(c.dataset.body).hidden = !open;
  });
}

Array.prototype.forEach.call(document.querySelectorAll('.caret'), function (c) {
  c.onclick = function () { toggleSection(c.dataset.section); };
});
Array.prototype.forEach.call(document.querySelectorAll('.side-tags h3 > span, #folders-section > span'), function (s) {
  var id = s.parentNode.querySelector('.caret').dataset.section;
  s.style.cursor = 'pointer';
  s.onclick = function () { toggleSection(id); };
});

function closeSidebarOnPhone() { document.body.classList.remove('side-open'); }

// ------------------------------------------------------------------ timeline

function loadTimeline(resetScroll) {
  var seq = ++state.loadSeq;
  var f = state.filter;
  // A newer list replaces the one still on its way (typing in the search box asks for one per pause).
  if (state.loadCtrl) state.loadCtrl.abort();
  var ctrl = state.loadCtrl = new AbortController();
  var url = isAll() ? '/api/all/timeline' + query({ tag: f.tags, person: f.people, group: f.groups, pet: f.pets, text: f.texts, name: f.names, type: f.types, fav: f.fav ? 1 : null, hidden: f.hidden ? 1 : null, date: f.dates, nodate: f.nodate ? 1 : null, compare: f.compare === 'different' ? 'different' : null, q: f.q })
    : LIBAPI + '/timeline' + query({ folder: f.folder, tag: f.tags, manual: f.manual ? 1 : null, person: f.people, group: f.groups, pet: f.pets, text: f.texts, name: f.names, type: f.types, fav: f.fav ? 1 : null, hidden: f.hidden ? 1 : null, place: f.place, date: f.dates, nodate: f.nodate ? 1 : null, compare: f.compare === 'different' ? null : f.compare || null, q: f.q });
  return api(url, { signal: ctrl.signal }).then(function (data) {
    if (seq !== state.loadSeq) return;
    state.loadCtrl = null;
    if (isAll()) allTimelineLoaded(data);
    state.anchor = null; // indices change with the new list
    var unnamed = !isAll() && (f.tags.some(function (id) { return !state.tagNames[id]; }) || f.people.some(function (id) { return !state.personNames[id]; }) || f.groups.some(function (id) { return !state.groupNames[id]; }));
    data.tags.forEach(function (t) { state.tagNames[t.id] = t.name; });
    data.people.forEach(function (p) { state.personNames[p.id] = p.name; });
    (data.groups || []).forEach(function (g) { state.groupNames[g.id] = g.name; });
    if (unnamed) renderChips(); // a link with tags this page has not seen yet
    // A date change moves photos: the open one is found again by its id.
    var openId = state.open >= 0 && state.data && state.data.ids ? state.data.ids[state.open] : null;
    state.data = data;
    if (openId != null) { var again = data.ids.indexOf(openId); if (again >= 0) state.open = again; }
    state.est = {};
    (data.est || []).forEach(function (e) { state.est[e[0]] = e[1]; });
    state.favs = {};
    (data.favs || []).forEach(function (id) { state.favs[id] = true; });
    state.hids = {};
    (data.hids || []).forEach(function (id) { state.hids[id] = true; });
    state.live = {};
    data.live.forEach(function (p) { state.live[p[0]] = p[1]; });
    // What a text search found in each photo, shown over its thumbnail.
    state.snips = {};
    (data.snips || []).forEach(function (p) { state.snips[p[0]] = p[1]; });
    var scroller = $('scroller');
    var keep = resetScroll ? 0 : scroller.scrollTop;
    relayout();
    scroller.scrollTop = keep;
    render();
    fillYears();
    state.countFor = location.hash;
    showCount();
    $('empty').hidden = data.count > 0;
  }).catch(function (e) { if (e.name !== 'AbortError') console.error(e); });
}

// Month groups over the (already sorted) items.
function groups() {
  var d = state.data, out = [];
  // Pairs of versions in path order: one list, no months.
  if (d.flat) return d.count ? [{ month: 0, start: 0, end: d.count, flat: true }] : [];
  for (var i = 0; i < d.count; i++) {
    var month = Math.floor(d.days[i] / 100);
    var g = out[out.length - 1];
    if (!g || g.month !== month) out.push(g = { month: month, start: i, end: i });
    g.end = i + 1;
  }
  return out;
}

function monthLabel(month) {
  var y = Math.floor(month / 100), m = month % 100;
  if (!y) return tr('grid.no_date');
  if (!m) return String(y);
  return I18n.date(new Date(y, m - 1, 1), { month: 'long', year: 'numeric' });
}

function relayout() {
  var sizer = $('sizer');
  Object.keys(state.rendered).forEach(function (k) { state.rendered[k].remove(); });
  state.rendered = {};
  state.rows = [];
  if (!state.data) return;
  var width = sizer.clientWidth || 300;
  var target = width < 500 ? 96 : width < 900 ? 140 : 180;
  state.cols = Math.max(3, Math.round(width / target));
  // Two versions of a photo sit side by side: whole pairs in a row.
  if (state.data.flat && state.cols % 2) state.cols++;
  state.cell = width / state.cols;
  var y = 0;
  groups().forEach(function (g) {
    if (!g.flat) {
      state.rows.push({ type: 'h', top: y, h: HEADER_H, label: monthLabel(g.month), n: g.end - g.start, month: g.month, start: g.start, end: g.end });
      y += HEADER_H;
    }
    for (var i = g.start; i < g.end; i += state.cols) {
      state.rows.push({ type: 'r', top: y, h: state.cell, start: i, end: Math.min(i + state.cols, g.end) });
      y += state.cell;
    }
  });
  sizer.style.height = Math.ceil(y + 24) + 'px';
}

// First row whose bottom is below y.
function rowAt(y) {
  var rows = state.rows, lo = 0, hi = rows.length - 1;
  while (lo < hi) {
    var mid = (lo + hi) >> 1;
    if (rows[mid].top + rows[mid].h <= y) lo = mid + 1; else hi = mid;
  }
  return lo;
}

function render() {
  if (!state.rows.length || state.filter.view) return;
  var scroller = $('scroller'), sizer = $('sizer');
  var offset = sizer.offsetTop;
  var top = scroller.scrollTop - offset, bottom = top + scroller.clientHeight;
  var first = rowAt(Math.max(0, top - OVERSCAN));
  var keep = {};
  for (var i = first; i < state.rows.length && state.rows[i].top < bottom + OVERSCAN; i++) {
    keep[i] = true;
    if (!state.rendered[i]) {
      var e = buildRow(state.rows[i]);
      state.rendered[i] = e;
      sizer.appendChild(e);
    }
  }
  // Rows scrolled out of reach go, and their thumbnails stop loading: a fast
  // scroll through years of photos asks only for where it stops.
  Object.keys(state.rendered).forEach(function (k) {
    if (!keep[k]) { dropPending(state.rendered[k]); state.rendered[k].remove(); delete state.rendered[k]; }
  });
  // The month at the top of the viewport.
  var r = rowAt(Math.max(0, top));
  for (; r >= 0; r--) if (state.rows[r].type === 'h') break;
  if (!state.filter.view) $('current-month').textContent = r >= 0 ? state.rows[r].label : '';
}

// The page that is open, shown next to the drive name instead of a month.
var VIEW_LABELS = {
  duplicates: 'side.duplicates', trash: 'side.trash', settings: 'side.settings', drives: 'side.manage_drives',
  faces: 'side.faces', pets: 'side.faces', people: 'side.faces', unnamed: 'people.unnamed', person: 'side.faces',
  locations: 'side.locations', text: 'side.text_check',
};

function thumbUrl(i) {
  var d = state.data;
  return fileBase(d.ids[i]) + '/thumb?v=' + d.versions.substr(i * 8, 8);
}

function buildRow(row) {
  if (row.type === 'h') {
    var h = el('div', 'sec', row.label);
    h.appendChild(el('span', 'n', I18n.number(row.n)));
    // While selecting: the whole month at once (also where Shift is missing, the iPad).
    var pick = el('button', 'pick');
    pick.type = 'button';
    pick.dataset.start = row.start;
    pick.dataset.end = row.end;
    pick.textContent = tr(allSelected(row.start, row.end) ? 'grid.deselect' : 'grid.select_all');
    pick.onclick = function () { selectRange(row.start, row.end - 1, !allSelected(row.start, row.end)); };
    h.appendChild(pick);
    h.style.top = row.top + 'px';
    h.style.height = row.h + 'px';
    return h;
  }
  var d = state.data;
  var e = el('div', 'row-cells');
  e.style.top = row.top + 'px';
  e.style.height = row.h + 'px';
  for (var i = row.start; i < row.end; i++) {
    var a = el('a', 'cell' + (state.selected[d.ids[i]] ? ' sel' : ''));
    a.style.width = state.cell + 'px';
    a.style.height = state.cell + 'px';
    a.href = '#';
    a.dataset.index = i;
    var ph = el('div', 'ph');
    var img = el('img');
    img.alt = '';
    img.decoding = 'async';
    img.onload = function () { this.classList.add('ok'); };
    img.onerror = thumbFailed(d.ids[i], d.kinds[i]);
    img.src = frames.cache[d.ids[i]] || thumbUrl(i);
    ph.appendChild(img);
    a.appendChild(ph);
    var kind = d.kinds[i];
    var badge = KIND_BADGE[kind] || '';
    if (state.live[d.ids[i]]) badge = 'LIVE';
    if (badge) a.appendChild(el('span', 'badge', badge));
    var snip = state.snips && state.snips[d.ids[i]];
    if (snip) { var sn = el('span', 'snip', snip); sn.title = snip; a.appendChild(sn); }
    if (state.est && state.est[d.ids[i]]) a.appendChild(estimateMark(state.est[d.ids[i]]));
    if (d.flat && isAll()) {
      var side = drives.allLibs[Math.floor(d.ids[i] / SPAN)];
      if (side) a.appendChild(el('span', 'tb ' + (side.role === 'backup' ? 'bk' : 'orig'), tr(side.role === 'backup' ? 'cmp.badge_backup' : 'cmp.badge_original')));
    }
    if (!isAll()) a.appendChild(heartButton(d.ids[i], 'fav'));
    else if (state.favs[d.ids[i]]) a.appendChild(heartButton(d.ids[i], 'fav', true));
    e.appendChild(a);
  }
  return e;
}

function thumbFailed(id, kind) {
  return function () {
    var img = this, cell = img.parentNode.parentNode;
    if (kind !== 'v' || img.dataset.frame) { cell.classList.add('broken'); return; }
    img.dataset.frame = '1';
    videoFrame(id, img, function (url) {
      if (url) img.src = url; else cell.classList.add('broken');
    });
  };
}

// Videos the server has no poster for (it has neither the system's video
// framework nor ffmpeg, or they could not read the video): grab a frame in
// the browser, two videos at a time, and send it to the server, which keeps
// a copy of its own as the video's poster.
var frames = { cache: {}, queue: [], busy: 0 };

function videoFrame(id, img, done) {
  frames.queue.push({ id: id, img: img, done: done });
  pumpFrames();
}

function pumpFrames() {
  while (frames.busy < 2 && frames.queue.length) {
    var job = frames.queue.shift();
    if (frames.cache[job.id]) job.done(frames.cache[job.id]);
    else if (job.img.isConnected) grabFrame(job); // skip cells scrolled away
  }
}

function grabFrame(job) {
  frames.busy++;
  var v = document.createElement('video');
  var finished = false;
  var timer = setTimeout(function () { finish(null); }, 20000);
  function finish(url) {
    if (finished) return;
    finished = true;
    clearTimeout(timer);
    v.removeAttribute('src');
    v.load();
    frames.busy--;
    if (url) frames.cache[job.id] = url;
    job.done(url);
    pumpFrames();
  }
  function draw() {
    try {
      // Same size as the server's posters: longer edge 384 px, never larger.
      var w = v.videoWidth, h = v.videoHeight, s = Math.min(1, 384 / Math.max(w, h, 1));
      var c = document.createElement('canvas');
      c.width = Math.max(1, Math.round(w * s));
      c.height = Math.max(1, Math.round(h * s));
      c.getContext('2d').drawImage(v, 0, 0, c.width, c.height);
      c.toBlob(function (b) {
        if (b) savePoster(job.id, b);
        finish(b ? URL.createObjectURL(b) : null);
      }, 'image/jpeg', 0.8);
    } catch (e) { finish(null); }
  }
  v.muted = true;
  v.playsInline = true;
  v.preload = 'auto';
  v.onerror = function () { finish(null); };
  v.onloadedmetadata = function () {
    // ~10% in, like the server's posters (the first frame is often black).
    var at = isFinite(v.duration) ? v.duration / 10 : 0;
    if (at > 0) { v.onseeked = draw; v.currentTime = at; } else if (v.readyState >= 2) draw(); else v.onloadeddata = draw;
  };
  v.src = fileBase(job.id) + '/original';
}

// Best effort: without it the frame is simply grabbed again next time.
function savePoster(id, blob) {
  fetch(fileBase(id) + '/poster', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'Content-Type': 'image/jpeg', 'X-Shoebox': '1' },
    body: blob,
  }).catch(function () {});
}

// Where item i sits in the layout.
function rowOf(i) {
  for (var r = 0; r < state.rows.length; r++) {
    var row = state.rows[r];
    if (row.type === 'r' && row.start <= i && i < row.end) return r;
  }
  return -1;
}

// Give item i's cell the keyboard focus, scrolling it into view first (to a
// third of the way down with `center`, else just far enough) and rendering
// its row if it was not.
function focusCell(i, center) {
  var r = rowOf(i);
  if (r < 0) return;
  var row = state.rows[r], scroller = $('scroller');
  var y = row.top + $('sizer').offsetTop;
  if (y < scroller.scrollTop || y + row.h > scroller.scrollTop + scroller.clientHeight) {
    if (center) scroller.scrollTop = y - scroller.clientHeight / 3;
    else scroller.scrollTop = y < scroller.scrollTop ? y : y + row.h - scroller.clientHeight;
  }
  render();
  var a = $('sizer').querySelector('.cell[data-index="' + i + '"]');
  if (a) a.focus({ preventScroll: true });
}

// Arrow keys in the grid: left/right to the previous/next item, up/down to
// the same column in the row above/below (or its last item).
function moveFocus(cell, key) {
  var i = parseInt(cell.dataset.index, 10), d = state.data;
  if (key === 'ArrowLeft' || key === 'ArrowRight') {
    var j = i + (key === 'ArrowLeft' ? -1 : 1);
    if (j >= 0 && j < d.count) focusCell(j);
    return;
  }
  var r = rowOf(i), dir = key === 'ArrowUp' ? -1 : 1;
  if (r < 0) return;
  var col = i - state.rows[r].start;
  for (var t = r + dir; t >= 0 && t < state.rows.length; t += dir) {
    var row = state.rows[t];
    if (row.type === 'r') { focusCell(Math.min(row.start + col, row.end - 1)); return; }
  }
}

$('sizer').addEventListener('click', function (ev) {
  var a = ev.target.closest('.cell');
  if (!a) return;
  ev.preventDefault();
  var i = parseInt(a.dataset.index, 10);
  // The heart in the corner: heart or un-heart, nothing else.
  if (ev.target.closest('.fav')) { if (isAll()) return; var fid = state.data.ids[i]; setFavorite([fid], !state.favs[fid]); return; }
  // Shift-click: everything between the last clicked photo and this one.
  if (ev.shiftKey && state.selecting && state.anchor != null) selectRange(state.anchor, i, true);
  else if (state.selecting || ev.shiftKey) {
    if (!state.selecting) startSelection();
    toggleSelected(i, a);
  } else openLightbox(i);
});
// Keep Shift-click from selecting the page's text.
$('sizer').addEventListener('mousedown', function (ev) {
  if (ev.shiftKey && ev.target.closest('.cell')) ev.preventDefault();
});

// Shift-click in a list of faces being selected: the items from the last one
// clicked (`anchor`, an id as `idOf` gives it) to this one, both included, in
// either order; null if there is no anchor in the list. The anchor stays.
function pickSpan(list, idOf, anchor, id) {
  var a = -1, b = -1;
  list.forEach(function (x, i) {
    if (idOf(x) === anchor) a = i;
    if (idOf(x) === id) b = i;
  });
  if (a < 0 || b < 0) return null;
  return list.slice(Math.min(a, b), Math.max(a, b) + 1);
}

// Click and drag while selecting (mouse or pen, as in Finder): a rectangle
// from where the button went down to the pointer. Whatever it covers, and only
// that, takes the opposite of the state of the tile pressed (an empty spot
// counts as unselected, so the drag selects), and what the rectangle leaves
// again gets back what it had. Near the top or bottom edge the page scrolls
// along and the rectangle grows with it. Touch keeps scrolling the page (tap to
// select there).
//
// A list takes part by being a `.has-drag` element whose `dragSite()` returns,
// while its tiles can be selected, { sel: selector of a tile, itemOf(tile),
// get(item), set(item, on) (data and look), done(item) (after a change; `item`
// is the one under the pointer, if any: anchor, bar), hit(rect) (optional: the
// items a rectangle of window coordinates covers, when the tiles are not all
// in the page) }; null while it cannot.
var tileDrag = null;
var DRAG_EDGE = 48; // px from the edge of the page where it scrolls along
var DRAG_START = 4; // px the pointer must move before a press becomes a drag
var DRAG_SKIP = 'button, input, select, textarea, .acts, .more-btn, .near, .mark, .copen, .menu, .fav';

$('scroller').addEventListener('pointerdown', function (ev) {
  if (ev.button !== 0 || ev.shiftKey || ev.pointerType === 'touch' || !ev.target.closest || ev.target.closest(DRAG_SKIP)) return;
  var host = ev.target.closest('.has-drag');
  var site = host && host.dragSite();
  if (!site) return;
  var tile = ev.target.closest(site.sel);
  if (tile && !host.contains(tile)) tile = null;
  var item = tile ? site.itemOf(tile) : undefined;
  var sr = $('scroller').getBoundingClientRect();
  tileDrag = {
    site: site, host: host, on: item === undefined ? true : !site.get(item),
    sx: ev.clientX - sr.left + $('scroller').scrollLeft, sy: ev.clientY - sr.top + $('scroller').scrollTop,
    x: ev.clientX, y: ev.clientY, pointer: ev.pointerId, moved: false, scrolling: false, done: false,
    before: new Map(), covered: new Set(), box: null,
  };
  document.body.classList.add('tile-drag');
});
document.addEventListener('pointermove', function (ev) {
  if (!tileDrag || tileDrag.done || ev.pointerId !== tileDrag.pointer) return;
  tileDrag.x = ev.clientX;
  tileDrag.y = ev.clientY;
  tileDragUpdate();
  if (tileDrag && tileDrag.moved && !tileDrag.scrolling) { tileDrag.scrolling = true; requestAnimationFrame(tileDragScroll); }
});
document.addEventListener('pointerup', tileDragEnd);
document.addEventListener('pointercancel', tileDragEnd);
// The tiles are links with pictures: no dragging them out of the page while selecting.
$('scroller').addEventListener('dragstart', function (ev) {
  var host = ev.target.closest && ev.target.closest('.has-drag');
  var site = host && host.dragSite();
  if (site && ev.target.closest(site.sel)) ev.preventDefault();
});
// The click that ends a drag selects nothing more.
$('scroller').addEventListener('click', function (ev) {
  if (tileDrag && tileDrag.moved) { ev.stopImmediatePropagation(); ev.preventDefault(); }
}, true);

function tileDragUpdate() {
  var d = tileDrag, scroller = $('scroller');
  if (!d.host.isConnected || !d.host.dragSite()) { tileDragEnd({ pointerId: d.pointer }); return; }
  var sr = scroller.getBoundingClientRect();
  // The rectangle in the page's own coordinates (it scrolls along) and in the window's.
  var px = Math.max(0, Math.min(scroller.clientWidth, d.x - sr.left + scroller.scrollLeft));
  var py = Math.max(0, Math.min(scroller.scrollHeight, d.y - sr.top + scroller.scrollTop));
  if (!d.moved && Math.abs(d.x - (d.sx + sr.left - scroller.scrollLeft)) < DRAG_START && Math.abs(d.y - (d.sy + sr.top - scroller.scrollTop)) < DRAG_START) return;
  var x1 = Math.min(d.sx, px), x2 = Math.max(d.sx, px), y1 = Math.min(d.sy, py), y2 = Math.max(d.sy, py);
  if (!d.moved) {
    d.moved = true;
    d.box = el('div', 'marquee');
    scroller.appendChild(d.box);
    // Keeps getting the pointer when it leaves the window.
    try { scroller.setPointerCapture(d.pointer); } catch (e) {}
  }
  d.box.style.left = x1 + 'px';
  d.box.style.top = y1 + 'px';
  d.box.style.width = (x2 - x1) + 'px';
  d.box.style.height = (y2 - y1) + 'px';
  var rect = { left: x1 - scroller.scrollLeft + sr.left, right: x2 - scroller.scrollLeft + sr.left, top: y1 - scroller.scrollTop + sr.top, bottom: y2 - scroller.scrollTop + sr.top };
  var now = new Set(d.site.hit ? d.site.hit(rect) : tilesIn(d.host, d.site, rect));
  var changed = false;
  d.covered.forEach(function (item) {
    if (now.has(item)) return;
    d.site.set(item, d.before.get(item));
    changed = true;
  });
  now.forEach(function (item) {
    if (d.covered.has(item)) return;
    if (!d.before.has(item)) d.before.set(item, d.site.get(item));
    if (d.site.get(item) !== d.on) { d.site.set(item, d.on); changed = true; }
  });
  d.covered = now;
  if (changed) {
    var hit = document.elementFromPoint(d.x, d.y), tile = hit && hit.closest(d.site.sel);
    d.site.done(tile && d.host.contains(tile) ? d.site.itemOf(tile) : undefined);
  }
}

// The items of the tiles in a list that a rectangle of window coordinates touches.
function tilesIn(host, site, rect) {
  var out = [];
  Array.prototype.forEach.call(host.querySelectorAll(site.sel), function (tile) {
    var r = tile.getBoundingClientRect();
    if (r.right < rect.left || r.left > rect.right || r.bottom < rect.top || r.top > rect.bottom) return;
    var item = site.itemOf(tile);
    if (item !== undefined) out.push(item);
  });
  return out;
}

// Scrolls while the pointer is near the top or bottom edge, faster the closer it is.
function tileDragScroll() {
  var d = tileDrag;
  if (!d || d.done) return;
  var scroller = $('scroller'), r = scroller.getBoundingClientRect(), v = 0;
  if (d.y < r.top + DRAG_EDGE) v = -(r.top + DRAG_EDGE - d.y);
  else if (d.y > r.bottom - DRAG_EDGE) v = d.y - (r.bottom - DRAG_EDGE);
  if (!v) { d.scrolling = false; return; }
  var top = scroller.scrollTop;
  scroller.scrollTop = top + Math.max(-40, Math.min(40, v / 2));
  if (scroller.scrollTop === top) { d.scrolling = false; return; }
  tileDragUpdate();
  if (tileDrag === d) requestAnimationFrame(tileDragScroll);
}

function tileDragEnd(ev) {
  var d = tileDrag;
  if (!d || ev.pointerId !== d.pointer) return;
  document.body.classList.remove('tile-drag');
  if (d.box) d.box.remove();
  if (!d.moved) { tileDrag = null; return; }
  // Kept until after the click that follows, so that click is swallowed above.
  d.done = true;
  setTimeout(function () { if (tileDrag === d) tileDrag = null; }, 0);
  try { $('scroller').releasePointerCapture(d.pointer); } catch (e) {}
}

// The timeline: the tiles are not all in the page, so the rectangle is worked
// out from the layout (rows and columns); items are the photos' positions.
$('sizer').classList.add('has-drag');
$('sizer').dragSite = function () {
  if (!state.selecting || !state.data || !state.data.ids) return null;
  var data = state.data;
  return {
    sel: '.cell',
    itemOf: function (tile) { return parseInt(tile.dataset.index, 10); },
    get: function (i) { return !!state.selected[data.ids[i]]; },
    set: function (i, on) { if (on) state.selected[data.ids[i]] = true; else delete state.selected[data.ids[i]]; },
    done: function (i) {
      if (state.data !== data) return;
      if (i !== undefined) state.anchor = i;
      showSelected();
    },
    hit: function (rect) {
      var sr = $('sizer').getBoundingClientRect(), out = [];
      var x1 = rect.left - sr.left, x2 = rect.right - sr.left, y1 = rect.top - sr.top, y2 = rect.bottom - sr.top;
      if (x2 < 0 || x1 > state.cols * state.cell || !state.rows.length) return out;
      var c1 = Math.max(0, Math.floor(x1 / state.cell)), c2 = Math.min(state.cols - 1, Math.floor(x2 / state.cell));
      for (var r = rowAt(Math.max(0, y1)); r < state.rows.length && state.rows[r].top <= y2; r++) {
        var row = state.rows[r];
        if (row.type !== 'r' || row.top + row.h < y1) continue;
        for (var i = row.start + c1; i <= Math.min(row.start + c2, row.end - 1); i++) out.push(i);
      }
      return out;
    },
  };
};
// Keep Shift-click on faces from selecting the page's text.
document.addEventListener('mousedown', function (ev) {
  if (ev.shiftKey && ev.target.closest && ev.target.closest('.face, .cface')) ev.preventDefault();
});

var ticking = false;
$('scroller').addEventListener('scroll', function () {
  if (ticking) return;
  ticking = true;
  requestAnimationFrame(function () { ticking = false; render(); });
}, { passive: true });

var resizeTimer = null;
window.addEventListener('resize', function () {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(function () {
    // Keep the same item at the top.
    var scroller = $('scroller');
    var r = state.rows[rowAt(Math.max(0, scroller.scrollTop - $('sizer').offsetTop))];
    var item = r ? (r.type === 'r' ? r.start : null) : null;
    var month = r && r.type === 'h' ? r.month : null;
    relayout();
    for (var i = 0; i < state.rows.length; i++) {
      var x = state.rows[i];
      if ((item != null && x.type === 'r' && x.start <= item && item < x.end) || (month != null && x.type === 'h' && x.month === month)) {
        scroller.scrollTop = x.top + $('sizer').offsetTop;
        break;
      }
    }
    render();
  }, 120);
});

function fillYears() {
  var sel = $('years');
  sel.textContent = '';
  var placeholder = el('option', '', tr('grid.year'));
  placeholder.value = '';
  sel.appendChild(placeholder);
  var seen = {};
  state.rows.forEach(function (r) {
    if (r.type !== 'h') return;
    var y = Math.floor(r.month / 100);
    if (!y || seen[y]) return;
    seen[y] = true;
    var o = el('option', '', String(y));
    o.value = y;
    sel.appendChild(o);
  });
  sel.hidden = Object.keys(seen).length < 2;
}

$('years').addEventListener('change', function () {
  var y = parseInt(this.value, 10);
  this.value = '';
  for (var i = 0; i < state.rows.length; i++) {
    var r = state.rows[i];
    if (r.type === 'h' && Math.floor(r.month / 100) === y) {
      $('scroller').scrollTop = r.top + $('sizer').offsetTop;
      return;
    }
  }
});

// ------------------------------------------------------------------ lightbox

var lb = { video: null, details: null, showFaces: false, hover: null, drawing: null, showText: false, textOpen: false, hoverLine: null, textBox: null, warm: [] };

function openLightbox(i) {
  state.open = i;
  // Opened from a text search: the words found are outlined and listed at once.
  lb.showText = lb.textOpen = state.filter.texts.length > 0;
  lb.hoverLine = null;
  $('lightbox').hidden = false;
  showItem();
}

function closeLightbox() {
  var hiddenChange = lb.hiddenDirty;
  lb.hiddenDirty = false;
  geoCleanup();
  stopDrawing();
  stopMedia();
  clearNode($('stage'));
  dropWarm();
  $('lightbox').hidden = true;
  // Opened from a page (face check), not from the grid: back to the page.
  if (lb.restore) { state.open = -1; lb.restore(); lb.restore = null; return; }
  // Keep the grid where the viewer left off, with the last item seen
  // focused (Tab and the arrow keys continue from there).
  var d = state.data, i = state.open;
  state.open = -1;
  if (!d || i < 0) return;
  focusCell(i, true);
  // Photos were hidden or shown meanwhile: the list changes now.
  if (hiddenChange && !state.filter.view) loadTimeline(false);
}

// The neighbours `showItem` asked for ahead.
function dropWarm() {
  (lb.warm || []).forEach(function (w) { w.removeAttribute('src'); });
  lb.warm = [];
}

function stopMedia() {
  Array.prototype.forEach.call($('stage').querySelectorAll('video'), function (v) {
    v.pause();
    v.removeAttribute('src');
    v.load();
  });
}

function step(delta) {
  var d = state.data;
  if (!d || state.open < 0) return;
  var i = state.open + delta;
  if (i < 0 || i >= d.count) return;
  state.open = i;
  showItem();
}

function showItem() {
  var d = state.data, i = state.open;
  var id = d.ids[i], kind = d.kinds[i];
  var stage = $('stage');
  var warm = [];
  stopMedia();
  // Leafing on quickly: the picture before stops loading.
  clearNode(stage);
  $('lb-prev').hidden = i <= 0;
  $('lb-next').hidden = i >= d.count - 1;
  $('lb-download').href = fileBase(id) + '/original?download=1';
  $('lb-live').hidden = !state.live[id];
  $('lb-fav').hidden = false;
  markLightboxHeart();
  markLightboxHide();
  markCollectionButton();
  applyAllowTrash();
  var rot = $('lb-rotate');
  rot.hidden = isAll() || kind === 'v';
  rot.disabled = kind !== 'j' && kind !== 'h' && kind !== 'p';
  setTip(rot, tr(rot.disabled ? 'lb.rotate_raw' : kind === 'j' ? 'lb.rotate_hint' : 'lb.rotate_view_hint'));
  $('lb-title').textContent = '';
  $('lb-text').hidden = true; // until the photo's details say it has text
  lb.hover = null;
  lb.hoverLine = null;
  stopDrawing();

  if (kind === 'v') {
    var v = el('video');
    v.controls = true;
    v.autoplay = true;
    v.playsInline = true;
    v.preload = 'metadata';
    v.poster = frames.cache[id] || thumbUrl(i);
    v.src = fileBase(id) + '/original';
    // An old camcorder format (.avi, .mkv, ...) the browser cannot decode: say so.
    v.addEventListener('error', function () {
      if (!v.parentNode || v.parentNode.querySelector('.lb-note')) return;
      v.parentNode.appendChild(el('div', 'lb-note', tr('lb.video_unplayable')));
    });
    stage.appendChild(v);
  } else {
    var img = el('img');
    img.alt = '';
    // No thumbnail placeholder: swapping it for the full image looked like a
    // zoom animation when stepping through photos with the arrow keys.
    img.onload = function () { drawFaces(); drawText(); };
    img.src = viewUrl(i);
    stage.appendChild(img);
    // Warm up the neighbours.
    [i + 1, i - 1].forEach(function (j) {
      if (j < 0 || j >= d.count || d.kinds[j] === 'v') return;
      var w = new Image();
      w.src = viewUrl(j);
      warm.push(w);
    });
  }
  // Those warmed for the photo before stop loading; the one that is now on the
  // stage goes on, as the stage asks for the same picture.
  dropWarm();
  lb.warm = warm;

  api(fileBase(id)).then(function (info) {
    if (state.open !== i) return;
    lb.details = info;
    markCollectionButton();
    $('lb-title').textContent = formatDate(info) + ' · ' + info.name;
    if (!$('lb-panel').hidden) renderPanel();
    drawFaces();
    syncTextButton();
    drawText();
  }).catch(function () {});
}

// Boxes around the faces (found by `shoebox recognize` or drawn by hand),
// with names, while the info panel is open and "Show boxes" is on; the face
// the pointer is on in the panel (or that was tapped there) is highlighted
// either way.
function drawFaces() {
  var stage = $('stage');
  stage.querySelectorAll('.face-box').forEach(function (b) { b.remove(); });
  var info = lb.details, img = stage.querySelector('img');
  if ($('lb-panel').hidden || !info || !info.faces || !img || img.hidden) return;
  // In the common timeline the item's id carries the drive; the details have the drive's own id.
  var openId = state.data.ids[state.open];
  if (info.id !== (isAll() ? openId % SPAN : openId)) return;
  var r = img.getBoundingClientRect(), s = stage.getBoundingClientRect();
  info.faces.forEach(function (f, k) {
    var hot = lb.hover === k;
    if (!lb.showFaces && !hot) return;
    f = turnedBox(f, info.view_turn || 0);
    var b = el('div', 'face-box' + (hot ? ' hot' : '') + (f.manual != null ? ' drawn' : '') + (f.species ? ' pet' : ''));
    b.style.left = (r.left - s.left + f.x * r.width) + 'px';
    b.style.top = (r.top - s.top + f.y * r.height) + 'px';
    b.style.width = (f.w * r.width) + 'px';
    b.style.height = (f.h * r.height) + 'px';
    if (f.person && f.state !== 'ignored') {
      b.appendChild(el('span', 'face-name', f.person.name + (f.state === 'confirmed' ? '' : '?')));
    }
    setTip(b, (f.species ? speciesName(f.species) + ', ' : '') + (f.kept ? tr('face.kept') : f.score != null ? tr('face.score', { score: f.score.toFixed(2) }) : tr('face.drawn')));
    stage.appendChild(b);
  });
}
window.addEventListener('resize', drawFaces);


// ---------------------------------------------------------- text in the photo (phase 9)

// The words of the search's text terms, folded like the server folds the
// lines (lower case, no accents, ß as ss, only letters and digits).
function foldMap(s) {
  var out = '', map = [];
  for (var i = 0; i < s.length; i++) {
    var c = s[i];
    var f = c === 'ß' ? 'ss' : c.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();
    f = f.replace(/[^\p{L}\p{N}]/gu, ' ');
    for (var j = 0; j < f.length; j++) { out += f[j]; map.push(i); }
  }
  return { text: out, map: map };
}
function searchWords() {
  var words = [];
  state.filter.texts.forEach(function (t) { foldMap(t).text.split(' ').forEach(function (w) { if (w) words.push(w); }); });
  return words;
}
// Does this line hold a word of the text search? (Its folded text, as the server stored it.)
function lineHit(line) {
  var words = searchWords();
  return words.length > 0 && words.some(function (w) { return (line.norm || '').indexOf(w) >= 0; });
}
// The line's text as nodes, the words of the search marked.
function markedText(text) {
  var words = searchWords(), box = document.createDocumentFragment();
  if (!words.length) { box.appendChild(document.createTextNode(text)); return box; }
  var folded = foldMap(text), ranges = [];
  words.forEach(function (w) {
    for (var at = folded.text.indexOf(w); at >= 0; at = folded.text.indexOf(w, at + w.length)) {
      ranges.push([folded.map[at], folded.map[at + w.length - 1] + 1]);
    }
  });
  ranges.sort(function (a, b) { return a[0] - b[0]; });
  var pos = 0;
  ranges.forEach(function (r) {
    if (r[1] <= pos) return;
    var from = Math.max(r[0], pos);
    if (from > pos) box.appendChild(document.createTextNode(text.slice(pos, from)));
    box.appendChild(el('mark', '', text.slice(from, r[1])));
    pos = r[1];
  });
  if (pos < text.length) box.appendChild(document.createTextNode(text.slice(pos)));
  return box;
}

// The top-bar button: only on photos that have text, dim until it is switched on.
function syncTextButton() {
  var b = $('lb-text'), info = lb.details;
  var has = !!(info && info.text && info.text.length);
  b.hidden = !has;
  b.classList.toggle('on', has && lb.showText);
  b.setAttribute('aria-pressed', has && lb.showText ? 'true' : 'false');
}

// Switch the outlines on the photo and the list in the panel together.
function toggleText() {
  var info = lb.details;
  if (!info || !info.text || !info.text.length) return;
  lb.showText = !lb.showText;
  lb.textOpen = lb.showText;
  if (lb.showText && $('lb-panel').hidden) { $('lb-panel').hidden = false; renderPanel(); }
  else if (lb.textBox) fillText(lb.textBox, info);
  lb.hoverLine = null;
  syncTextButton();
  drawText();
}
$('lb-text').appendChild(icon('scan'));
$('lb-text').onclick = toggleText;

// Boxes around the lines of text (fractions of the picture as the file has it, like the faces').
// While a text search is open only the lines that hold its words, thick and yellow;
// a line the pointer is on in the list is outlined either way. Hidden lines are not drawn.
function drawText() {
  var stage = $('stage');
  stage.querySelectorAll('.text-box').forEach(function (b) { b.remove(); });
  var info = lb.details, img = stage.querySelector('img');
  if (!lb.showText || !info || !info.text || !img || img.hidden) return;
  var openId = state.data.ids[state.open];
  if (info.id !== (isAll() ? openId % SPAN : openId)) return;
  var r = img.getBoundingClientRect(), s = stage.getBoundingClientRect();
  var searching = searchWords().length > 0;
  info.text.forEach(function (l, k) {
    if (l.hidden) return;
    var hit = searching && lineHit(l), hot = lb.hoverLine === k;
    if (searching && !hit && !hot) return;
    var f = turnedBox(l, info.view_turn || 0);
    var b = el('div', 'text-box' + (hit ? ' hit' : '') + (hot ? ' hot' : ''));
    b.style.left = (r.left - s.left + f.x * r.width) + 'px';
    b.style.top = (r.top - s.top + f.y * r.height) + 'px';
    b.style.width = (f.w * r.width) + 'px';
    b.style.height = (f.h * r.height) + 'px';
    b.title = l.text;
    // A click on a box finds its line in the list.
    b.onclick = function (ev) {
      ev.stopPropagation();
      lb.hoverLine = k;
      lb.textOpen = true;
      if (lb.textBox) fillText(lb.textBox, info);
      var row = lb.textBox && lb.textBox.querySelector('[data-line="' + k + '"]');
      if (row) row.scrollIntoView({ block: 'nearest' });
      drawText();
    };
    stage.appendChild(b);
  });
}
window.addEventListener('resize', drawText);

// The foldable "Recognized text" block of the info panel (folded until the
// button or the heading opens it).
function infoText(info) {
  if (!info.text || !info.text.length) { lb.textBox = null; return null; }
  var box = el('div', 'txt-sec');
  lb.textBox = box;
  fillText(box, info);
  return box;
}

function fillText(box, info) {
  box.textContent = '';
  var lines = info.text, shown = lines.filter(function (l) { return !l.hidden; });
  var head = el('div', 'txt-head');
  head.appendChild(el('span', 'chev', lb.textOpen ? '▾' : '▸'));
  head.appendChild(el('span', 'txt-title', tr('text.title')));
  head.appendChild(el('span', 'txt-count', String(shown.length)));
  var copy = el('button', 'txt-copy');
  copy.type = 'button';
  copy.appendChild(icon('copy'));
  copy.appendChild(document.createTextNode(tr('text.copy_all')));
  copy.onclick = function (ev) {
    ev.stopPropagation();
    copyText(shown.map(function (l) { return l.text; }).join('\n')).then(function (ok) {
      toast(ok ? tr('text.copied', { n: shown.length }) : tr('text.copy_failed'));
    });
  };
  head.appendChild(copy);
  head.onclick = function () { lb.textOpen = !lb.textOpen; fillText(box, info); };
  box.appendChild(head);
  if (!lb.textOpen) return;
  var list = el('div', 'txt-list');
  lines.forEach(function (l, k) {
    var row = el('div', 'txt-line' + (lineHit(l) && !l.hidden ? ' hit' : '') + (l.hidden ? ' gone' : ''));
    row.dataset.line = k;
    var t = el('span', 'tt');
    t.appendChild(markedText(l.text));
    row.appendChild(t);
    // Only a drive's own photo app decides what finds a photo (the common timeline only looks).
    if (!isAll()) {
      var x = el('button', 'x', l.hidden ? '↺' : '✕');
      x.type = 'button';
      setTip(x, tr(l.hidden ? 'text.show_again' : 'text.hide'));
      x.onclick = function (ev) {
        ev.stopPropagation();
        var id = info.id, hidden = !l.hidden;
        post(LIBAPI + '/files/' + id + '/text-hidden', { norm: l.norm, hidden: hidden }).then(function () {
          l.hidden = hidden;
          if (lb.details === info) { fillText(box, info); drawText(); }
        }).catch(function (e) { toast(e.message); });
      };
      row.appendChild(x);
    }
    row.addEventListener('mouseenter', function () { lb.hoverLine = k; drawText(); });
    row.addEventListener('mouseleave', function () { if (lb.hoverLine === k) { lb.hoverLine = null; drawText(); } });
    list.appendChild(row);
  });
  box.appendChild(list);
  var gone = lines.length - shown.length;
  if (gone) box.appendChild(el('div', 'txt-note', tr('text.hidden_note', { n: gone })));
}

// A face's box (fractions of the picture as the file has it) for a picture
// that shoebox shows turned by `q` quarter turns clockwise.
function turnedBox(f, q) {
  q = ((q % 4) + 4) % 4;
  if (!q) return f;
  var b = [f.x, f.y, f.w, f.h];
  var t = q === 1 ? [1 - b[1] - b[3], b[0], b[3], b[2]]
    : q === 2 ? [1 - b[0] - b[2], 1 - b[1] - b[3], b[2], b[3]]
    : [b[1], 1 - b[0] - b[2], b[3], b[2]];
  return Object.assign({}, f, { x: t[0], y: t[1], w: t[2], h: t[3] });
}

function viewUrl(i) {
  var d = state.data;
  return fileBase(d.ids[i]) + '/view?v=' + d.versions.substr(i * 8, 8);
}

function formatDate(info) {
  if (info.estimate) return formatEstimate(info.estimate);
  var s = info.taken || info.sort_date;
  if (!s) return '';
  var p = s.split(/[-T:]/).map(Number);
  var date = new Date(p[0], p[1] - 1, p[2], p[3] || 0, p[4] || 0);
  if (info.date_source === 'folder') return I18n.date(date, { month: 'long', year: 'numeric' });
  return I18n.dateTime(date, { dateStyle: 'medium', timeStyle: 'short' });
}

function formatBytes(n) {
  var units = ['B', 'KB', 'MB', 'GB'], u = 0;
  while (n >= 1000 && u < units.length - 1) { n /= 1000; u++; }
  return (u ? n.toFixed(1) : n) + ' ' + units[u];
}

// In the common timeline a photo is only looked at: what it is, where it is,
// and a way into its own drive for everything else (tags, faces, moving).
function renderPanelAll(info) {
  var panel = $('lb-panel');
  var id = state.data.ids[state.open];
  var lib = drives.allLibs[Math.floor(id / SPAN)];
  var dl = el('dl');
  var row = function (label, value) {
    if (!value) return;
    dl.appendChild(el('dt', '', label));
    var dd = el('dd');
    if (value instanceof Node) dd.appendChild(value); else dd.textContent = value;
    dl.appendChild(dd);
  };
  row(tr('info.date'), formatDate(info));
  var driveName = lib.volume || lib.name;
  var driveCell = row(tr('info.drive'), driveName);
  if (driveCell && state.data.flat) driveCell.appendChild(el('span', 'cmp-badge ' + (lib.role === 'backup' ? 'backup' : 'original'), tr(lib.role === 'backup' ? 'cmp.badge_backup' : 'cmp.badge_original')));
  if (!state.data.flat) alsoOnRow(dl, dl.lastChild, lib.id, id % SPAN);
  row(tr('info.path'), info.path);
  row(tr('info.size'), (info.width && info.height ? info.width + ' × ' + info.height + ' · ' : '') + formatBytes(info.size));
  row(tr('info.camera'), info.camera);
  if (info.hidden_folder) row(tr('info.hidden'), tr('hidden.via_folder', { folder: info.hidden_folder }));
  // Everything below is only shown: tags, favorites and people are changed on the photo's own drive.
  if (state.favs[id]) {
    var heart = el('span', 'heart');
    heart.innerHTML = HEART_ON;
    setTip(heart, tr('fav.readonly'));
    row(tr('info.favorite'), heart);
  }
  var tags = el('div', 'tags readonly');
  (info.tags || []).forEach(function (t) {
    if (t.source === 'user' && (isFavTag(t.name) || isHiddenTag(t.name))) return;
    var tag = el('span', t.source === 'folder' ? 'folder' : 'own');
    if (t.source === 'folder') tag.appendChild(folderIcon());
    tag.appendChild(document.createTextNode(t.name));
    tags.appendChild(tag);
  });
  if (tags.children.length) row(tr('info.tags'), tags);
  var faces = el('div', 'pfaces readonly');
  (info.faces || []).forEach(function (f, k) {
    if (!f.person || f.state === 'ignored') return;
    var name = f.state === 'confirmed' ? f.person.name : tr(f.state === 'maybe' ? 'info.guess_maybe' : 'info.guess', { name: f.person.name });
    var line = petLabel('div', 'pface', f.species, name);
    // As in the drive's own panel: pointing at (or tapping) a face shows its box.
    setTip(line, tr('info.show_where'));
    line.onclick = function () { lb.hover = lb.hover === k ? null : k; drawFaces(); };
    line.addEventListener('mouseenter', function () { lb.hover = k; drawFaces(); });
    line.addEventListener('mouseleave', function () { if (lb.hover === k) { lb.hover = null; drawFaces(); } });
    faces.appendChild(line);
  });
  if ((info.faces || []).length) {
    var show = toolButton(FRAME_ICON, tr(lb.showFaces ? 'info.hide_boxes' : 'info.show_boxes'));
    show.classList.toggle('on', !!lb.showFaces);
    show.onclick = function () { lb.showFaces = !lb.showFaces; renderPanel(); };
    var tools = el('div', 'ptools');
    tools.appendChild(show);
    faces.appendChild(tools);
    row(tr('info.people'), faces);
  }
  panel.appendChild(dl);
  var text = infoText(info);
  if (text) panel.appendChild(text);
  var actions = el('div', 'actions');
  var open = el('button', 'btn quiet', tr('info.open_in', { name: driveName }));
  setTip(open, tr('info.open_in_hint'));
  open.onclick = function () { switchLibrary(lib.id, 'folder=' + info.folder_id); };
  actions.appendChild(open);
  if (state.reveal) {
    var reveal = revealButton();
    reveal.onclick = function () { revealOn(lib.id, info.path); };
    actions.appendChild(reveal);
  }
  var copy = el('button', 'btn quiet', tr('reveal.copy_path'));
  copy.title = info.path;
  copy.onclick = function () { copyPath(info.path); };
  actions.appendChild(copy);
  panel.appendChild(actions);
  drawFaces();
}

// Phase 18: the other copies of the shown photo (same name, same content) on
// the drives of the common timeline, each a way into that drive's folder. Put
// in after `before` once the server answered.
var otherCopies = {};
function alsoOnRow(dl, before, library, file) {
  var key = library + ':' + file;
  var got = otherCopies[key] || (otherCopies[key] = api('/api/all/copies' + query({ library: library, file: file })));
  got.then(function (list) {
    if (!list.length) return;
    var dd = el('dd');
    list.forEach(function (c) {
      var cut = c.path.lastIndexOf('/');
      var b = el('button', 'link also-on', (c.volume || c.name) + (cut > 0 ? ' · ' + c.path.slice(0, cut) : ''));
      setTip(b, tr('info.also_on_hint'));
      b.onclick = function () { switchLibrary(c.library, 'folder=' + c.folder_id); };
      dd.appendChild(b);
    });
    var next = before ? before.nextSibling : null;
    dl.insertBefore(el('dt', '', tr('info.also_on')), next);
    dl.insertBefore(dd, next);
  }).catch(function () { delete otherCopies[key]; });
}

function renderPanel() {
  var info = lb.details, panel = $('lb-panel');
  panel.textContent = '';
  geoCleanup();
  if (!info) return;
  if (isAll()) { renderPanelAll(info); return; }
  var dl = el('dl');
  var row = function (label, value) {
    if (value == null || value === '') return null;
    dl.appendChild(el('dt', '', label));
    var dd = el('dd');
    if (value instanceof Node) dd.appendChild(value); else dd.textContent = value;
    dl.appendChild(dd);
    return dd;
  };
  infoDate(row, info);
  // The drive is named only when there is more than one (offline ones count: the
  // program was started with several); with a single drive the row says nothing new.
  if (drives.current && drives.list.length > 1) row(tr('info.drive'), drives.current.volume || drives.current.name);
  var nameCell = row(tr('info.name'), info.name);
  var cmpBadge = compareBadge(info.id);
  if (nameCell && cmpBadge) nameCell.appendChild(cmpBadge);
  var folder = info.path.indexOf('/') >= 0 ? info.path.slice(0, info.path.lastIndexOf('/')) : '';
  if (folder) {
    var fb = el('button', '', folder);
    fb.onclick = function () { closeLightbox(); setFilter({ folder: info.folder_id, tags: [], q: '' }); };
    row(tr('info.folder'), fb);
  }
  if (info.width && info.height) row(tr('info.size'), info.width + ' × ' + info.height + ' · ' + formatBytes(info.size));
  else row(tr('info.size'), formatBytes(info.size));
  if (info.duration_ms) row(tr('info.length'), tr('info.seconds', { n: Math.round(info.duration_ms / 1000) }));
  row(tr('info.camera'), info.camera);
  infoScreenshot(row, info);
  // Hidden by its folder: the eye in the top bar cannot show it again, the folder can.
  if (info.hidden_folder) row(tr('info.hidden'), tr('hidden.via_folder', { folder: info.hidden_folder }));
  infoTags(row, info);
  if (info.linked && info.linked.length) {
    var versions = el('div');
    info.linked.forEach(function (l) { versions.appendChild(el('div', '', l.path + (l.missing ? ' ' + tr('info.missing') : ''))); });
    row(tr('info.versions'), versions);
  }
  infoFaces(row, info);
  panel.appendChild(dl);
  var text = infoText(info);
  if (text) panel.appendChild(text);
  drawFaces();
  drawText();

  var actions = el('div', 'actions');
  infoReveal(actions, info);
  var move = el('button', 'btn quiet', tr('sel.move'));
  move.onclick = function () { moveDialog([info.id], function () { closeLightbox(); }); };
  if (!readOnly) actions.appendChild(move);
  panel.appendChild(actions);
  panel.appendChild(infoGeo(info));
}

// Moving to the trash is off until it is allowed in the settings (originals
// stay as they are by default): the photo view's trash icon and the
// selection bar's button only show then.
var allowTrash = false;
var trashCount = 0;
// A library whose index is kept next to the program (phase 16) is only read:
// no move, rotate, rename or delete; the CSS hides the buttons (body.read-only).
var readOnly = false;

function applyAllowTrash() {
  // The trash page stays reachable while something is in it (or while it is open).
  $('nav-trash').hidden = !allowTrash && !trashCount && state.filter.view !== 'trash';
  var d = state.data, i = state.open;
  $('lb-trash').hidden = !allowTrash || isAll() || i < 0 || !d;
  $('sel-trash').hidden = $('sel-trash-sep').hidden = !allowTrash;
}

function loadAllowTrash() {
  return api(LIBAPI + '/allow-trash').then(function (r) { allowTrash = !!r.allow; applyAllowTrash(); }).catch(function () {});
}

$('lb-trash').onclick = function () {
  var d = state.data, i = state.open;
  if (!allowTrash || !d || i < 0 || isAll()) return;
  trashDialog([d.ids[i]], function () { closeLightbox(); });
};

// The grid keeps showing the thumbnail it has until told otherwise. After a
// turn the new one is asked for once the user has stopped turning (a photo is
// often turned two or three times), so the server makes one thumbnail, not one
// per turn.
var TURN_THUMB_DELAY_MS = 1000;
var turned = { timer: null, ids: {} };

function thumbAfterTurn(id) {
  turned.ids[id] = true;
  clearTimeout(turned.timer);
  turned.timer = setTimeout(function () {
    var ids = turned.ids;
    turned.ids = {};
    var d = state.data;
    if (!d) return;
    Object.keys(state.rendered).forEach(function (k) {
      state.rendered[k].querySelectorAll('a.cell').forEach(function (a) {
        var i = +a.dataset.index, img = a.querySelector('img');
        if (img && ids[d.ids[i]]) img.src = thumbUrl(i);
      });
    });
  }, TURN_THUMB_DELAY_MS);
}

// Turns a JPEG on the drive (its EXIF orientation, two bytes, no loss), like
// the rotate button of the Finder's Quick Look; a HEIC or PNG is only shown
// turned by shoebox (its file stays as it is). `turns` is in quarter turns
// clockwise.
function rotateOpen(turns) {
  var d = state.data, i = state.open;
  if (readOnly || !d || i < 0 || isAll() || d.kinds[i] === 'v' || d.kinds[i] === 'r' || lb.rotating) return;
  lb.rotating = true;
  var id = d.ids[i];
  post(fileBase(id) + '/rotate', { turns: turns }).then(function (r) {
    lb.rotating = false;
    // The address of the picture carries the version, so the new one is
    // fetched; the grid's thumbnail follows a moment after the last turn.
    if (state.data === d && d.ids[i] === id) {
      d.versions = d.versions.slice(0, i * 8) + r.version + d.versions.slice(i * 8 + 8);
      if (state.open === i) showItem();
      thumbAfterTurn(id);
    }
    if (r.view_only && !lb.toldViewOnly) {
      lb.toldViewOnly = true;
      toast(tr('lb.rotate_view_only'));
    }
  }).catch(function (e) {
    lb.rotating = false;
    toast(e.message);
  });
}

$('lb-rotate').onclick = function (ev) { rotateOpen(ev && ev.altKey ? 1 : -1); };
$('lb-close').onclick = closeLightbox;
$('lb-prev').onclick = function () { step(-1); };
$('lb-next').onclick = function () { step(1); };
$('lb-info').onclick = function () {
  var p = $('lb-panel');
  p.hidden = !p.hidden;
  if (!p.hidden) renderPanel();
  else { geoCleanup(); drawFaces(); }
};
$('lb-live').onclick = function () {
  var d = state.data, id = d.ids[state.open], video = state.live[id];
  if (!video) return;
  var stage = $('stage');
  var still = stage.querySelector('img');
  // Pressed again while the motion plays: back to the still.
  var playing = stage.querySelector('video');
  if (playing) { stopMedia(); playing.remove(); if (still) still.hidden = false; return; }
  var v = el('video');
  v.autoplay = true;
  v.playsInline = true;
  v.src = fileBase(video) + '/original';
  v.onended = function () { v.remove(); if (still) still.hidden = false; };
  if (still) still.hidden = true;
  stage.appendChild(v);
};

// p: a movie pauses or plays on, a Live Photo plays its motion (or stops it,
// like the LIVE button). Space stays open/close on every item.
function playOpen() {
  var d = state.data, i = state.open;
  if (!d || i < 0) return;
  if (d.kinds[i] === 'v') {
    var v = $('stage').querySelector('video');
    if (!v) return;
    if (v.paused || v.ended) v.play().catch(function () {});
    else v.pause();
  } else if (!$('lb-live').hidden) $('lb-live').onclick();
}

// t: the info panel's tag field with its list of tags (the arrow keys pick
// one, Enter adds it; Esc closes the list, a second Esc leaves the field).
// Opens the panel if it is closed.
function tagOpen() {
  if (!lb.details || isAll()) return;
  var p = $('lb-panel');
  if (p.hidden) { p.hidden = false; renderPanel(); }
  var input = p.querySelector('.tags input');
  if (input) { input.focus(); return; }
  var add = p.querySelector('.tags button.add');
  if (add) add.click();
}

document.addEventListener('keydown', function (ev) {
  if (!$('modal').hidden) {
    if (ev.key === 'Escape') closeModal();
    return;
  }
  if ($('lightbox').hidden) {
    if (ev.key === 'Escape' && state.selecting) endSelection();
    if (state.selecting && (ev.key === 'b' || ev.key === 'B') && !ev.altKey && !ev.metaKey && !ev.ctrlKey && !/^(INPUT|TEXTAREA|SELECT)$/.test(ev.target.tagName)) {
      ev.preventDefault();
      toggleSelbarFocus();
      return;
    }
    var cell = document.activeElement;
    if (!cell || !cell.classList.contains('cell') || ev.altKey || ev.metaKey || ev.ctrlKey) return;
    if (/^Arrow(Left|Right|Up|Down)$/.test(ev.key)) {
      ev.preventDefault();
      moveFocus(cell, ev.key);
    } else if (ev.key === ' ') {
      // Like a click: opens the photo (or selects it while selecting).
      ev.preventDefault();
      cell.click();
    }
    return;
  }
  if (lb.drawing) {
    if (ev.key === 'Escape') { ev.preventDefault(); stopDrawing(); }
    return;
  }
  // Space closes the viewer, except on a focused video (play/pause).
  var onVideo = document.activeElement && document.activeElement.tagName === 'VIDEO';
  var plain = !ev.metaKey && !ev.ctrlKey && !ev.altKey && !/^(INPUT|TEXTAREA|SELECT)$/.test(ev.target.tagName);
  if (ev.key === 'Escape' || (ev.key === ' ' && !onVideo)) {
    ev.preventDefault();
    closeLightbox();
  }
  else if (ev.key === 'ArrowLeft') step(-1);
  else if (ev.key === 'ArrowRight') step(1);
  else if (ev.key === 'i') $('lb-info').onclick();
  else if ((ev.key === 'p' || ev.key === 'P') && plain) { ev.preventDefault(); playOpen(); }
  else if ((ev.key === 't' || ev.key === 'T') && plain) {
    ev.preventDefault(); // the letter must not land in the field that gets the focus
    tagOpen();
  }
  else if ((ev.key === 'w' || ev.key === 'W') && plain) toggleText();
  else if ((ev.key === 'f' || ev.key === 'F') && !ev.metaKey && !ev.ctrlKey && !ev.altKey) toggleFavoriteOpen();
  else if ((ev.key === 'h' || ev.key === 'H') && !ev.metaKey && !ev.ctrlKey && !ev.altKey) toggleHideOpen();
  else if ((ev.key === 'c' || ev.key === 'C') && !ev.metaKey && !ev.ctrlKey && !ev.altKey) toggleCollection();
  else if ((ev.key === 'r' || ev.key === 'R') && !ev.metaKey && !ev.ctrlKey && !ev.altKey) rotateOpen(ev.shiftKey ? 1 : -1);
});

// Swipe left/right on touch screens.
(function () {
  var x0 = null, y0 = null;
  var box = $('lightbox');
  box.addEventListener('touchstart', function (ev) {
    if (ev.touches.length !== 1) { x0 = null; return; }
    x0 = ev.touches[0].clientX;
    y0 = ev.touches[0].clientY;
  }, { passive: true });
  box.addEventListener('touchend', function (ev) {
    if (x0 === null || lb.drawing || ev.target.closest('.lb-panel, video, .draw-layer')) return;
    var dx = ev.changedTouches[0].clientX - x0, dy = ev.changedTouches[0].clientY - y0;
    x0 = null;
    if (Math.abs(dx) > 50 && Math.abs(dx) > 1.5 * Math.abs(dy)) step(dx < 0 ? 1 : -1);
    else if (dy > 90 && Math.abs(dy) > 2 * Math.abs(dx)) closeLightbox();
  });
})();

// ------------------------------------------------------------------ status

// One status request at a time: asks that come while one is under way (a
// stream event, a change, the page coming back) are answered by one more
// request after it, never by a pile of them.
var infoFlight = null, infoAgain = false, infoTimer = null;

function loadInfo() {
  if (infoFlight) { infoAgain = true; return infoFlight; }
  infoFlight = fetchInfo().then(function () {
    infoFlight = null;
    if (infoAgain) { infoAgain = false; loadInfo(); }
  });
  return infoFlight;
}

// Only while something runs (a scan, thumbnails, faces, grouping) the status
// is asked for again by itself; otherwise it follows from what the person does.
function watchProgress(info) {
  clearTimeout(infoTimer);
  infoTimer = null;
  var f = info.faces, c = info.clusters;
  if (info.busy || f.running || c.running || c.embedding) infoTimer = setTimeout(loadInfo, 5000);
}

function fetchInfo() {
  return api(LIBAPI + '/info').then(function (info) {
    $('title').textContent = info.name;
    state.reveal = revealLabel(info.reveal);
    state.exportOk = !!info.export;
    applyExportOk();
    readOnly = !!info.read_only;
    document.body.classList.toggle('read-only', readOnly);
    document.title = info.name + ' · shoebox';
    var parts = [
      trn('count.photos', info.photos),
      trn('count.videos', info.videos),
    ];
    if (info.missing) parts.push(tr('status.missing', { n: info.missing }));
    $('nav-trash').textContent = info.trash ? tr('side.trash_n', { n: info.trash }) : tr('side.trash');
    trashCount = info.trash || 0;
    applyAllowTrash();
    dates.needs = info.needs_date || 0;
    if ((state.hiddenCount || 0) !== (info.hidden || 0)) { state.hiddenCount = info.hidden || 0; paintHiddenRow(); }
    if (state.filter && state.filter.nodate) renderChips();
    $('nav-nodate').hidden = isAll() || (!dates.needs && !(state.filter && state.filter.nodate));
    if (info.thumbs_done < info.thumbs_total) {
      parts.push(tr('status.thumbs', { pct: Math.floor(100 * info.thumbs_done / info.thumbs_total) }));
    }
    var scan = info.jobs.filter(function (j) { return j.kind === 'scan'; })[0];
    if (info.busy) parts.push(tr('status.scanning'));
    else if (scan) parts.push(tr('status.last_scan', { date: I18n.date(new Date(scan.started_at * 1000)) }));
    if (!info.video_posters && info.videos) parts.push(tr('status.no_ffmpeg'));
    var f = info.faces, c = info.clusters;
    if (f.running) parts.push(tr('status.finding_faces', { pct: Math.floor(100 * f.done / Math.max(f.total, 1)) }));
    else if (f.done && f.done < f.total) parts.push(tr('status.faces_todo', { count: trn('count.photos', f.total - f.done) }));
    if (!f.running && f.pets_done && f.pets_done < f.total) parts.push(tr('status.pets_todo', { count: trn('count.photos', f.total - f.pets_done) }));
    state.textLines = info.text ? info.text.lines : 0;
    if (info.text && info.text.done && info.text.done < info.text.total) parts.push(tr('status.text_todo', { count: trn('count.photos', info.text.total - info.text.done) }));
    if (c.embedding) parts.push(tr('status.learning'));
    facesInfo(info);
    facesStream();
    $('status').textContent = parts.join(' · ');

    // Reload when a scan changed the index (but not while it is still busy).
    if (state.indexVersion !== null && info.index_version !== state.indexVersion && !info.busy) {
      reloadAll();
    }
    if (!info.busy) state.indexVersion = info.index_version;
    watchProgress(info);
  }).catch(function () {});
}

// ------------------------------------------------------------------ changes

// After a change: folders, the grid (keeping the scroll position) or the
// page that is open, and the status line.
function reloadAll() {
  if (isAll()) { if (state.filter.view) loadView(state.filter.view); else loadTimeline(false); return; }
  loadFolders();
  loadOwnTags();
  loadPeople();
  loadPlaces();
  // The unnamed clusters and a person's faces change their cards in place
  // after each action; loading them again would lose the place and what is
  // typed in other cards (the Unnamed page is refreshed in place instead).
  if (state.filter.view === 'unnamed') { scheduleUnnamedRefresh(); return; }
  if (state.filter.view === 'person') return;
  if (state.filter.view) loadView(state.filter.view);
  else loadTimeline(false);
}

function loadView(view) {
  if (view === 'duplicates') loadDuplicates();
  else if (view === 'settings') loadSettings();
  else if (view === 'faces' || view === 'pets') loadFaces();
  else if (view === 'people') { if (people.stale) loadPeople().then(loadPeoplePage); else loadPeoplePage(); }
  else if (view === 'unnamed') loadUnnamed();
  else if (view === 'person') loadPersonPage();
  else if (view === 'drives') loadDrivesPage();
  else if (view === 'locations') loadLocations();
  else if (view === 'text') loadTextCheck();
  else loadTrash();
}

function changed() {
  reloadAll();
  loadInfo();
}

var toastTimer = null;
function toast(text) {
  var t = $('toast');
  t.textContent = text;
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(function () { t.hidden = true; }, 4000);
}

// A small dialog. actions: [{label, cls, onclick(close)}]; onclick returns
// false to keep the dialog open.
function openModal(title, body, actions, cls) {
  document.querySelector('#modal .dialog').className = 'dialog' + (cls ? ' ' + cls : '');
  $('modal-title').textContent = title;
  var b = $('modal-body');
  b.textContent = '';
  if (typeof body === 'string') b.appendChild(el('p', '', body)); else b.appendChild(body);
  var a = $('modal-actions');
  a.textContent = '';
  var buttons = actions.map(function (act) {
    var btn = el('button', 'btn ' + (act.cls || ''), act.label);
    btn.onclick = function () { if (act.onclick && act.onclick(btn) === false) return; closeModal(); };
    a.appendChild(btn);
    return btn;
  });
  // The focus goes back where it came from when the dialog closes (keyboard use).
  if ($('modal').hidden) openModal.opener = document.activeElement;
  $('modal').hidden = false;
  var input = b.querySelector('input:not([type=file])');
  var preferred = actions.findIndex(function (act) { return act.focus; });
  if (input) input.focus(); else if (preferred >= 0) buttons[preferred].focus();
  return buttons;
}

function closeModal() {
  $('modal').hidden = true;
  var opener = openModal.opener;
  openModal.opener = null;
  if (opener && opener.isConnected && opener.getClientRects().length) opener.focus();
  if (closeModal.onclose) { var f = closeModal.onclose; closeModal.onclose = null; f(); }
}

// A line of a dialog that counts photos (date or location for a selection):
// its number in bold and, right after it, an info button that shows which
// photos it means (the first names; the server sends at most 20). Pointer,
// keyboard focus and a tap all open it. `bold` names the variable of the
// text that is the count ('n' or e.g. 'total').
var INFO_ICON = lineIcon('<circle cx="12" cy="12" r="8.5"/><path d="M12 11v5.5"/><circle cx="12" cy="7.8" r=".5"/>');
var countLines = 0;
function countLine(key, n, vars, bold, names) {
  var line = el('p', 'hint count-line');
  var count = bold === 'n' ? n : vars[bold];
  var sentinel = '\u0001';
  var marked = Object.assign({}, vars);
  marked[bold] = sentinel;
  var parts = trn(key, n, marked).split(sentinel);
  line.appendChild(document.createTextNode(parts[0]));
  if (parts.length > 1) {
    line.appendChild(namesBadge(count, names));
    line.appendChild(document.createTextNode(parts.slice(1).join('')));
  }
  return line;
}

function namesBadge(count, names) {
  var wrap = el('span', 'names');
  wrap.appendChild(el('b', '', String(count)));
  var btn = el('button', 'names-info');
  btn.type = 'button';
  btn.innerHTML = INFO_ICON;
  btn.setAttribute('aria-label', tr('app.names_show'));
  btn.setAttribute('aria-expanded', 'false');
  var pop = el('div', 'names-pop');
  pop.id = 'names-pop-' + (++countLines);
  pop.setAttribute('role', 'tooltip');
  pop.hidden = true;
  var list = el('ul');
  names.forEach(function (name) { list.appendChild(el('li', '', name)); });
  if (count > names.length) list.appendChild(el('li', 'more', trn('app.names_more', count - names.length)));
  pop.appendChild(list);
  btn.setAttribute('aria-describedby', pop.id);
  var pinned = false;
  var show = function (on) { pop.hidden = !on; btn.setAttribute('aria-expanded', on ? 'true' : 'false'); };
  wrap.addEventListener('mouseenter', function () { show(true); });
  wrap.addEventListener('mouseleave', function () { if (!pinned && document.activeElement !== btn) show(false); });
  btn.addEventListener('focus', function () { show(true); });
  btn.addEventListener('blur', function () { pinned = false; show(false); });
  btn.onclick = function () { pinned = !pinned; show(pinned || wrap.matches(':hover')); };
  wrap.appendChild(btn);
  wrap.appendChild(pop);
  return wrap;
}

// A click outside the dialog closes it, but only one that also began there: a
// press inside can end outside when the dialog changes size meanwhile (the
// name list under a field folds away when the field loses focus).
var modalPressed = false;
$('modal').addEventListener('mousedown', function (ev) { modalPressed = ev.target === this; });
$('modal').addEventListener('touchstart', function (ev) { modalPressed = ev.target === this; }, { passive: true });
$('modal').addEventListener('click', function (ev) { if (ev.target === this && modalPressed) closeModal(); modalPressed = false; });

// Tab stays in an open dialog: from the last control to the first and back,
// never on to the page behind it. Capturing, so it also works from fields
// that keep their keys to themselves.
document.addEventListener('keydown', function (ev) {
  if (ev.key !== 'Tab' || $('modal').hidden) return;
  var dialog = document.querySelector('#modal .dialog');
  var all = Array.prototype.filter.call(dialog.querySelectorAll('button, input, select, textarea, a[href], [tabindex]'), function (e) {
    return e.tabIndex >= 0 && !e.disabled && e.getClientRects().length;
  });
  if (!all.length) return;
  var first = all[0], last = all[all.length - 1], at = document.activeElement;
  if (!dialog.contains(at) || (ev.shiftKey ? at === first : at === last)) {
    ev.preventDefault();
    (ev.shiftKey ? last : first).focus();
  }
}, true);

// Shows what did not work, if anything.
function report(title, lines) {
  if (!lines || !lines.length) return;
  var ul = el('ul', 'filelist');
  lines.forEach(function (l) { ul.appendChild(el('li', '', l)); });
  openModal(title, ul, [{ label: tr('app.ok') }]);
}

function failed(e) { openModal(tr('app.failed'), String(e.message || e), [{ label: tr('app.ok') }]); }

// ------------------------------------------------------------------ selection

function startSelection() {
  if (isAll()) return;
  state.selecting = true;
  state.anchor = null;
  document.body.classList.add('selecting');
  updateSelbar();
}

function endSelection() {
  // Closed from the keyboard inside the bar: the focus goes back to the select button.
  if ($('selbar').contains(document.activeElement)) $('select').focus();
  state.selecting = false;
  state.selected = {};
  state.anchor = null;
  document.body.classList.remove('selecting');
  Array.prototype.forEach.call(document.querySelectorAll('.cell.sel'), function (c) { c.classList.remove('sel'); });
  updateSelbar();
}

function selectedIds() { return Object.keys(state.selected).map(Number); }

function toggleSelected(i, cell) {
  var id = state.data.ids[i];
  if (state.selected[id]) delete state.selected[id]; else state.selected[id] = true;
  cell.classList.toggle('sel', !!state.selected[id]);
  state.anchor = i;
  updateSelbar();
}

// Select (or deselect) the photos from index a to b, both included, in
// either order. The anchor stays, so another Shift-click changes the range end.
function selectRange(a, b, on) {
  var ids = state.data.ids;
  for (var i = Math.min(a, b); i <= Math.max(a, b); i++) {
    if (on) state.selected[ids[i]] = true; else delete state.selected[ids[i]];
  }
  if (state.anchor == null) state.anchor = a;
  showSelected();
}

// The ticks on the photos shown, and the bar, after the selection changed.
function showSelected() {
  var ids = state.data.ids;
  Array.prototype.forEach.call(document.querySelectorAll('.cell'), function (c) {
    c.classList.toggle('sel', !!state.selected[ids[parseInt(c.dataset.index, 10)]]);
  });
  updateSelbar();
}

function allSelected(start, end) {
  for (var i = start; i < end; i++) if (!state.selected[state.data.ids[i]]) return false;
  return end > start;
}

function updateSelbar() {
  var n = selectedIds().length;
  $('selbar').hidden = !state.selecting;
  var hint = tr(window.matchMedia('(pointer: fine)').matches ? 'sel.hint_mouse' : 'sel.hint_touch');
  $('sel-count').textContent = n ? trn('count.photos', n) : hint;
  $('sel-count').classList.toggle('hint', !n);
  Array.prototype.forEach.call(document.querySelectorAll('.sec .pick'), function (b) {
    b.textContent = tr(state.data && allSelected(parseInt(b.dataset.start, 10), parseInt(b.dataset.end, 10)) ? 'grid.deselect' : 'grid.select_all');
  });
  var total = state.data && state.data.ids ? state.data.ids.length : 0;
  $('sel-all').textContent = tr(total && n >= total ? 'sel.none' : 'sel.all');
  $('sel-all').disabled = !total;
  // Dimmed, not disabled: the tools stay in the keyboard order and keep their
  // label, so the bar does not jump while photos are picked.
  // The eye hides what is selected, or (all of it hidden already) shows it again.
  var allHidden = n > 0 && selectedIds().every(function (id) { return state.hids[id]; });
  $('sel-hide').innerHTML = allHidden ? SHOW_ICON : HIDE_ICON;
  setTip($('sel-hide'), tr(allHidden ? 'sel.unhide' : 'sel.hide'));
  $('sel-hide').setAttribute('aria-label', tr(allHidden ? 'sel.unhide' : 'sel.hide'));
  ['sel-tag', 'sel-untag', 'sel-date', 'sel-geo', 'sel-move', 'sel-export', 'sel-hide', 'sel-trash'].forEach(function (id) {
    $(id).setAttribute('aria-disabled', n ? 'false' : 'true');
  });
  var stop = $('selbar').querySelector('button[tabindex="0"]');
  if (selbarTools().indexOf(stop) < 0) selbarFocus($('sel-done'));
}

// The bar is one toolbar for the keyboard: Tab enters and leaves it, the arrow
// keys (and Home, End) go from tool to tool. The last tool used keeps the place.
function selbarTools() {
  return Array.prototype.filter.call($('selbar').querySelectorAll('button'), function (b) { return !b.hidden && !b.disabled; });
}

// B goes from the photos to the bar and back (the photos are many tab stops).
var selbarReturn = null;
function toggleSelbarFocus() {
  var bar = $('selbar');
  if (bar.contains(document.activeElement)) {
    if (selbarReturn && selbarReturn.isConnected) selbarReturn.focus();
    return;
  }
  selbarReturn = document.activeElement;
  var stop = bar.querySelector('button[tabindex="0"]');
  if (stop) stop.focus();
}

function selbarFocus(b) {
  Array.prototype.forEach.call($('selbar').querySelectorAll('button'), function (t) { t.tabIndex = t === b ? 0 : -1; });
}

Array.prototype.forEach.call($('selbar').querySelectorAll('[data-icon]'), function (b) {
  b.insertAdjacentHTML('afterbegin', SEL_ICONS[b.dataset.icon]);
});
$('lb-trash').innerHTML = TRASH_ICON;
selbarFocus($('sel-done'));
$('selbar').addEventListener('focusin', function (ev) { if (ev.target.tagName === 'BUTTON') selbarFocus(ev.target); });
$('selbar').addEventListener('keydown', function (ev) {
  var tools = selbarTools(), at = tools.indexOf(document.activeElement);
  if (at < 0) return;
  var to = { ArrowRight: at + 1, ArrowLeft: at - 1, Home: 0, End: tools.length - 1 }[ev.key];
  if (to === undefined) return;
  ev.preventDefault();
  tools[(to + tools.length) % tools.length].focus();
});
// A dimmed tool does nothing (nothing is selected yet).
$('selbar').addEventListener('click', function (ev) {
  var b = ev.target.closest('button');
  if (b && b.getAttribute('aria-disabled') === 'true') { ev.stopImmediatePropagation(); ev.preventDefault(); }
}, true);

$('select').innerHTML = SELECT_ICON;
$('select').onclick = function () {
  if (state.filter.view) setFilter({ folder: null, tags: [], people: [], q: '' });
  if (state.selecting) endSelection(); else startSelection();
};
// Every photo of the timeline as it is shown now (all months, e.g. all photos
// of a tag), or none again when everything is selected already.
$('sel-all').onclick = function () {
  var total = state.data.ids.length;
  selectRange(0, total - 1, selectedIds().length < total);
};
$('sel-done').onclick = endSelection;
$('sel-move').onclick = function () { moveDialog(selectedIds(), endSelection); };
$('sel-trash').onclick = function () { trashDialog(selectedIds(), endSelection); };
$('sel-tag').onclick = function () { addTagDialog(selectedIds()); };
$('sel-untag').onclick = function () { removeTagDialog(selectedIds()); };
$('sel-export').onclick = function () { openExport(selectedIds(), ''); };
$('sel-hide').onclick = function () {
  var ids = selectedIds();
  var on = !ids.every(function (id) { return state.hids[id]; });
  setHidden(ids, on).then(function (r) { if (r && r.files) endSelection(); });
};
$('sel-geo').onclick = function () { locationDialog(null, selectedIds()); };

// ------------------------------------------------------------------ move and trash

// " Empty folder removed: X" for the end of a toast, or nothing.
function removedFolders(r) {
  var gone = r.removed_folders || [];
  if (!gone.length) return '';
  return '. ' + tr(gone.length === 1 ? 'move.folders_removed' : 'move.folders_removed_many', { folders: gone.join(', ') });
}

function moveDialog(ids, done) {
  var body = el('div');
  body.appendChild(el('p', '', tr('move.text', { count: trn('count.photos', ids.length) })));
  var input = el('input');
  input.type = 'text';
  input.className = 'wide';
  input.setAttribute('list', 'folder-list');
  input.placeholder = tr('move.placeholder', { event: eventFolderName(2021, 3, tr('pattern.example_event')) });
  var current = state.filter.folder && state.folderById[state.filter.folder];
  if (current && current.path) input.value = current.path;
  body.appendChild(input);
  var keepLabel = el('label', 'check');
  var keep = el('input');
  keep.type = 'checkbox';
  keep.checked = false;
  keepLabel.appendChild(keep);
  keepLabel.appendChild(document.createTextNode(' ' + tr('move.keep_folder_tags')));
  setTip(keepLabel, tr('move.keep_hint'));
  body.appendChild(keepLabel);
  body.appendChild(el('p', 'hint', tr('move.hint')));
  // Moving into a hidden folder hides the photos; out of one shows them again.
  var hiddenNote = el('p', 'hint');
  body.appendChild(hiddenNote);
  var noteHidden = function () {
    var to = input.value.trim().replace(/^\/+|\/+$/g, '');
    var into = !!to && (state.folders || []).some(function (fo) { return fo.hidden_own && (to === fo.path || to.indexOf(fo.path + '/') === 0); });
    var from = !!(current && current.hidden);
    hiddenNote.textContent = into && !from ? tr('hidden.move_into') : !into && from && to ? tr('hidden.move_out') : '';
  };
  input.addEventListener('input', noteHidden);
  noteHidden();
  var error = el('p', 'error');
  body.appendChild(error);
  var go = function (btn) {
    var folder = input.value.trim();
    if (!folder) { error.textContent = tr('move.choose'); return false; }
    btn.disabled = true;
    post(LIBAPI + '/move', { ids: ids, folder: folder, keep_folder_tags: keep.checked }).then(function (r) {
      closeModal();
      if (done) done();
      toast(tr('move.done', { count: trn('count.files', r.files.length), folder: r.folder }) + removedFolders(r));
      report(tr('move.skipped'), r.skipped);
      changed();
    }).catch(function (e) { btn.disabled = false; error.textContent = e.message; });
    return false;
  };
  var buttons = openModal(tr('move.title'), body, [{ label: tr('app.cancel'), cls: 'quiet' }, { label: tr('move.title'), onclick: go }]);
  input.addEventListener('keydown', function (ev) { if (ev.key === 'Enter') go(buttons[1]); });
}

function trashDialog(ids, done) {
  openModal(
    tr('sel.trash'),
    tr('trash.text', { count: trn('count.photos', ids.length) }),
    [{ label: tr('app.cancel'), cls: 'quiet' }, {
      label: tr('sel.trash'), cls: 'danger', focus: true, onclick: function (btn) {
        btn.disabled = true;
        post(LIBAPI + '/trash', { ids: ids }).then(function (r) {
          closeModal();
          if (done) done();
          toast(tr('trash.done', { count: trn('count.files', r.files.length + r.sidecars) }) + removedFolders(r));
          report(tr('trash.skipped'), r.skipped);
          changed();
        }).catch(function (e) { closeModal(); failed(e); });
        return false;
      },
    }]
  );
}

function renameFolder(folder) {
  var body = el('div');
  var input = el('input');
  input.type = 'text';
  input.className = 'wide';
  input.value = folder.path;
  body.appendChild(input);
  body.appendChild(el('p', 'hint', tr('rename.hint', { pattern: patternLabel(eventPattern) })));
  var error = el('p', 'error');
  body.appendChild(error);
  var go = function (btn) {
    btn.disabled = true;
    post(LIBAPI + '/folders/' + folder.id + '/rename', { path: input.value }).then(function (r) {
      closeModal();
      toast(tr('rename.now', { path: r.path }));
      changed();
    }).catch(function (e) { btn.disabled = false; error.textContent = e.message; });
    return false;
  };
  var buttons = openModal(tr('rename.title'), body, [{ label: tr('app.cancel'), cls: 'quiet' }, { label: tr('rename.button'), onclick: go }]);
  input.addEventListener('keydown', function (ev) { if (ev.key === 'Enter') go(buttons[1]); });
}

// ------------------------------------------------------------------ chips input
// One bordered field that looks like a text input, with its values as chips
// inside it and, if `suggest` is given, a list of suggestions under it. It
// knows nothing about what the values are: everything specific is passed in.
//   values        chips to start with (strings)
//   placeholder   shown only while there is no chip and no text
//   label         aria-label of the field
//   removeLabel   function (value): aria-label of a chip's ✕
//   normalize     function (text): chip value or null (default: trim, NFC, empty is null)
//   same          function (a, b): when two values are one (default: NFC, ignoring case)
//   suggest       function (text, taken): rows [{ value, label, detail }] or a Promise of them
//   renderRow     function (row, text): node of one row (default: label with the typed part bold)
//   freeRow       function (text): text of the last row ("Add “xyz”"); null: only suggested values
//                 can become chips; left out: typed text becomes a chip without a row
//   noneText      function (text): a line shown when text was typed and nothing matches
//   delay, maxRows, onChange(values)
// Returns { el, values(), set(list), add(value), focus(), destroy() }.
var chipsSeq = 0;

function chipsInput(o) {
  var values = (o.values || []).slice();
  var normalize = o.normalize || function (t) { t = String(t).normalize('NFC').trim(); return t || null; };
  var same = o.same || function (a, b) { return a.normalize('NFC').toLowerCase() === b.normalize('NFC').toLowerCase(); };
  var delay = o.delay == null ? 120 : o.delay, maxRows = o.maxRows || 8;
  var freeText = o.freeRow !== null;
  var id = 'chips-' + (++chipsSeq);

  var wrap = el('div', 'chips-wrap');
  var field = el('div', 'chips-field');
  var input = el('input', 'chips-text');
  input.type = 'text';
  input.setAttribute('enterkeyhint', 'done');
  input.setAttribute('autocomplete', 'off');
  input.setAttribute('autocapitalize', 'off');
  input.setAttribute('autocorrect', 'off');
  input.spellcheck = false;
  if (o.label) input.setAttribute('aria-label', o.label);
  var list = null, rows = [], at = -1, ask = 0, timer = null;
  if (o.suggest) {
    input.setAttribute('role', 'combobox');
    input.setAttribute('aria-autocomplete', 'list');
    input.setAttribute('aria-expanded', 'false');
    input.setAttribute('aria-controls', id);
    list = el('div', 'chips-list');
    list.id = id;
    list.setAttribute('role', 'listbox');
    list.hidden = true;
  }

  var has = function (v) { return values.some(function (x) { return same(x, v); }); };
  var closeList = function () {
    ask++;
    clearTimeout(timer);
    if (!list) return;
    list.hidden = true;
    list.classList.remove('up');
    rows = [];
    at = -1;
    input.setAttribute('aria-expanded', 'false');
    input.removeAttribute('aria-activedescendant');
  };
  var mark = function (i) {
    at = i;
    rows.forEach(function (r, k) {
      r.node.classList.toggle('at', k === i);
      r.node.setAttribute('aria-selected', k === i ? 'true' : 'false');
    });
    if (i >= 0) {
      input.setAttribute('aria-activedescendant', rows[i].node.id);
      rows[i].node.scrollIntoView({ block: 'nearest' });
    } else {
      input.removeAttribute('aria-activedescendant');
    }
  };
  var draw = function () {
    Array.prototype.slice.call(field.querySelectorAll('.chip')).forEach(function (c) { field.removeChild(c); });
    values.forEach(function (v, i) {
      var chip = el('span', 'chip', v);
      var x = el('button', '', '✕');
      x.type = 'button';
      if (o.removeLabel) x.setAttribute('aria-label', o.removeLabel(v));
      x.onclick = function (ev) { ev.stopPropagation(); remove(i); input.focus(); };
      chip.appendChild(x);
      field.insertBefore(chip, input);
    });
    input.placeholder = values.length ? '' : (o.placeholder || '');
  };
  var changed = function () { draw(); if (o.onChange) o.onChange(values.slice()); };
  var add = function (text) {
    var v = normalize(text);
    if (v == null || has(v)) return false;
    values.push(v);
    changed();
    return true;
  };
  var remove = function (i) {
    values.splice(i, 1);
    changed();
    if (list && document.activeElement === input) refill();
  };

  // Puts the list under the field, or over it when the window has no room below.
  var place = function () {
    list.classList.remove('up');
    var r = field.getBoundingClientRect(), h = list.offsetHeight;
    if (r.bottom + h + 12 > window.innerHeight && r.top > h + 12) list.classList.add('up');
  };
  var show = function (found, text) {
    list.textContent = '';
    rows = [];
    at = -1;
    var typed = normalize(text);
    var shown = (found || []).filter(function (r) { return !has(r.value); }).slice(0, maxRows);
    shown.forEach(function (r) { rows.push({ value: r.value, node: null, row: r }); });
    if (typed != null && o.freeRow && !has(typed) && !shown.some(function (r) { return same(r.value, typed); })) {
      rows.push({ value: typed, node: null, free: true });
    }
    if (!shown.length && typed != null && o.noneText) list.appendChild(el('div', 'chips-none', o.noneText(text)));
    rows.forEach(function (r, i) {
      var node;
      if (r.free) {
        node = el('div', 'chips-row add', o.freeRow(text));
      } else if (o.renderRow) {
        node = o.renderRow(r.row, text);
      } else {
        node = el('div', 'chips-row');
        var label = String(r.row.label == null ? r.row.value : r.row.label), t = text.trim();
        var k = t ? label.toLowerCase().indexOf(t.toLowerCase()) : -1;
        var main = el('span', 'main');
        if (k >= 0) {
          main.appendChild(document.createTextNode(label.slice(0, k)));
          main.appendChild(el('b', '', label.slice(k, k + t.length)));
          main.appendChild(document.createTextNode(label.slice(k + t.length)));
        } else {
          main.textContent = label;
        }
        node.appendChild(main);
        if (r.row.detail) node.appendChild(el('small', '', r.row.detail));
      }
      node.id = id + '-' + i;
      node.setAttribute('role', 'option');
      node.setAttribute('aria-selected', 'false');
      // The field keeps the focus, so a click does not end the typing.
      node.addEventListener('mousedown', function (ev) { ev.preventDefault(); });
      node.addEventListener('click', function () { pick(i); });
      r.node = node;
      list.appendChild(node);
    });
    list.hidden = !list.children.length;
    input.setAttribute('aria-expanded', list.hidden ? 'false' : 'true');
    if (!list.hidden) place();
  };
  var refill = function () {
    var mine = ++ask, text = input.value;
    Promise.resolve(o.suggest(text, values.slice())).then(function (found) {
      // An answer to an earlier question is out of date.
      if (mine !== ask || document.activeElement !== input) return;
      show(found, text);
    }).catch(function () { if (mine === ask) closeList(); });
  };
  var pick = function (i) {
    var r = rows[i];
    if (!r) return;
    add(r.value);
    input.value = '';
    refill();
    input.focus();
  };
  // Typed text becomes chips; commas and line breaks split it.
  var commit = function () {
    var parts = input.value.split(/[,\n\r]+/);
    input.value = '';
    if (!freeText) return;
    parts.forEach(add);
  };

  input.addEventListener('keydown', function (ev) {
    var open = list && !list.hidden;
    if (ev.key === 'Enter') {
      ev.preventDefault();
      if (open && at >= 0) pick(at);
      else if (input.value.trim() && freeText) { commit(); if (list) refill(); }
    } else if (ev.key === ',' && freeText) {
      ev.preventDefault();
      commit();
      if (list) refill();
    } else if (ev.key === 'Backspace' && !input.value && values.length) {
      remove(values.length - 1);
    } else if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
      if (!list) return;
      ev.preventDefault();
      if (!open) { refill(); return; }
      var n = rows.length;
      if (n) mark(ev.key === 'ArrowDown' ? (at + 1) % n : (at <= 0 ? n - 1 : at - 1));
    } else if (ev.key === 'Escape' && open) {
      ev.preventDefault();
      ev.stopPropagation();
      closeList();
    }
  });
  input.addEventListener('input', function () {
    if (/[,\n\r]/.test(input.value) && freeText) commit();
    if (!list) return;
    clearTimeout(timer);
    timer = setTimeout(refill, delay);
  });
  // A single-line field drops line breaks from pasted text, so they are read here.
  input.addEventListener('paste', function (ev) {
    var text = ev.clipboardData ? ev.clipboardData.getData('text') : '';
    if (!freeText || !/[,\n\r]/.test(text)) return;
    ev.preventDefault();
    (input.value + text).split(/[,\n\r]+/).forEach(add);
    input.value = '';
    if (list) refill();
  });
  input.addEventListener('focus', function () { if (list) refill(); });
  input.addEventListener('blur', function () {
    // Typed text is never lost.
    if (input.value.trim() && freeText) commit();
    closeList();
  });
  field.addEventListener('click', function (ev) { if (ev.target === field) input.focus(); });

  field.appendChild(input);
  wrap.appendChild(field);
  if (list) wrap.appendChild(list);
  draw();
  return {
    el: wrap,
    values: function () { return values.slice(); },
    set: function (next) { values = next.slice(); draw(); },
    add: function (v) { return add(v); },
    focus: function () { input.focus(); },
    destroy: function () { clearTimeout(timer); ask++; if (wrap.parentNode) wrap.parentNode.removeChild(wrap); },
  };
}

// ------------------------------------------------------------------ duplicates page

var DUP_KINDS_IDS = ['identical', 'resolution', 'edited', 'similar'];
var dupState = { syncs: [], groups: [], shown: 0, marked: {}, seen: {}, types: dupTypes() };
var DUP_KINDS = [
  { id: 'identical', get label() { return tr('dups.kind.identical'); }, get hint() { return tr('dups.kind.identical.hint'); } },
  { id: 'resolution', get label() { return tr('dups.kind.resolution'); }, get hint() { return tr('dups.kind.resolution.hint'); } },
  { id: 'edited', get label() { return tr('dups.kind.edited'); }, get hint() { return tr('dups.kind.edited.hint'); } },
  { id: 'similar', get label() { return tr('dups.kind.similar'); }, get hint() { return tr('dups.kind.similar.hint'); } },
];

// Which kinds of duplicates are shown (remembered in this browser).
function dupTypes() {
  var t = { identical: true, resolution: true, edited: true, similar: true };
  try {
    var saved = JSON.parse(localStorage.getItem('dupTypes') || 'null');
    if (saved) DUP_KINDS_IDS.forEach(function (k) { if (saved[k] === false) t[k] = false; });
  } catch (e) { /* storage may be blocked */ }
  return t;
}

function visibleGroups() {
  return dupState.groups.filter(function (g) { return dupState.types[g.kind]; });
}

var PAGE_GROUPS = 30;

// With several drives the page has two lists: copies on this drive (decided
// per group) and copies on different drives. A backup drive holds copies on
// purpose, so drives marked as backups (or looking like one) are not in the
// second list; "All drives" says which is which.
var dupTab = 'here';

function addDupTabs(page) {
  if (drives.list.length < 2 || isAll()) return;
  var tabs = el('div', 'tabs underline');
  [['here', tr('dups.tab.here')], ['across', tr('dups.tab.across')]].forEach(function (t) {
    var b = el('button', dupTab === t[0] ? 'on' : '', t[1]);
    b.onclick = function () { dupTab = t[0]; loadDuplicates(); };
    tabs.appendChild(b);
  });
  page.insertBefore(tabs, page.children[1] || null);
}

function loadDuplicates() {
  if (isAll()) dupTab = 'across';
  if (drives.list.length > 1 && dupTab === 'across') return loadDuplicatesAcross();
  return loadDuplicatesHere();
}

function loadDuplicatesAcross() {
  var page = $('page');
  clearNode(page);
  page.appendChild(el('h2', '', tr('dups.title')));
  addDupTabs(page);
  return renderDuplicatesAcross(page, function () { return state.filter.view === 'duplicates'; }, false);
}

// The copies on different drives, into `page`; `alive` says whether it is still
// the page on screen when the answer arrives. Inside "Manage drives" (`embedded`)
// the link back to the drive roles is left out: they are right above.
// Same content, so one picture per photo and one row per copy; ticked copies
// (key `drive:id`) go to the trash of their own drive, one copy per photo stays.
var crossState = { groups: [], marked: {}, syncs: [], reload: null, bar: null, pick: null, ignore: null, labels: {} };

function crossKey(f) { return f.library + ':' + f.id; }

// The disk name where it is known, else the library's name; two drives that
// would read the same get the library name added.
function crossLabels(groups) {
  var seen = {}, label = {};
  groups.forEach(function (g) { g.files.forEach(function (f) { seen[f.library] = f; }); });
  var count = {};
  Object.keys(seen).forEach(function (lib) { var n = seen[lib].volume || seen[lib].name; count[n] = (count[n] || 0) + 1; });
  Object.keys(seen).forEach(function (lib) {
    var f = seen[lib], n = f.volume || f.name;
    label[lib] = count[n] > 1 && f.volume ? f.volume + ' · ' + f.name : n;
  });
  return label;
}

// "Ignore folders": folder names that are on several drives on purpose. A photo
// with a copy in a folder of such a name is left out of the list (the server
// does it, `ignore` in the request). Kept by this browser, like the Maps setting.
var DUP_IGNORE_KEY = 'shoebox-dup-ignore';

function dupIgnoreNames() {
  try {
    var saved = JSON.parse(localStorage.getItem(DUP_IGNORE_KEY) || '[]');
    return Array.isArray(saved) ? saved.filter(function (n) { return typeof n === 'string' && n.trim(); }) : [];
  } catch (e) { return []; }
}

function saveDupIgnoreNames(names) {
  try { localStorage.setItem(DUP_IGNORE_KEY, JSON.stringify(names)); } catch (e) { /* storage may be blocked */ }
}

function dupIgnoreQuery(names, first) {
  return names.map(function (n, i) { return (i || first ? '&' : '') + 'ignore=' + encodeURIComponent(n); }).join('');
}

// The field with the note under it. `onChange` runs after every change of the
// names; `note.set(hidden)` writes how many photos are left out.
function ignoreFoldersField(onChange) {
  var box = el('div', 'ignore-folders');
  var head = el('label', 'lbl', tr('dups.ignore.label'));
  var hidden = 0;
  var note = el('p', 'note');
  var drawNote = function () {
    note.textContent = field.values().length ? tr('dups.ignore.note', { count: trn('count.photos', hidden) }) : tr('dups.ignore.hint');
  };
  var driveLabels = function (r) {
    return r.drives.map(function (d) { return crossState.labels[d.library] || d.volume || d.name; }).join(', ');
  };
  var field = chipsInput({
    values: dupIgnoreNames(),
    placeholder: tr('dups.ignore.placeholder'),
    label: tr('dups.ignore.label'),
    removeLabel: function (v) { return tr('dups.ignore.remove', { name: v }); },
    suggest: function (text, taken) {
      return api('/api/all/duplicates/folders?q=' + encodeURIComponent(text) + dupIgnoreQuery(taken, true)).then(function (rows) {
        return rows.map(function (r) {
          return { value: r.name, label: r.name, detail: trn('count.photos', r.photos) + ' · ' + tr('dups.ignore.suggest_on', { drives: driveLabels(r) }) };
        });
      });
    },
    freeRow: function (text) { return tr('dups.ignore.suggest_add', { name: text.trim() }); },
    noneText: function () { return tr('dups.ignore.suggest_none'); },
    onChange: function (names) { saveDupIgnoreNames(names); drawNote(); onChange(names); },
  });
  head.addEventListener('click', function () { field.focus(); });
  box.appendChild(head);
  box.appendChild(field.el);
  box.appendChild(note);
  drawNote();
  return {
    el: box,
    add: function (name) { return field.add(name); },
    names: function () { return field.values(); },
    setHidden: function (n) { hidden = n; drawNote(); },
  };
}

function renderDuplicatesAcross(page, alive, embedded) {
  crossState.reload = embedded
    ? function () { clearNode(page); return renderDuplicatesAcross(page, alive, true); }
    : loadDuplicatesAcross;
  var asked = 0;
  var fill;
  var ignore = ignoreFoldersField(function () { fill(); });
  crossState.ignore = ignore;
  page.appendChild(ignore.el);
  var sub = el('p', 'sub', tr('dups.looking'));
  page.appendChild(sub);
  var box = el('div');
  page.appendChild(box);
  // The list again, e.g. after a name was added or taken away; the field stays.
  fill = function () {
    var mine = ++asked;
    return api('/api/all/duplicates?limit=200' + dupIgnoreQuery(ignore.names(), true)).then(function (r) {
      if (!alive() || mine !== asked) return;
      clearNode(box);
      ignore.setHidden(r.hidden_groups || 0);
      sub.textContent = r.total_groups
        ? tr('dups.across.some', { count: trn('count.photos_exist', r.total_groups), drives: r.compared.join(', ') })
        : r.compared.length > 1 ? tr('dups.across.none_in', { drives: r.compared.join(', ') }) : tr('dups.across.none');
      r.excluded.forEach(function (x) {
        var b = el('div', 'banner');
        b.appendChild(document.createTextNode(tr('dups.excluded', { name: x.name, reason: x.reason })));
        if (!embedded) {
          var go = el('button', 'link', tr('dups.decide'));
          go.onclick = function () { showView('drives'); };
          b.appendChild(go);
        }
        box.appendChild(b);
      });
      crossState.groups = r.groups;
      crossState.marked = {};
      crossState.syncs = [];
      var labels = crossLabels(r.groups);
      crossState.labels = labels;
      r.groups.forEach(function (g) { box.appendChild(crossGroupNode(g, labels)); });
      if (r.total_groups > r.groups.length) box.appendChild(el('p', 'sub', tr('dups.first_of', { shown: r.groups.length, total: r.total_groups })));
      var bar = el('div', 'dup-bar');
      bar.hidden = !r.groups.length;
      box.appendChild(bar);
      crossState.bar = bar;
      updateCrossBar(labels);
    }).catch(failed);
  };
  return fill();
}

function crossMarkedCount() {
  var n = 0;
  crossState.groups.forEach(function (g) { g.files.forEach(function (f) { if (crossState.marked[crossKey(f)]) n++; }); });
  return n;
}

// Ticks every copy on drive `lib`, but never the last copy of a photo.
function crossMarkDrive(lib) {
  crossState.groups.forEach(function (g) {
    var open = g.files.filter(function (f) { return !crossState.marked[crossKey(f)]; }).length;
    g.files.forEach(function (f) {
      if (f.library !== lib || crossState.marked[crossKey(f)] || open <= 1) return;
      crossState.marked[crossKey(f)] = true;
      open--;
    });
  });
  crossState.syncs.forEach(function (sync) { sync(); });
}

// "N marked", "Mark all copies on [drive]", Clear, Move to trash.
function updateCrossBar(labels) {
  var bar = crossState.bar;
  if (!bar) return;
  bar.textContent = '';
  var n = crossMarkedCount();
  bar.appendChild(el('span', '', tr('dups.marked', { count: trn('count.copies', n) })));
  bar.appendChild(el('span', 'meta', tr('dups.cross.mark_on')));
  var pick = el('select');
  Object.keys(labels).forEach(function (lib) {
    var o = el('option', '', labels[lib]);
    o.value = lib;
    pick.appendChild(o);
  });
  if (crossState.pick && labels[crossState.pick]) pick.value = crossState.pick;
  pick.onchange = function () { crossState.pick = pick.value; };
  bar.appendChild(pick);
  var mark = el('button', 'btn quiet', tr('dups.cross.mark'));
  setTip(mark, tr('dups.cross.mark_hint'));
  mark.onclick = function () { crossState.pick = pick.value; crossMarkDrive(pick.value); };
  bar.appendChild(mark);
  var clear = el('button', 'btn quiet', tr('dups.clear'));
  setTip(clear, tr('dups.clear_hint'));
  clear.disabled = n === 0;
  clear.onclick = function () { crossState.marked = {}; crossState.syncs.forEach(function (sync) { sync(); }); };
  bar.appendChild(clear);
  var del = el('button', 'btn danger', tr('sel.trash'));
  del.disabled = n === 0;
  del.onclick = trashCrossMarked;
  bar.appendChild(del);
}

// One request per drive; each file goes to the trash of the drive it is on,
// and only if it is still in that drive's index.
function trashCrossMarked() {
  var perDrive = {}, total = 0;
  crossState.groups.forEach(function (g) {
    var keep = g.files.filter(function (f) { return !crossState.marked[crossKey(f)]; }).length;
    if (!keep) return;
    g.files.forEach(function (f) {
      if (!crossState.marked[crossKey(f)]) return;
      (perDrive[f.library] = perDrive[f.library] || []).push(f.id);
      total++;
    });
  });
  if (!total) return;
  var libs = Object.keys(perDrive);
  openModal(tr('sel.trash'), tr('dups.cross.delete_text', { count: trn('count.copies', total) }), [
    { label: tr('app.cancel'), cls: 'quiet' },
    { label: tr('sel.trash'), cls: 'danger', focus: true, onclick: function (btn) {
      btn.disabled = true;
      var done = 0, skipped = [], folders = [];
      var send = function (i) {
        if (i >= libs.length) return Promise.resolve();
        return post('/api/lib/' + libs[i] + '/trash', { ids: perDrive[libs[i]] }).then(function (r) {
          done += r.files.length + r.sidecars;
          (r.skipped || []).forEach(function (x) { skipped.push(x); });
          (r.removed_folders || []).forEach(function (x) { folders.push(x); });
          return send(i + 1);
        });
      };
      send(0).then(function () {
        closeModal();
        toast(tr('dups.cross.done', { count: trn('count.files', done) }) + removedFolders({ removed_folders: folders }));
        report(tr('trash.skipped'), skipped);
        crossState.reload();
      }).catch(function (e) { closeModal(); failed(e); crossState.reload(); });
      return false;
    } },
  ]);
}

function crossGroupNode(g, labels) {
  var box = el('div', 'group cross');
  var first = g.files[0];
  var base = '/api/lib/' + first.library + '/files/' + first.id;
  var a = el('a', 'thumb');
  a.href = base + (first.kind === 'video' ? '/original' : '/view') + '?v=' + first.version;
  a.target = '_blank';
  a.rel = 'noopener';
  var img = el('img');
  img.alt = '';
  img.loading = 'lazy';
  img.src = base + '/thumb?v=' + first.version;
  a.appendChild(img);
  box.appendChild(a);
  var main = el('div', 'main');
  var baseName = function (f) { return f.path.split('/').pop(); };
  var sameName = g.files.every(function (f) { return baseName(f) === baseName(first); });
  main.appendChild(el('div', 'name', baseName(first)));
  main.appendChild(el('div', 'meta', tr('dups.same_on', { count: trn('count.drives', new Set(g.files.map(function (f) { return f.library; })).size) }) + ' · ' + formatBytes(g.size)));
  var boxes = [], rows = [];
  // At least one copy stays: the last unticked box is disabled.
  var refresh = function () {
    var open = boxes.filter(function (b) { return !b.checked; });
    boxes.forEach(function (b) { b.disabled = !b.checked && open.length === 1; });
  };
  g.files.forEach(function (f) {
    var row = el('div', 'copy-row');
    var cb = el('input');
    cb.type = 'checkbox';
    setTip(cb, tr('dups.delete_copy'));
    var state = el('span', 'state');
    var sync = function () {
      cb.checked = !!crossState.marked[crossKey(f)];
      row.classList.toggle('marked', cb.checked);
      state.textContent = tr(cb.checked ? 'dups.cross.goes' : 'dups.cross.stays');
      refresh();
      updateCrossBar(labels);
    };
    cb.onchange = function () {
      if (cb.checked) crossState.marked[crossKey(f)] = true; else delete crossState.marked[crossKey(f)];
      sync();
    };
    boxes.push(cb);
    rows.push(sync);
    row.appendChild(cb);
    var where = el('div', 'where');
    var disk = el('span', 'disk', labels[f.library]);
    where.appendChild(disk);
    if (f.volume) where.appendChild(el('span', 'meta', ' · ' + tr('dups.cross.folder', { name: f.name })));
    var folder = f.path.indexOf('/') >= 0 ? f.path.slice(0, f.path.lastIndexOf('/')) : tr('dups.top_level');
    var fb = el('button', 'folder', folder + (sameName ? '' : ' / ' + baseName(f)));
    setTip(fb, tr('dups.show_in', { name: f.name }));
    fb.onclick = function () { switchLibrary(f.library, 'q=' + encodeURIComponent(baseName(f))); };
    var line = el('div');
    line.appendChild(fb);
    // The folder the copy lies in, by name: any photo with a copy in a folder of that name is left out.
    var leaf = f.path.indexOf('/') >= 0 ? f.path.slice(0, f.path.lastIndexOf('/')).split('/').pop() : '';
    if (leaf && crossState.ignore) {
      var ib = el('button', 'folder ignore-this', tr('dups.ignore.add'));
      setTip(ib, tr('dups.ignore.add_hint', { name: leaf }));
      ib.onclick = function () { crossState.ignore.add(leaf); };
      line.appendChild(el('span', 'meta', ' · '));
      line.appendChild(ib);
    }
    where.appendChild(line);
    row.appendChild(where);
    row.appendChild(state);
    main.appendChild(row);
    cb.checked = false;
    row.classList.remove('marked');
    state.textContent = tr('dups.cross.stays');
  });
  crossState.syncs.push(function () { rows.forEach(function (sync) { sync(); }); });
  box.appendChild(main);
  return box;
}

function loadDuplicatesHere() {
  var page = $('page');
  clearNode(page);
  page.appendChild(el('h2', '', tr('dups.title')));
  addDupTabs(page);
  page.appendChild(el('p', 'sub', tr('dups.looking')));
  return api(LIBAPI + '/duplicates').then(function (res) {
    if (state.filter.view !== 'duplicates') return;
    dupState.groups = res.groups;
    dupState.shown = 0;
    // Ticks survive a reload (a scan finishing meanwhile) for copies still listed.
    var still = {};
    dupState.groups.forEach(function (g) {
      var suggested = {};
      suggestedIds(g).forEach(function (id) { suggested[id] = true; });
      g.files.forEach(function (f) {
        if (dupState.marked[f.id]) still[f.id] = true;
        // The suggestion starts ticked, once (an untick stays after a reload).
        else if (suggested[f.id] && !dupState.seen[f.id]) still[f.id] = true;
        if (suggested[f.id]) dupState.seen[f.id] = true;
      });
    });
    dupState.marked = still;
    renderDuplicates();
    updateDupBar();
  }).catch(failed);
}

function renderDuplicates() {
  var page = $('page');
  clearNode(page);
  dupState.syncs = [];
  page.appendChild(el('h2', '', tr('dups.title')));
  addDupTabs(page);
  page.appendChild(el('p', 'sub', tr(dupState.groups.length ? 'dups.sub' : 'dups.none')));
  if (dupState.groups.length) page.appendChild(dupToolbar());
  var box = el('div');
  page.appendChild(box);
  var more = el('button', 'btn quiet', tr('app.show_more'));
  more.onclick = function () { showGroups(box, more); };
  page.appendChild(more);
  var action = el('div', 'dup-bar');
  action.id = 'dup-bar';
  action.hidden = true;
  page.appendChild(action);
  showGroups(box, more);
}

function showGroups(box, more) {
  var groups = visibleGroups();
  var end = Math.min(groups.length, dupState.shown + PAGE_GROUPS);
  for (var k = dupState.shown; k < end; k++) box.appendChild(groupNode(groups[k]));
  dupState.shown = end;
  more.hidden = end >= groups.length;
  if (!groups.length && dupState.groups.length) box.appendChild(el('p', 'sub', tr('dups.none_kinds')));
}

// Ticked files in the groups that are shown.
function markedCount() {
  var n = 0;
  visibleGroups().forEach(function (g) { g.files.forEach(function (f) { if (dupState.marked[f.id]) n++; }); });
  return n;
}

// Left: which kinds to show (a drop-down with check boxes). Right: the
// Preselect menu, which only ticks; nothing is removed before Move to trash.
var dupFilterCloser = false;
function dupToolbar() {
  var bar = el('div', 'dup-top');
  var filter = el('details', 'dup-filter');
  var sum = el('summary', 'btn quiet');
  var menu = el('div', 'dup-filter-menu');
  var count = {};
  dupState.groups.forEach(function (g) { count[g.kind] = (count[g.kind] || 0) + 1; });
  var refreshLabel = function () {
    var on = DUP_KINDS.filter(function (k) { return dupState.types[k.id]; });
    sum.textContent = tr('dups.show', { what: on.length === DUP_KINDS.length ? tr('dups.all_kinds') : on.length ? on.map(function (k) { return k.label; }).join(', ') : tr('dups.nothing') });
  };
  DUP_KINDS.forEach(function (k) {
    var label = el('label', 'check');
    setTip(label, k.hint);
    var cb = el('input');
    cb.type = 'checkbox';
    cb.checked = dupState.types[k.id];
    cb.onchange = function () {
      dupState.types[k.id] = cb.checked;
      try { localStorage.setItem('dupTypes', JSON.stringify(dupState.types)); } catch (e) { /* ignore */ }
      refreshLabel();
      var open = filter.open;
      dupState.shown = 0;
      renderDuplicates();
      updateDupBar();
      var again = document.querySelector('.dup-filter');
      if (again && open) again.open = true;
    };
    label.appendChild(cb);
    label.appendChild(document.createTextNode(' ' + k.label + ' (' + (count[k.id] || 0) + ')'));
    menu.appendChild(label);
  });
  refreshLabel();
  // A click anywhere else closes the menu (one listener, whichever toolbar is current).
  if (!dupFilterCloser) {
    dupFilterCloser = true;
    document.addEventListener('click', function (e) {
      document.querySelectorAll('.dup-filter[open]').forEach(function (open) {
        if (!open.contains(e.target)) open.open = false;
      });
    });
  }
  filter.appendChild(sum);
  filter.appendChild(menu);
  bar.appendChild(filter);
  bar.appendChild(preselectMenu());
  return bar;
}

// What the page ticks by itself: of an identical or same-photo group the
// files that are surely worse (a smaller or stripped copy) or the same file
// again, never the best one. Look-alikes and edits are never ticked by the
// page: the person decides on those.
function suggestedIds(g) {
  if (g.kind !== 'identical' && g.kind !== 'resolution') return [];
  var pickOf = {};
  g.files.forEach(function (f) { if (f.pick) pickOf[f.row] = f; });
  return g.files.filter(function (f) {
    var best = pickOf[f.row];
    return !f.pick && ((f.keeper && !f.bigger_file) || (f.same && best && best.same === f.same) || (f.in_copies && best && !best.in_copies));
  }).map(function (f) { return f.id; });
}

// The format of a file by its extension, JPEG and HEIC spelled one way.
function fileFormat(f) {
  var e = (f.name.split('.').pop() || '').toLowerCase();
  return e === 'jpeg' || e === 'jpe' ? 'jpg' : e === 'heif' ? 'heic' : e;
}

// The HEIC and JPEG files of a group to tick so that `keep` stays wherever a
// photo exists in both formats.
function otherFormats(g, keep) {
  var ids = [];
  dupRows(g).forEach(function (row) {
    if (!row.some(function (f) { return fileFormat(f) === keep; })) return;
    row.forEach(function (f) {
      var fmt = fileFormat(f);
      if (fmt !== keep && (fmt === 'heic' || fmt === 'jpg')) ids.push(f.id);
    });
  });
  return ids;
}

// The entries of the Preselect menu: what each ticks in a group (`ids`), and
// the kind of group it is for (`kind`, and `also` for a second one; none:
// every kind).
var PRESELECT = [
  { id: 'suggested', ids: suggestedIds },
  { id: 'samefolder', kind: 'identical', ids: function (g) {
    var buckets = {}, ids = [];
    g.files.forEach(function (f) { var k = f.same + ':' + f.folder_id; (buckets[k] = buckets[k] || []).push(f); });
    Object.keys(buckets).forEach(function (k) {
      var b = buckets[k];
      if (b.length < 2) return;
      var keep = b.filter(function (f) { return f.pick; })[0] || b.filter(function (f) { return !f.copy_named; })[0] || b[0];
      b.forEach(function (f) { if (f !== keep) ids.push(f.id); });
    });
    return ids;
  } },
  { id: 'smaller', kind: 'resolution', also: 'similar', ids: function (g) { return g.files.filter(function (f) { return f.keeper && !f.bigger_file; }).map(function (f) { return f.id; }); } },
  { id: 'heic', kind: 'resolution', ids: function (g) { return otherFormats(g, 'heic'); } },
  { id: 'jpeg', kind: 'resolution', ids: function (g) { return otherFormats(g, 'jpg'); } },
  { id: 'edited', kind: 'edited', ids: function (g) {
    return g.files.some(function (f) { return !isEdit(f); }) ? g.files.filter(isEdit).map(function (f) { return f.id; }) : [];
  } },
  { id: 'originals', kind: 'edited', ids: function (g) {
    return g.files.some(isEdit) ? g.files.filter(function (f) { return !isEdit(f); }).map(function (f) { return f.id; }) : [];
  } },
  { id: 'biggest', kind: 'similar', ids: function (g) {
    var top = Math.max.apply(null, g.files.map(function (f) { return f.size; }));
    var best = g.files.filter(function (f) { return f.size === top; });
    return best.length === 1 ? g.files.filter(function (f) { return f !== best[0]; }).map(function (f) { return f.id; }) : [];
  } },
  { id: 'lastshot', kind: 'similar', ids: function (g) {
    // The best file of the latest shot stays; a group is skipped when a shot
    // has no capture date or two shots tie for the latest.
    var shots = dupRows(g).map(function (row) { return row.filter(function (f) { return f.pick; })[0] || row[0]; });
    if (shots.some(function (f) { return !f.taken; })) return [];
    var latest = shots.map(function (f) { return f.taken; }).sort().pop();
    var last = shots.filter(function (f) { return f.taken === latest; });
    return last.length === 1 ? g.files.filter(function (f) { return f !== last[0]; }).map(function (f) { return f.id; }) : [];
  } },
];

// The message of an entry (its label, or its tooltip with `_hint`).
function preselectText(entry, hint) {
  var texts = {
    suggested: ['dups.pre.suggested', 'dups.pre.suggested_hint'],
    samefolder: ['dups.pre.samefolder', 'dups.pre.samefolder_hint'],
    smaller: ['dups.pre.smaller', 'dups.pre.smaller_hint'],
    heic: ['dups.pre.heic', 'dups.pre.heic_hint'],
    jpeg: ['dups.pre.jpeg', 'dups.pre.jpeg_hint'],
    edited: ['dups.pre.edited', 'dups.pre.edited_hint'],
    originals: ['dups.pre.originals', 'dups.pre.originals_hint'],
    biggest: ['dups.pre.biggest', 'dups.pre.biggest_hint'],
    lastshot: ['dups.pre.lastshot', 'dups.pre.lastshot_hint'],
  };
  return tr(texts[entry.id][hint ? 1 : 0]);
}

// Is a group of the kind this entry works on?
function preselectKind(entry, g) {
  return !entry.kind || g.kind === entry.kind || g.kind === entry.also;
}

// Files and groups an entry would tick among the groups that are shown.
function preselectCount(entry) {
  var files = 0, groups = 0;
  visibleGroups().forEach(function (g) {
    if (!preselectKind(entry, g)) return;
    var n = entry.ids(g).length;
    if (n) { files += n; groups++; }
  });
  return { files: files, groups: groups };
}

// Ticks what the entry names (replacing the ticks of the groups that are
// shown) and brings those groups to the top. Nothing is removed.
function applyPreselect(entry) {
  Object.keys(shownMarked()).forEach(function (id) { delete dupState.marked[id]; });
  var hit = [], rest = [];
  dupState.groups.forEach(function (g) {
    var ids = dupState.types[g.kind] && preselectKind(entry, g) ? entry.ids(g) : [];
    ids.forEach(function (id) { dupState.marked[id] = true; });
    (ids.length ? hit : rest).push(g);
  });
  dupState.groups = hit.concat(rest);
  dupState.shown = 0;
  renderDuplicates();
  updateDupBar();
  window.scrollTo(0, 0);
}

// The Preselect drop-down: one line per rule that finds something, with how
// many files it would tick and a tooltip that says what it does. Rules that
// find nothing among the groups that are shown are left out.
function preselectMenu() {
  var box = el('details', 'dup-filter dup-pre');
  box.appendChild(el('summary', 'btn', tr('dups.pre')));
  var menu = el('div', 'dup-filter-menu');
  var heading = null, any = false;
  PRESELECT.forEach(function (entry) {
    var c = preselectCount(entry);
    if (!c.files) return;
    any = true;
    if (entry.kind && entry.kind !== heading) {
      heading = entry.kind;
      menu.appendChild(el('div', 'dup-pre-head', DUP_KINDS.filter(function (k) { return k.id === entry.kind; })[0].label));
    }
    var b = el('button', 'btn quiet', preselectText(entry, false) + ' · ' + trn('count.files', c.files));
    setTip(b, preselectText(entry, true));
    b.onclick = function () { applyPreselect(entry); };
    menu.appendChild(b);
  });
  if (!any) menu.appendChild(el('div', 'dup-pre-head', tr('dups.pre.none')));
  box.appendChild(menu);
  return box;
}

// Ticked files of the groups that are shown, as a set of ids.
function shownMarked() {
  var m = {};
  visibleGroups().forEach(function (g) { g.files.forEach(function (f) { if (dupState.marked[f.id]) m[f.id] = true; }); });
  return m;
}

// Puts the check boxes in line with `dupState.marked`, also for the groups
// that are not on screen yet.
function syncDupBoxes() {
  dupState.syncs.forEach(function (sync) { sync(); });
  updateDupBar();
}

// The bar stays while there are duplicates: Clear unticks everything, Move
// to trash deletes
// whatever is ticked (also a single file after a Clear).
function updateDupBar() {
  var bar = $('dup-bar');
  if (!bar) return;
  var groups = visibleGroups();
  bar.hidden = groups.length === 0;
  bar.textContent = '';
  if (!groups.length) return;
  var marked = shownMarked();
  var n = Object.keys(marked).length;
  bar.appendChild(el('span', '', tr('dups.marked', { count: trn('count.copies', n) })));
  var clear = el('button', 'btn quiet', tr('dups.clear'));
  setTip(clear, tr('dups.clear_hint'));
  clear.disabled = n === 0;
  clear.onclick = function () {
    Object.keys(marked).forEach(function (id) { delete dupState.marked[id]; });
    syncDupBoxes();
  };
  bar.appendChild(clear);
  var del = el('button', 'btn danger', tr('sel.trash'));
  del.disabled = n === 0;
  del.onclick = deleteMarked;
  bar.appendChild(del);
}

// One row per photo (content); the copies as cards on its right.
function dupRows(g) {
  var rows = [], byKey = {};
  g.files.forEach(function (f) {
    var key = 'r' + f.row;
    if (!byKey[key]) { byKey[key] = []; rows.push(byKey[key]); }
    byKey[key].push(f);
  });
  return rows;
}

// `versions`: the files of the photo's row, in card order; a click then opens the
// viewer on them (left and right switch between the versions, Esc or Space
// come back to this thumbnail) instead of the file in a new tab.
function dupThumb(f, compare, versions) {
  var a = el('a', 'thumb');
  a.href = LIBAPI + '/files/' + f.id + (f.kind === 'video' ? '/original' : '/view') + '?v=' + f.version;
  a.target = '_blank';
  a.rel = 'noopener';
  var img = el('img');
  img.alt = '';
  img.loading = 'lazy';
  img.src = LIBAPI + '/files/' + f.id + '/thumb?v=' + f.version;
  a.appendChild(img);
  if (compare) {
    // Opens the group's pictures side by side instead of one in a new tab.
    a.removeAttribute('target');
    setTip(a, tr('dups.compare'));
    a.onclick = function (e) { e.preventDefault(); compare(f); };
  } else if (versions && versions.length > 1) {
    a.removeAttribute('target');
    a.onclick = function (e) {
      e.preventDefault();
      a.focus(); // the viewer returns here (Safari does not focus a clicked link)
      openFiles(versions.map(function (v) { return { file: v.id, kind: v.kind, version: v.version }; }), 0);
    };
  }
  return a;
}

// "IMG_E1234" is the edited version of "IMG_1234" (iPhone).
function isEdit(f) { return /^img_e\d+/i.test(f.name); }

// The group's photos large and side by side. "Keep this one" ticks all the
// others and closes the dialog; "Keep" marks photos to stay (several can) and
// "Keep (N)" ticks all the others. Nothing is trashed here: Move to trash in
// the bar does the removing for every group at once.
function compareDialog(g, start, onKeep) {
  var back = el('div', 'modal');
  var dlg = el('div', 'dialog compare');
  dlg.appendChild(el('h2', '', tr('dups.compare_title')));
  var strip = el('div', 'compare-strip');
  var kept = {};
  var close = function () { back.remove(); document.removeEventListener('keydown', onKey); };
  var onKey = function (e) { if (e.key === 'Escape') close(); };
  document.addEventListener('keydown', onKey);
  var toggles = [];
  g.files.forEach(function (f) {
    var col = el('div', 'compare-col' + (f.id === start.id ? ' start' : ''));
    var img = el('img');
    img.alt = f.name;
    img.src = LIBAPI + '/files/' + f.id + (f.kind === 'video' ? '/thumb' : '/view') + '?v=' + f.version;
    col.appendChild(img);
    col.appendChild(el('div', 'name', f.name + (g.kind === 'edited' ? ' · ' + tr(isEdit(f) ? 'dups.edited_lower' : 'dups.original_lower') : '')));
    var meta = [];
    if (f.width && f.height) meta.push(f.width + ' × ' + f.height);
    meta.push((f.size / 1e6).toFixed(1) + ' MB');
    col.appendChild(el('div', 'meta', meta.join(' · ')));
    var row = el('div', 'compare-buttons');
    // Keep: mark this photo to stay; Keep (N) below ticks all the others.
    var tog = el('button', 'btn quiet keep-toggle');
    setTip(tog, tr('dups.keep_hint'));
    tog.innerHTML = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="3" width="18" height="18" rx="4"/><path class="tick" d="M7.5 12.5l3 3 6-7"/></svg>';
    tog.appendChild(el('span', '', tr('dups.keep')));
    tog.onclick = function () { if (kept[f.id]) delete kept[f.id]; else kept[f.id] = true; refresh(); };
    toggles.push({ f: f, col: col, tog: tog });
    row.appendChild(tog);
    var keep = el('button', 'btn', tr('dups.keep_this'));
    keep.onclick = function () { close(); onKeep([f]); };
    row.appendChild(keep);
    col.appendChild(row);
    strip.appendChild(col);
  });
  dlg.appendChild(strip);
  var row = el('div', 'actions');
  var go = el('button', 'btn');
  go.onclick = function () {
    close();
    onKeep(g.files.filter(function (f) { return kept[f.id]; }));
  };
  row.appendChild(go);
  var done = el('button', 'btn quiet', tr('app.close'));
  done.onclick = close;
  row.appendChild(done);
  // Keep (N) counts the photos marked; one has to stay, so it needs at least one.
  var refresh = function () {
    var n = 0;
    toggles.forEach(function (t) {
      var on = !!kept[t.f.id];
      if (on) n++;
      t.tog.classList.toggle('on', on);
      t.col.classList.toggle('kept', on);
    });
    go.textContent = tr('dups.keep') + ' (' + n + ')';
    go.disabled = n === 0;
  };
  refresh();
  dlg.appendChild(row);
  back.onclick = function (e) { if (e.target === back) close(); };
  back.appendChild(dlg);
  document.body.appendChild(back);
}

function groupNode(g) {
  var box = el('div', 'group');
  var head = el('div', 'group-head');
  var kind = DUP_KINDS.filter(function (k) { return k.id === g.kind; })[0];
  var title = el('span', 'label', kind.label);
  setTip(title, kind.hint);
  head.appendChild(title);
  var ids = g.files.map(function (f) { return f.id; });
  var decide = function (decision, label, hint) {
    var b = el('button', 'btn quiet', label);
    setTip(b, hint);
    b.onclick = function () {
      b.disabled = true;
      post(LIBAPI + '/duplicates/decide', { ids: ids, decision: decision }).then(function () {
        ids.forEach(function (id) { delete dupState.marked[id]; });
        box.remove();
        updateDupBar();
        toast(tr(decision === 'linked' ? 'dups.kept_versions' : 'dups.kept_different'));
      }).catch(function (e) { b.disabled = false; failed(e); });
    };
    head.appendChild(b);
  };
  if (g.kind === 'similar') decide('distinct', tr('dups.btn_different'), tr('dups.btn_different_hint'));
  decide('linked', tr('dups.btn_keep'), tr('dups.btn_keep_hint'));
  box.appendChild(head);
  box.appendChild(el('p', 'group-hint', g.kind === 'similar' || g.kind === 'edited'
    ? tr('dups.group_hint_series', { hint: kind.hint })
    : kind.hint));

  var boxes = [], cardSyncs = [];
  // At least one copy of the group stays: the last unticked box is disabled.
  var refresh = function () {
    var open = boxes.filter(function (b) { return !b.checked; });
    boxes.forEach(function (b) { b.disabled = !b.checked && open.length === 1; });
  };
  var registerGroup = function () {
    refresh();
    dupState.syncs.push(function () {
      cardSyncs.forEach(function (sync) { sync(); });
      refresh();
    });
  };
  // One card per file: the check box, where it is, what it is, its tags.
  // `thumb`: the card carries its own thumbnail (similar photos).
  var cards = [];
  var compare = function (f) { compareDialog(g, f, keepTicked); };
  // The one-click choice on a card of a series: tick the other files and
  // leave this one open. Nothing is trashed until "Move to trash".
  // `keep`: the files that stay; every other file of the group is ticked.
  var keepTicked = function (keep) {
    var stay = {};
    keep.forEach(function (f) { stay[f.id] = true; });
    cards.forEach(function (c) {
      c.cb.checked = !stay[c.f.id];
      c.cb.dispatchEvent(new Event('change'));
    });
  };
  // What tells the files of a series apart, on the card that leads: more
  // pixels, a bigger file. Shown only where the files differ; never decides.
  var most = function (key) {
    var v = g.files.map(key), top = Math.max.apply(null, v);
    return v.some(function (x) { return x !== top; }) ? top : null;
  };
  var heavier = {};
  g.files.forEach(function (f) { if (f.keeper && f.bigger_file) heavier[f.keeper] = true; });
  var sizeBadge = function (key) {
    var b = el('div', 'badge worse', tr(key));
    setTip(b, tr('dups.size_conflict_hint'));
    return b;
  };
  var topPixels = most(function (f) { return (f.width || 0) * (f.height || 0); });
  var topSize = most(function (f) { return f.size; });
  var copyCard = function (f, showName, thumb) {
    var card = el('div', 'copy' + (thumb ? ' with-thumb' : ''));
    if (thumb) card.appendChild(dupThumb(f, compare));
    var label = el('label', 'check');
    var cb = el('input');
    cb.type = 'checkbox';
    cb.checked = !!dupState.marked[f.id];
    cb.onchange = function () {
      if (cb.checked) dupState.marked[f.id] = true; else delete dupState.marked[f.id];
      card.classList.toggle('marked', cb.checked);
      refresh();
      updateDupBar();
    };
    boxes.push(cb);
    cards.push({ f: f, cb: cb });
    cardSyncs.push(function () {
      cb.checked = !!dupState.marked[f.id];
      card.classList.toggle('marked', cb.checked);
    });
    label.appendChild(cb);
    label.appendChild(document.createTextNode(' ' + tr('dups.delete_copy')));
    card.appendChild(label);
    card.classList.toggle('marked', cb.checked);
    var folder = f.path.indexOf('/') >= 0 ? f.path.slice(0, f.path.lastIndexOf('/')) : tr('dups.top_level');
    var fb = el('button', 'folder', folder);
    fb.title = f.path;
    fb.onclick = function () { setFilter({ folder: f.folder_id, tags: [], q: '' }); };
    card.appendChild(fb);
    if (showName) card.appendChild(el('div', 'name', f.name));

    if (g.kind === 'edited') card.appendChild(el('div', 'badge', tr(isEdit(f) ? 'dups.badge_edited' : 'dups.badge_original')));
    var meta = [];
    if (f.width && f.height) meta.push(f.width + ' × ' + f.height);
    meta.push((f.size / 1e6).toFixed(1) + ' MB');
    card.appendChild(el('div', 'meta', meta.join(' · ')));
    // Fewer pixels, and a bigger file than the copy that stays: not ticked for
    // you. Both cards say what they lose on, both red.
    if (f.keeper && !f.bigger_file) card.appendChild(el('div', 'badge worse', tr('dups.lower_quality')));
    else if (f.keeper) card.appendChild(sizeBadge('dups.lower_quality'));
    else if (heavier[f.id]) card.appendChild(sizeBadge('dups.smaller_file'));
    if (f.in_copies) card.appendChild(el('div', 'meta', tr('dups.in_copies')));
    card.appendChild(el('div', 'meta', f.taken ? formatDate({ taken: f.taken, date_source: 'file' }) : tr('dups.no_capture_date')));
    if (thumb && topPixels !== null && (f.width || 0) * (f.height || 0) === topPixels) card.appendChild(el('div', 'badge', tr('dups.badge_pixels')));
    if (thumb && topSize !== null && f.size === topSize) card.appendChild(el('div', 'badge', tr('dups.badge_bigger')));
    if (thumb) {
      // Pinned to the foot of the card, edge to edge.
      var only = el('button', 'keep-one', tr('dups.keep_this'));
      setTip(only, tr('dups.keep_this_hint'));
      only.onclick = function () { keepTicked([f]); };
      card.appendChild(only);
    }
    return card;
  };
  var bestFirst = function (copies) {
    return copies.slice().sort(function (a, b) {
      return (!!b.pick - !!a.pick) || ((b.width || 0) * (b.height || 0) - (a.width || 0) * (a.height || 0));
    });
  };

  if (g.kind === 'similar' || g.kind === 'edited') {
    // Different shots side by side, each card with its own thumbnail; a
    // shot's other versions (a messenger copy) follow it.
    var series = el('div', 'copies series');
    dupRows(g).forEach(function (copies) {
      bestFirst(copies).forEach(function (f) { series.appendChild(copyCard(f, true, true)); });
    });
    box.appendChild(series);
    registerGroup();
    return box;
  }

  dupRows(g).forEach(function (copies) {
    var row = el('div', 'dup-row');
    // The photo shown is the best version of the row; its cards come first.
    copies = bestFirst(copies);
    var first = copies[0];
    var left = el('div', 'dup-photo');
    left.appendChild(dupThumb(first, null, copies));
    var name = el('div', 'name', first.name + ' ');
    if (first.same) {
      var same = el('span', 'same', String.fromCharCode(64 + first.same));
      setTip(same, tr('dups.same_letter'));
      name.appendChild(same);
    }
    left.appendChild(name);
    row.appendChild(left);
    var cards = el('div', 'copies');
    copies.forEach(function (f) { cards.appendChild(copyCard(f, copies.length > 1 || first.name !== f.name, false)); });
    row.appendChild(cards);
    box.appendChild(row);
  });
  registerGroup();
  return box;
}

// Sends the marked copies, one request per group. Capture dates that really
// conflict are asked for afterwards, then those groups are sent again.
function deleteMarked() {
  var jobs = [], total = 0;
  visibleGroups().forEach(function (g) {
    var remove = g.files.filter(function (f) { return dupState.marked[f.id]; }).map(function (f) { return f.id; });
    if (!remove.length) return;
    var keep = g.files.filter(function (f) { return !dupState.marked[f.id]; }).map(function (f) { return f.id; });
    if (!keep.length) return;
    jobs.push({ keep: keep, remove: remove });
    total += remove.length;
  });
  if (!jobs.length) return;
  var run = function (btn) {
    btn.disabled = true;
    var done = { files: 0, tags: 0, dates: 0, folders: [], skipped: [] }, conflicts = [];
    var send = function (job, i) {
      if (i >= jobs.length) return Promise.resolve();
      return post(LIBAPI + '/duplicates/remove', job).then(function (r) {
        if (r.conflicts && r.conflicts.length) conflicts.push({ job: job, list: r.conflicts });
        else collectRemoved(done, r);
        return send(jobs[i + 1], i + 1);
      });
    };
    send(jobs[0], 0).then(function () {
      closeModal();
      if (conflicts.length) return askDates(conflicts, done);
    }).then(function () {
      finishRemoved(done);
    }).catch(function (e) { closeModal(); failed(e); changed(); });
    return false;
  };
  openModal(
    tr('sel.trash'),
    tr('dups.delete_text', { count: trn('count.copies', total) }),
    [{ label: tr('app.cancel'), cls: 'quiet' }, { label: tr('sel.trash'), cls: 'danger', focus: true, onclick: run }]
  );
}

function collectRemoved(done, r) {
  done.files += r.trashed.files.length;
  done.tags += r.tags_added;
  done.dates += r.dates_set;
  (r.trashed.removed_folders || []).forEach(function (f) { done.folders.push(f); });
  (r.trashed.skipped || []).forEach(function (s) { done.skipped.push(s); });
}

function finishRemoved(done) {
  var text = [tr('dups.done.trashed', { count: trn('count.copies', done.files) })];
  if (done.tags) text.push(tr('dups.done.tags', { count: trn('count.tags', done.tags) }));
  if (done.dates) text.push(tr('dups.done.dates', { count: trn('count.capture_dates', done.dates) }));
  toast(text.join(', ') + removedFolders({ removed_folders: done.folders }));
  report(tr('dups.skipped'), done.skipped);
  dupState.marked = {};
  changed();
}

// Copies with capture dates that cannot be merged: the user picks one date
// per photo that stays.
function askDates(conflicts, done) {
  return new Promise(function (resolve, reject) {
    var body = el('div');
    body.appendChild(el('p', '', tr('dups.dates.text')));
    var picks = [];
    conflicts.forEach(function (c) {
      c.list.forEach(function (x, n) {
        body.appendChild(el('div', 'name', x.path));
        x.dates.forEach(function (d, k) {
          var label = el('label', 'check');
          var radio = el('input');
          radio.type = 'radio';
          radio.name = 'date-' + c.job.keep[0] + '-' + n;
          radio.value = d;
          radio.checked = k === 0;
          label.appendChild(radio);
          label.appendChild(document.createTextNode(' ' + formatDate({ taken: d, date_source: 'file' })));
          body.appendChild(label);
          if (k === 0) picks.push({ job: c.job, keep: x.keep, radio: radio, name: radio.name });
        });
      });
    });
    openModal(tr('dups.dates.title'), body, [{
      label: tr('app.cancel'), cls: 'quiet', onclick: function () { resolve(); },
    }, {
      label: tr('sel.trash'), cls: 'danger', onclick: function (btn) {
        btn.disabled = true;
        var chosen = {};
        picks.forEach(function (p) {
          var sel = document.querySelector('input[name="' + p.name + '"]:checked');
          (chosen[p.job.keep[0]] = chosen[p.job.keep[0]] || {})[p.keep] = sel ? sel.value : p.radio.value;
        });
        var jobs = conflicts.map(function (c) { return c.job; });
        var send = function (i) {
          if (i >= jobs.length) return Promise.resolve();
          var job = jobs[i];
          return post(LIBAPI + '/duplicates/remove', { keep: job.keep, remove: job.remove, dates: chosen[job.keep[0]] }).then(function (r) {
            collectRemoved(done, r);
            return send(i + 1);
          });
        };
        send(0).then(function () { closeModal(); resolve(); }).catch(function (e) { closeModal(); reject(e); });
        return false;
      },
    }]);
  });
}

// ------------------------------------------------------------------ trash page

// "Only duplicates" on the trash page: shows just the copies removed as
// duplicates, so a selection (and Select all) cannot reach what was trashed by hand.
var trashDupsOnly = false;

// The file a trash card shows: the photo, not its RAW or Live Photo video.
function trashFirst(files) {
  var main = function (f) { return f.kind && f.kind !== 'raw' && f.kind !== 'video'; };
  return files.filter(main)[0] || files.filter(function (f) { return f.kind; })[0] || files[0];
}

function loadTrash() {
  return api(LIBAPI + '/trash').then(function (items) {
    if (state.filter.view !== 'trash') return;
    var page = $('page');
    clearNode(page);
    var allBatches = [], byBatch = {};
    items.forEach(function (it) {
      if (!byBatch[it.batch]) { byBatch[it.batch] = []; allBatches.push(it.batch); }
      byBatch[it.batch].push(it);
    });
    var dupBatches = allBatches.filter(function (b) { return !!trashFirst(byBatch[b]).duplicate_of; });
    if (!dupBatches.length) trashDupsOnly = false;
    var batches = trashDupsOnly ? dupBatches : allBatches;
    var head = el('div', 'trash-head');
    head.appendChild(el('h2', '', tr('side.trash')));
    if (allBatches.length) {
      var only = el('label', 'check');
      var onlyBox = el('input');
      onlyBox.type = 'checkbox';
      onlyBox.checked = trashDupsOnly;
      onlyBox.disabled = !dupBatches.length;
      onlyBox.onchange = function () { trashDupsOnly = onlyBox.checked; loadTrash(); };
      only.appendChild(onlyBox);
      only.appendChild(document.createTextNode(' ' + tr('trash.only_duplicates', { count: I18n.number(dupBatches.length) })));
      setTip(only, tr('trash.only_duplicates_hint'));
      head.appendChild(only);
    }
    page.appendChild(head);
    page.appendChild(el('p', 'sub', batches.length
      ? tr('trash.page_sub', { count: trn('count.photos', batches.length) })
      : tr('trash.empty')));
    // Click a photo to select it (Shift-click: from the last one clicked); the
    // bar at the bottom puts the selection back or deletes it for good.
    var picked = {}, anchor = null, cardOf = {};
    var bar = el('div', 'dup-bar');
    bar.hidden = !batches.length;
    var drawBar = function () {
      var n = batches.filter(function (b) { return picked[b]; }).length;
      bar.textContent = '';
      bar.appendChild(el('span', n ? '' : 'meta', n ? tr('trash.selected', { count: trn('count.photos', n) }) : tr('trash.hint')));
      var all = el('button', 'btn quiet', tr('trash.select_all', { count: I18n.number(batches.length) }));
      all.disabled = n === batches.length;
      all.onclick = function () { batches.forEach(function (b) { picked[b] = true; cardOf[b].classList.add('sel'); }); drawBar(); };
      bar.appendChild(all);
      var clear = el('button', 'btn quiet', tr('dups.clear'));
      clear.disabled = !n;
      clear.onclick = function () { picked = {}; batches.forEach(function (b) { cardOf[b].classList.remove('sel'); }); drawBar(); };
      bar.appendChild(clear);
      var back = el('button', 'btn quiet', tr('trash.put_back'));
      back.disabled = !n;
      back.onclick = function () { putBack(batches.filter(function (b) { return picked[b]; }), byBatch); };
      bar.appendChild(back);
      var del = el('button', 'btn danger', tr('trash.delete_for_good'));
      del.disabled = !n;
      del.onclick = function () { deleteBatches(batches.filter(function (b) { return picked[b]; }), trashDupsOnly ? -1 : batches.length); };
      bar.appendChild(del);
    };
    var cards = el('div', 'cards has-drag');
    cards.style.flexWrap = 'wrap';
    cards.dragSite = function () {
      return {
        sel: '.card',
        itemOf: function (tile) { return tile.batch; },
        get: function (b) { return !!picked[b]; },
        set: function (b, on) { if (on) picked[b] = true; else delete picked[b]; cardOf[b].classList.toggle('sel', on); },
        done: function (b) { if (b !== undefined) anchor = b; drawBar(); },
      };
    };
    batches.forEach(function (b) {
      var files = byBatch[b];
      var first = trashFirst(files);
      var card = el('div', 'card pick');
      cardOf[b] = card;
      card.batch = b;
      var thumb = el('div', 'thumb');
      var img = el('img');
      img.alt = '';
      img.src = LIBAPI + '/trash/' + first.id + '/thumb';
      img.onerror = function () { this.remove(); };
      thumb.appendChild(img);
      // In the square's corner, the same place on every card (the whole
      // picture stays visible, not cropped).
      var badges = trashBadges(files, first);
      if (badges) thumb.appendChild(badges);
      card.appendChild(thumb);
      card.appendChild(el('div', 'name', first.path.split('/').pop()));
      var folder = first.path.indexOf('/') >= 0 ? first.path.slice(0, first.path.lastIndexOf('/')) : tr('dups.top_level');
      card.appendChild(el('div', 'meta', folder));
      card.appendChild(el('div', 'meta', tr('trash.deleted', { date: I18n.date(new Date(first.deleted_at * 1000)) })));
      card.onclick = function (ev) {
        var span = ev.shiftKey && anchor !== null ? pickSpan(batches, function (x) { return x; }, anchor, b) : null;
        var on = !picked[b];
        (span || [b]).forEach(function (x) {
          if (on) picked[x] = true; else delete picked[x];
          cardOf[x].classList.toggle('sel', !!picked[x]);
        });
        anchor = b;
        drawBar();
      };
      card.onmousedown = function (ev) { if (ev.shiftKey) ev.preventDefault(); };
      cards.appendChild(card);
    });
    page.appendChild(cards);
    page.appendChild(bar);
    drawBar();
  }).catch(failed);
}

// The badges at the foot of a trash card: one per file that went with the
// photo (LIVE ▶ for its Live Photo video, RAW, else the file type), and for a copy
// removed as a duplicate one that opens the copy that stayed.
function trashBadges(files, first) {
  var box = el('div', 'badges');
  files.forEach(function (f) {
    if (f === first) return;
    var name = f.path.split('/').pop();
    var live = f.kind === 'video' && first.kind !== 'video';
    var b = el('span', 'badge', live ? 'LIVE ▶' : f.kind === 'raw' ? 'RAW' : name.slice(name.lastIndexOf('.') + 1).toUpperCase());
    setTip(b, name + ' · ' + formatBytes(f.size));
    box.appendChild(b);
  });
  var dup = first.duplicate_of;
  if (dup) {
    var d;
    if (dup.id != null) {
      d = el('button', 'badge dup', tr('trash.duplicate'));
      setTip(d, tr('trash.duplicate_of', { path: dup.path }) + '\n' + tr('trash.duplicate_open'));
      d.onclick = function (ev) {
        ev.stopPropagation();
        openFiles([{ file: dup.id, kind: dup.kind, version: dup.version }], 0);
      };
    } else {
      d = el('span', 'badge dup gone', tr('trash.duplicate'));
      setTip(d, tr('trash.duplicate_gone'));
    }
    box.appendChild(d);
  }
  return box.children.length ? box : null;
}

// Puts the batches back, one after the other; says how many worked.
function putBack(list, byBatch) {
  var done = 0, first = null;
  var next = function (i) {
    if (i >= list.length) return Promise.resolve();
    return post(LIBAPI + '/trash/' + list[i] + '/restore').then(function () {
      if (!first) first = byBatch[list[i]][0].path;
      done++;
      return next(i + 1);
    });
  };
  next(0).then(function () {
    toast(done === 1 ? tr('trash.put_back_done', { path: first }) : tr('trash.put_back_many', { count: trn('count.photos', done) }));
    changed();
  }).catch(function (e) { failed(e); changed(); });
}

// Asks, then deletes the batches for good; everything at once when all are chosen
// (total -1: only some are shown, so only the chosen ones).
function deleteBatches(list, total) {
  deleteForGood(list.length === total ? [null] : list, list.length);
}

function deleteForGood(batches, n) {
  openModal(tr('trash.delete_for_good'), tr('trash.delete_text', { count: trn('count.photos', n) }), [
    { label: tr('app.cancel'), cls: 'quiet' },
    { label: tr('trash.delete_btn'), cls: 'danger', onclick: function () {
      var deleted = 0;
      var next = function (i) {
        if (i >= batches.length) return Promise.resolve();
        return post(LIBAPI + '/trash/empty', { batch: batches[i] }).then(function (r) { deleted += r.deleted; return next(i + 1); });
      };
      next(0).then(function () {
        toast(tr('trash.deleted_toast', { count: trn('count.files', deleted) }));
        changed();
      }).catch(function (e) { failed(e); changed(); });
    } },
  ]);
}

// ------------------------------------------------------------------ show in Finder, copy path, context menu

// Ask the computer that runs shoebox to show the file in its file manager.
// Only offered (state.reveal) to a browser on that computer; an iPad cannot
// open a Finder window over there.
var REVEAL_LABELS = { 'Show in Finder': 'reveal.finder', 'Show in Explorer': 'reveal.explorer', 'Open folder': 'reveal.folder' };
function revealLabel(text) { return text ? (REVEAL_LABELS[text] ? tr(REVEAL_LABELS[text]) : text) : null; }

function revealFile(id) {
  post(LIBAPI + '/files/' + id + '/reveal').then(function (r) {
    toast(tr('reveal.shown', { app: r.app === 'file manager' ? tr('reveal.file_manager') : r.app }));
  }).catch(failed);
}

// The one open-folder icon of the whole app (drawn, so it looks the same on
// every system and takes the colour of the text around it). Never the emoji.
var FOLDER_OPEN = lineIcon('<path d="M4 18V7a2 2 0 0 1 2-2h3.5l2 2H17a2 2 0 0 1 2 2v1.5"/><path d="M4 18l2.6-6.4a1.5 1.5 0 0 1 1.4-.9h12.3a1 1 0 0 1 .9 1.4L19 17.6a1.5 1.5 0 0 1-1.4.9H4z"/>');
function folderIcon() {
  var i = el('span', 'ficon');
  i.setAttribute('aria-hidden', 'true');
  i.innerHTML = FOLDER_OPEN;
  return i;
}
// A page with its corner folded over, for a file in the picker (our own drawing).
var FILE_PAGE = lineIcon('<path d="M6.5 4.5A1.5 1.5 0 0 1 8 3h6l4.5 4.5V19.5A1.5 1.5 0 0 1 17 21H8a1.5 1.5 0 0 1-1.5-1.5z"/><path d="M14 3.5V7a1 1 0 0 0 1 1h3.5"/>');
function fileIcon() {
  var i = el('span', 'ficon');
  i.setAttribute('aria-hidden', 'true');
  i.innerHTML = FILE_PAGE;
  return i;
}

// "Show in Finder" with the drawn open-folder icon at the left of the label.
function revealButton() {
  var b = el('button', 'btn quiet with-icon');
  b.appendChild(folderIcon());
  b.appendChild(el('span', '', state.reveal));
  return b;
}

// Info panel, 5a: the buttons that show the file on the shoebox computer
// and copy its path.
function infoReveal(actions, info) {
  if (state.reveal) {
    var reveal = revealButton();
    reveal.onclick = function () { revealFile(info.id); };
    actions.appendChild(reveal);
  }
  var copy = el('button', 'btn quiet', tr('reveal.copy_path'));
  copy.title = info.path;
  copy.onclick = function () { copyPath(info.path); };
  actions.appendChild(copy);
}

// The path within the library, as in the info panel. Works on every device.
function copyPath(path) {
  copyText(path).then(function (ok) {
    if (ok) { toast(tr('reveal.copied')); return; }
    // No clipboard access: show the path, selected, for copying by hand.
    var box = el('input');
    box.readOnly = true;
    box.value = path;
    box.onfocus = function () { box.select(); };
    openModal(tr('info.path'), box, [{ label: tr('app.ok') }]);
  });
}

// navigator.clipboard only exists on https and localhost; an iPad that opens
// shoebox by IP address has neither, so fall back to selecting a text field
// and `execCommand`, which iOS allows during a tap.
function copyText(text) {
  if (navigator.clipboard && window.isSecureContext) {
    return navigator.clipboard.writeText(text).then(function () { return true; }, function () { return legacyCopy(text); });
  }
  return Promise.resolve(legacyCopy(text));
}

function legacyCopy(text) {
  var previous = document.activeElement;
  var t = el('textarea');
  t.value = text;
  t.readOnly = true; // no keyboard on iOS
  t.style.cssText = 'position:fixed;top:0;left:0;width:1px;height:1px;opacity:0;font-size:16px';
  document.body.appendChild(t);
  t.focus();
  t.select();
  t.setSelectionRange(0, text.length);
  var ok = false;
  try { ok = document.execCommand('copy'); } catch (e) { ok = false; }
  t.remove();
  if (previous && previous.focus) previous.focus();
  return ok;
}

// A small menu at the pointer: [{label, run}].
var menu = null, menuFrom = null;

function showMenu(x, y, items) {
  closeMenu();
  menuFrom = document.activeElement;
  menu = el('div', 'menu');
  menu.setAttribute('role', 'menu');
  items.forEach(function (it) {
    var b = el('button', '', it.label);
    b.setAttribute('role', 'menuitem');
    b.onclick = function () { closeMenu(); it.run(); };
    menu.appendChild(b);
  });
  document.body.appendChild(menu);
  menu.style.left = Math.max(4, Math.min(x, window.innerWidth - menu.offsetWidth - 4)) + 'px';
  menu.style.top = Math.max(4, Math.min(y, window.innerHeight - menu.offsetHeight - 4)) + 'px';
  menu.firstChild.focus();
}

function closeMenu() {
  if (!menu) return;
  menu.remove();
  menu = null;
  if (menuFrom && document.body.contains(menuFrom)) menuFrom.focus();
  menuFrom = null;
}

document.addEventListener('mousedown', function (ev) { if (menu && !menu.contains(ev.target)) closeMenu(); });
window.addEventListener('blur', closeMenu);
window.addEventListener('resize', closeMenu);
$('scroller').addEventListener('scroll', closeMenu, { passive: true });
// Capturing, so that Escape and the arrows only act on the menu while it is open.
document.addEventListener('keydown', function (ev) {
  if (!menu) return;
  var items = Array.prototype.slice.call(menu.children), at = items.indexOf(document.activeElement);
  if (ev.key === 'Escape') {
    closeMenu();
  } else if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
    items[(at + (ev.key === 'ArrowDown' ? 1 : items.length - 1)) % items.length].focus();
  } else if (ev.key === 'Tab') {
    closeMenu();
    return;
  } else {
    return; // Enter and Space press the focused button
  }
  ev.preventDefault();
  ev.stopPropagation();
}, true);

// Right-click on a photo in the grid.
$('sizer').addEventListener('contextmenu', function (ev) {
  var a = ev.target.closest('.cell');
  if (!a || !state.data) return;
  ev.preventDefault();
  var id = state.data.ids[parseInt(a.dataset.index, 10)];
  var items = [];
  if (state.reveal) items.push({ label: state.reveal, run: function () { revealFile(id); } });
  items.push({ label: tr('reveal.copy_path'), run: function () {
    api(LIBAPI + '/files/' + id).then(function (info) { copyPath(info.path); }).catch(failed);
  } });
  // On one person's photos (from the sidebar or as the only search term):
  // their face in this photo becomes their picture.
  var only = state.filter.people && state.filter.people.length === 1 ? state.filter.people[0] : null;
  if (only != null && state.personNames[only]) {
    items.push({ label: tr('menu.use_as', { name: state.personNames[only] }), run: function () {
      post(LIBAPI + '/people/' + only + '/cover', { file: id }).then(function (r) { toast(tr('menu.picture_changed')); personUpdated(r); }).catch(failed);
    } });
  }
  showMenu(ev.clientX, ev.clientY, items);
});

// ------------------------------------------------------------------ own tags

// Info panel, 5b: folder tags (from where the file is; not removable, move
// the photo to get rid of one), own tags (✗ removes them) and "+ Tag".
function infoTags(row, info) {
  var tags = el('div', 'tags');
  info.tags.forEach(function (t) {
    state.tagNames[t.id] = t.name;
    if (t.source === 'user' && (isFavTag(t.name) || isHiddenTag(t.name))) return; // the heart and the eye in the top bar
    var show = function () { closeLightbox(); setFilter({ folder: null, tags: [t.id], q: '' }); };
    if (t.source === 'folder') {
      var b = el('button', '');
      b.appendChild(folderIcon());
      b.appendChild(document.createTextNode(t.name));
      setTip(b, tr('tags.folder_hint'));
      b.onclick = show;
      tags.appendChild(b);
      return;
    }
    var chip = el('span', 'own');
    var name = el('button', '', t.name);
    name.onclick = show;
    var x = el('button', 'x', '✕');
    setTip(x, tr('tags.remove_hint'));
    x.setAttribute('aria-label', tr('tags.remove_label', { name: t.name }));
    x.onclick = function () {
      post(LIBAPI + '/tags/remove', { ids: [info.id], name: t.name }).then(function () { tagsChanged(info.id); }).catch(failed);
    };
    chip.appendChild(name);
    chip.appendChild(x);
    tags.appendChild(chip);
  });
  var add = el('button', 'add', tr('tags.add'));
  setTip(add, tr('tags.add_key_hint'));
  add.onclick = function () {
    var input = tagInput();
    input.onkeydown = function (ev) {
      ev.stopPropagation(); // arrows and Escape belong to the field, not the viewer
      if (ev.key === 'Escape') { input.replaceWith(add); return; }
      if (ev.key !== 'Enter' || !input.value.trim()) return;
      // "favorite" is the heart, not a tag in the list.
      if (meansFav(input.value)) { setFavorite([info.id], true).then(function () { input.replaceWith(add); toast(tr('tags.reserved_fav')); }); return; }
      // "hidden", "versteckt" hide the photo, no tag appears.
      if (meansHidden(input.value)) { setHidden([info.id], true, true).then(function () { input.replaceWith(add); toast(tr('tags.reserved_hidden')); }); return; }
      input.disabled = true;
      post(LIBAPI + '/tags/add', { ids: [info.id], name: input.value }).then(function () { tagsChanged(info.id); })
        .catch(function (e) { input.disabled = false; failed(e); });
    };
    add.replaceWith(input);
    input.focus();
  };
  tags.appendChild(add);
  row(tr('info.tags'), tags);
}

// A text field that suggests existing tags while typing.
function tagInput() {
  var input = el('input');
  input.type = 'text';
  input.placeholder = tr('tags.placeholder');
  input.autocomplete = 'off';
  input.setAttribute('enterkeyhint', 'done');
  attachTagSuggestions(input);
  return input;
}

// After a change: refresh the open info panel and the sidebar.
// "Screenshot": a drop-down (Yes / No) that starts on shoebox's guess. Picking
// an answer is the user's own decision, kept by content in the library (never
// in the file); "Back to automatic" returns to the guess.
function infoScreenshot(row, info) {
  if (info.kind === 'video' || info.kind === 'raw') return;
  var box = el('div', 'shot-pick');
  var pick = el('select');
  [['1', tr('shot.yes')], ['0', tr('shot.no')]].forEach(function (o) {
    var opt = el('option', '', o[1]);
    opt.value = o[0];
    pick.appendChild(opt);
  });
  var marked = info.screenshot_mark != null;
  pick.value = info.screenshot ? '1' : '0';
  setTip(pick, tr('shot.hint'));
  var set = function (value) {
    post(LIBAPI + '/screenshots', { ids: [info.id], value: value }).then(function () { screenshotsChanged(info.id); }).catch(failed);
  };
  pick.onchange = function () { set(pick.value === '1'); };
  box.appendChild(pick);
  box.appendChild(el('span', 'note', ' ' + tr(marked ? 'shot.yours' : 'shot.guess')));
  if (marked) {
    // An undo icon at the right of the row; the words are its tooltip.
    var back = el('button', 'icon-btn shot-back');
    back.innerHTML = lineIcon('<path d="M9 5L4.5 9.5 9 14"/><path d="M5 9.5h9a5 5 0 0 1 0 10h-3"/>');
    setTip(back, tr('shot.back'));
    back.setAttribute('aria-label', tr('shot.back'));
    back.onclick = function () { set(null); };
    box.appendChild(back);
  }
  row(tr('shot.label'), box);
}

function screenshotsChanged(id) {
  // The Type filter may now show other photos (the open viewer keeps its photos until it closes).
  if (!state.filter.view && state.filter.types.length) loadTimeline(false);
  if (!lb.details || lb.details.id !== id) return;
  api(LIBAPI + '/files/' + id).then(function (info) {
    if (!lb.details || lb.details.id !== id) return;
    lb.details = info;
    renderPanel();
  }).catch(function () {});
}

function tagsChanged(id) {
  loadOwnTags();
  // A bulk change can change what a tag filter or search shows (the open
  // viewer keeps its photos until it closes).
  if (id == null && !state.filter.view && (state.filter.tags.length || state.filter.q)) loadTimeline(false);
  if (id == null || !lb.details || lb.details.id !== id) return;
  api(LIBAPI + '/files/' + id).then(function (info) {
    if (!lb.details || lb.details.id !== id) return;
    lb.details = info;
    coll.busy = false;
    markCollectionButton();
    renderPanel();
    markLightboxHide();
  }).catch(function () { coll.busy = false; markCollectionButton(); });
}

function addTagDialog(ids) {
  var body = el('div');
  body.appendChild(el('p', '', tr('tags.add_text', { count: trn('count.photos', ids.length) })));
  var input = tagInput();
  input.className = 'wide';
  body.appendChild(input);
  body.appendChild(el('p', 'hint', tr('tags.add_hint')));
  var error = el('p', 'error');
  body.appendChild(error);
  var go = function (btn) {
    if (!input.value.trim()) { error.textContent = tr('tags.type_name'); return false; }
    btn.disabled = true;
    post(LIBAPI + '/tags/add', { ids: ids, name: input.value }).then(function (r) {
      closeModal();
      toast(isFavTag(r.tag.name) ? tr('tags.reserved_fav') : isHiddenTag(r.tag.name) ? tr('tags.reserved_hidden')
        : r.files ? tr('tags.added', { count: trn('count.photos', r.files), name: r.tag.name }) : tr('tags.already', { name: r.tag.name }));
      tagsChanged(null);
      if (isFavTag(r.tag.name)) loadTimeline(false); // the hearts (any spelling came back as the tag)
      if (isHiddenTag(r.tag.name)) hiddenChanged(); // the photos are out of the list now
    }).catch(function (e) { btn.disabled = false; error.textContent = e.message; });
    return false;
  };
  var buttons = openModal(tr('tags.add_title'), body, [{ label: tr('app.cancel'), cls: 'quiet' }, { label: tr('tags.add_btn'), onclick: go }]);
  input.addEventListener('keydown', function (ev) { if (ev.key === 'Enter') go(buttons[1]); });
  input.focus();
}

// Lists the own tags on the selection; folder tags cannot be removed.
function removeTagDialog(ids) {
  post(LIBAPI + '/tags/selection', { ids: ids }).then(function (tags) {
    if (!tags.length) {
      openModal(tr('tags.remove_title'), tr('tags.none_own'), [{ label: tr('app.ok') }]);
      return;
    }
    var body = el('div');
    body.appendChild(el('p', '', tr('tags.remove_which', { count: trn('count.photos', ids.length) })));
    var list = el('div', 'taglist');
    tags.forEach(function (t) {
      var b = el('button', '', (isFavTag(t.name) ? '♥ ' + tr('fav.label') : isHiddenTag(t.name) ? tr('hidden.label') : t.name) + ' (' + I18n.number(t.count) + ')');
      b.onclick = function () {
        b.disabled = true;
        post(LIBAPI + '/tags/remove', { ids: ids, name: t.name }).then(function (r) {
          closeModal();
          toast(tr('tags.removed', { name: t.name, count: trn('count.photos', r.files) }));
          tagsChanged(null);
          if (isFavTag(t.name)) loadTimeline(false); // the hearts
          if (isHiddenTag(t.name)) hiddenChanged();
        }).catch(function (e) { closeModal(); failed(e); });
      };
      list.appendChild(b);
    });
    body.appendChild(list);
    openModal(tr('tags.remove_title'), body, [{ label: tr('app.cancel'), cls: 'quiet' }]);
  }).catch(failed);
}

// Sidebar: the tags the user added, most used first.
function loadOwnTags() {
  return api(LIBAPI + '/tags' + query({ own: 1, limit: 500 })).then(function (tags) {
    var list = $('own-tags');
    list.textContent = '';
    state.favCount = 0;
    tags = tags.filter(function (t) {
      if (isFavTag(t.name)) state.favCount = t.count;
      return !isFavTag(t.name) && !isHiddenTag(t.name);
    });
    if (state.favCount) {
      var fli = el('li'), fr = el('div', 'row');
      fli.id = 'fav-row';
      fr.appendChild(el('span', 'toggle heart', '♥'));
      var fname = el('button', 'name', tr('fav.label'));
      fname.onclick = function () { setFilter({ folder: null, tags: [], fav: true, q: '' }); };
      fr.appendChild(fname);
      fr.appendChild(el('span', 'count', I18n.number(state.favCount)));
      fli.appendChild(fr);
      list.appendChild(fli);
    }
    tags.forEach(function (t) {
      state.tagNames[t.id] = t.name;
      var li = el('li');
      var r = el('div', 'row');
      r.appendChild(el('span', 'toggle', '#'));
      var name = el('button', 'name', t.name);
      name.dataset.tag = t.id;
      name.onclick = function () { setFilter({ folder: null, tags: [t.id], q: '' }); };
      r.appendChild(name);
      r.appendChild(el('span', 'count', I18n.number(t.count)));
      li.appendChild(r);
      // Under the active tag: only the photos given it by hand. Only a tag that is
      // also a folder name has any other kind (a folder tag is not listed on its own).
      if (t.kind === 'both' && t.manual > 0) {
        var sr = el('div', 'row sub');
        var sname = el('button', 'name', tr('side.manual_only'));
        sname.dataset.manual = t.id;
        sname.onclick = function () { if (!state.filter.manual) setFilter(withFilter({ manual: true })); };
        sr.appendChild(sname);
        sr.appendChild(el('span', 'count', I18n.number(t.manual)));
        sr.hidden = true;
        li.appendChild(sr);
      }
      list.appendChild(li);
    });
    $('tags-section').hidden = !tags.length && !state.favCount && !state.hiddenCount;
    paintHiddenRow();
    markActiveFolder();
    renderChips();
  }).catch(function () {});
}

// ------------------------------------------------------------------ face check page
// Did recognition work? All faces as crops, sortable and filterable, so
// false detections (posters, statues, background) are easy to spot and can
// be marked "not a face" (one at a time, or several selected); a filter
// shows the marked ones, to undo a mistake.

var faceState = { sort: 'size', desc: false, filter: 'all', faces: [], total: 0, minPx: 30, picking: false, picked: {} };
// `pet`: a pet drawn by hand, whose species nobody said.
var PET_SPECIES = { cat: true, dog: true, pet: true };
// An element with `text`, led by the species' icon when it is a pet's (our own drawings, not emojis).
function petLabel(tag, cls, species, text) {
  var e = el(tag, cls);
  if (PET_SPECIES[species]) e.appendChild(speciesIcon(species));
  e.appendChild(document.createTextNode(text));
  return e;
}
function speciesName(species) { return tr('pet.species.' + (PET_SPECIES[species] ? species : 'pet')); }
// "3 cats", "1 dog": the pets of one species (or just pets).
function animalCount(species, n) { return trn('count.' + (species === 'cat' ? 'cats' : species === 'dog' ? 'dogs' : 'pets'), n); }

// The check page of people's faces (view `faces`) or of cats and dogs (view `pets`).
function checkKind() { return state.filter.view === 'pets' ? 'pets' : 'faces'; }
function onCheckPage() { return state.filter.view === 'faces' || state.filter.view === 'pets'; }
var PAGE_FACES = 300;
var FACE_SORTS = [
  ['size', false, 'facecheck.sort.smallest'], ['size', true, 'facecheck.sort.largest'],
  ['score', false, 'facecheck.sort.lowest'], ['score', true, 'facecheck.sort.highest'],
];
var FACE_FILTERS = [
  ['all', 'facecheck.filter.all'], ['small', 'facecheck.filter.small'], ['large', 'facecheck.filter.large'], ['rotated', 'facecheck.filter.rotated'],
  ['not_face', 'facecheck.filter.not_face'], ['pet_face', 'facecheck.filter.pet_face'], ['unmatched', 'facecheck.filter.unmatched'],
];
// Cats and dogs are not looked for in turned copies, and the false finds are no pets.
var PET_FILTERS = [
  ['all', 'facecheck.pet_filter.all'], ['small', 'facecheck.filter.small'], ['large', 'facecheck.filter.large'], ['not_face', 'facecheck.pet_filter.not_face'], ['unmatched', 'facecheck.filter.unmatched'],
];
function checkFilters() { return checkKind() === 'pets' ? PET_FILTERS : FACE_FILTERS; }

$('nav-settings').onclick = function () { showView('settings'); };

function loadFaces() {
  var page = $('page');
  var kind = checkKind();
  clearNode(page);
  var back = el('button', 'link back', tr('settings.back'));
  back.onclick = function () { showView('settings'); };
  page.appendChild(back);
  page.appendChild(el('h2', '', tr(kind === 'pets' ? 'side.pet_check' : 'side.face_check')));
  var sub = el('p', 'sub', tr('dups.looking'));
  page.appendChild(sub);
  if (!checkFilters().some(function (o) { return o[0] === faceState.filter; })) faceState.filter = 'all';
  page.appendChild(faceToolbar());
  var grid = el('div', 'faces has-drag');
  grid.dragSite = function () {
    if (!faceState.picking) return null;
    return {
      sel: '.face[data-id]',
      itemOf: function (tile) { return tile.face; },
      get: function (x) { return !!faceState.picked[x.id]; },
      set: function (x, on) {
        if (on) faceState.picked[x.id] = true; else delete faceState.picked[x.id];
        var c = document.querySelector('.face[data-id="' + x.id + '"]');
        if (c) c.classList.toggle('sel', on);
      },
      done: function (x) { if (x) faceState.anchor = x.id; updateFacePick(); },
    };
  };
  page.appendChild(grid);
  var more = el('button', 'btn quiet', tr('app.show_more'));
  more.hidden = true;
  more.onclick = function () { moreFaces(grid, more); };
  page.appendChild(more);
  page.appendChild(facePickBar());
  faceState.faces = [];
  faceState.picked = {};
  updateFacePick();
  api(LIBAPI + '/faces/stats' + query({ kind: kind === 'pets' ? 'pets' : null })).then(function (s) {
    if (checkKind() === kind && onCheckPage()) sub.textContent = faceSummary(s);
  }).catch(function () {});
  return moreFaces(grid, more);
}

function faceSummary(s) {
  var pets = s.kind === 'pets';
  var things = function (n) { return trn(pets ? 'count.pets' : 'count.faces', n); };
  var parts = [tr('facecheck.faces_in', { faces: things(s.faces), photos: trn('count.photos', s.looked - s.failed) })];
  if (s.failed) parts.push(tr('facecheck.failed', { count: trn('count.photos', s.failed) }));
  if (s.looked < s.photos) parts.push(tr('facecheck.todo', { n: s.photos - s.looked }));
  parts.push(tr('facecheck.small', { n: s.small, px: s.min_cluster_px }));
  if (!pets) parts.push(s.rotated_looked ? tr('facecheck.turned', { count: trn('count.faces', s.rotated_faces) }) : tr('facecheck.turned_none'));
  if (s.pet_faces) parts.push(tr('facecheck.pet_faces', { count: trn('count.faces', s.pet_faces) }));
  if (s.not_faces) parts.push(tr(pets ? 'facecheck.not_pets' : 'facecheck.not_faces', { count: things(s.not_faces) }));
  var widths = s.widths.map(function (b) {
    var label = b.from == null ? '< ' + b.to : b.to == null ? b.from + '+' : b.from + '–' + b.to;
    return label + ' px: ' + I18n.number(b.count);
  });
  var text = parts.join(' · ') + '. ' + tr('facecheck.widths', { widths: widths.join(', ') });
  if (s.unmatched) {
    var left = s.widths.filter(function (b) { return b.unmatched; }).map(function (b) {
      var label = b.from == null ? '< ' + b.to : b.to == null ? b.from + '+' : b.from + '–' + b.to;
      return label + ' px: ' + I18n.number(b.unmatched);
    });
    text += ' ' + tr('facecheck.unmatched', { count: things(s.unmatched), widths: left.join(', ') });
  }
  return text;
}

function faceToolbar() {
  var bar = el('div', 'toolbar');
  var sort = el('select');
  FACE_SORTS.forEach(function (o, k) {
    var opt = el('option', '', tr(o[2]));
    opt.value = k;
    opt.selected = o[0] === faceState.sort && o[1] === faceState.desc;
    sort.appendChild(opt);
  });
  sort.onchange = function () {
    var o = FACE_SORTS[sort.value];
    faceState.sort = o[0];
    faceState.desc = o[1];
    loadFaces();
  };
  var filter = el('select');
  checkFilters().forEach(function (o) {
    var opt = el('option', '', tr(o[1]));
    opt.value = o[0];
    opt.selected = o[0] === faceState.filter;
    filter.appendChild(opt);
  });
  filter.onchange = function () { faceState.filter = filter.value; loadFaces(); };
  var pick = el('button', 'btn quiet', tr('facecheck.select'));
  pick.id = 'face-pick';
  pick.onclick = function () {
    faceState.picking = !faceState.picking;
    if (!faceState.picking) clearFacePicks();
    updateFacePick();
  };
  bar.appendChild(sort);
  bar.appendChild(filter);
  bar.appendChild(pick);
  return bar;
}

// The bar for selected crops: mark them "not a face" (or undo that, in the
// filter that shows the marked ones).
function facePickBar() {
  var bar = el('div', 'selbar');
  bar.id = 'face-bar';
  bar.hidden = true;
  var count = el('span');
  count.id = 'face-pick-count';
  var mark = el('button', 'btn');
  mark.id = 'face-pick-mark';
  mark.onclick = function () { markFaces(Object.keys(faceState.picked).map(Number)); };
  var done = el('button', 'btn quiet', tr('app.done'));
  done.onclick = function () { faceState.picking = false; clearFacePicks(); updateFacePick(); };
  bar.appendChild(count);
  bar.appendChild(mark);
  bar.appendChild(done);
  return bar;
}

function clearFacePicks() {
  faceState.picked = {};
  faceState.anchor = null;
  Array.prototype.forEach.call(document.querySelectorAll('.face.sel'), function (c) { c.classList.remove('sel'); });
}

function updateFacePick() {
  var bar = $('face-bar');
  if (!bar) return;
  var n = Object.keys(faceState.picked).length;
  bar.hidden = !faceState.picking;
  $('face-pick').classList.toggle('active', faceState.picking);
  var pets = checkKind() === 'pets';
  $('face-pick-count').textContent = n ? trn(pets ? 'count.pets' : 'count.faces', n) : tr('facecheck.tap');
  var mark = $('face-pick-mark');
  var undoing = faceState.filter === 'not_face';
  mark.textContent = tr(pets ? (undoing ? 'facecheck.is_pet' : 'facecheck.not_pet') : (undoing ? 'facecheck.is_face' : 'facecheck.not_face'));
  mark.disabled = !n;
}

// Mark faces "not a face", or, in the filter of marked faces, undo that.
// They leave the list they were in.
function markFaces(ids) {
  if (!ids.length) return;
  var undo = faceState.filter === 'not_face';
  post(undo ? LIBAPI + '/faces/undo' : LIBAPI + '/faces/not-face', { faces: ids }).then(function (r) {
    ids.forEach(function (id) {
      var card = document.querySelector('.face[data-id="' + id + '"]');
      if (card) card.remove();
      delete faceState.picked[id];
    });
    var before = faceState.faces.length;
    faceState.faces = faceState.faces.filter(function (f) { return ids.indexOf(f.id) < 0; });
    faceState.total -= before - faceState.faces.length;
    updateFacePick();
    toast(tr(undo ? 'facecheck.back' : 'facecheck.marked', { count: trn('count.faces', r.faces) }));
  }).catch(failed);
}

function moreFaces(grid, more) {
  var f = faceState.filter;
  var q = query({
    kind: checkKind() === 'pets' ? 'pets' : null,
    sort: faceState.sort, desc: faceState.desc ? 'true' : null, offset: faceState.faces.length, limit: PAGE_FACES,
    max_px: f === 'small' ? faceState.minPx : null, min_px: f === 'large' ? faceState.minPx : null,
    rotated: f === 'rotated' ? 'true' : null, not_face: f === 'not_face' ? 'true' : null, pet_face: f === 'pet_face' ? 'true' : null, unmatched: f === 'unmatched' ? 'true' : null,
  });
  more.disabled = true;
  return api(LIBAPI + '/faces' + q).then(function (r) {
    if (!onCheckPage()) return;
    faceState.total = r.total;
    faceState.minPx = r.min_cluster_px;
    r.faces.forEach(function (face) { faceState.faces.push(face); grid.appendChild(faceCard(face, null)); });
    if (!faceState.total) grid.appendChild(el('p', 'sub', tr(checkKind() === 'pets' ? 'facecheck.none_pets' : 'facecheck.none')));
    more.disabled = false;
    more.hidden = faceState.faces.length >= faceState.total;
  }).catch(failed);
}

// One crop; clicking it opens its photo (or selects it, while selecting).
// `similarity` is set in the list of nearest neighbours.
function faceCard(face, similarity) {
  var card = el('div', 'face' + (face.small ? ' small' : ''));
  card.dataset.id = face.id;
  card.face = face;
  var a = el('a', 'crop');
  a.href = '#';
  setTip(a, tr('facecheck.open_photo'));
  var img = el('img');
  img.alt = '';
  img.loading = 'lazy';
  img.src = LIBAPI + '/faces/' + face.id + '/crop';
  img.onerror = function () { card.classList.add('broken'); };
  a.appendChild(img);
  a.onclick = function (ev) {
    ev.preventDefault();
    if (faceState.picking && similarity == null) {
      var span = ev.shiftKey && pickSpan(faceState.faces, function (x) { return x.id; }, faceState.anchor, face.id);
      if (span) {
        span.forEach(function (x) {
          faceState.picked[x.id] = true;
          var c = document.querySelector('.face[data-id="' + x.id + '"]');
          if (c) c.classList.add('sel');
        });
      } else {
        if (faceState.picked[face.id]) delete faceState.picked[face.id]; else faceState.picked[face.id] = true;
        card.classList.toggle('sel', !!faceState.picked[face.id]);
        faceState.anchor = face.id;
      }
      updateFacePick();
      return;
    }
    closeModal();
    openFacePhoto(face);
  };
  card.appendChild(a);
  var caption = Math.round(face.px) + ' px · ' + face.score.toFixed(2);
  if (similarity != null) caption = similarity.toFixed(2) + ' · ' + caption;
  card.appendChild(el('div', 'meta', caption));
  if (face.species) card.appendChild(petLabel('span', 'tag', face.species, speciesName(face.species)));
  if (face.small) card.appendChild(el('span', 'tag', tr('facecheck.tag_small')));
  if (face.roll) card.appendChild(el('span', 'tag', tr('facecheck.tag_turned')));
  if (similarity == null) {
    var near = el('button', 'near', '≈');
    setTip(near, tr(face.species ? 'facecheck.similar_pets' : 'facecheck.similar_btn'));
    near.onclick = function () { similarFaces(face); };
    card.appendChild(near);
    var undo = faceState.filter === 'not_face';
    var mark = el('button', 'mark', undo ? '↺' : '✕');
    setTip(mark, face.species ? tr(undo ? 'facecheck.undo_hint_pet' : 'facecheck.not_pet') : tr(undo ? 'facecheck.undo_hint' : 'facecheck.not_face'));
    mark.onclick = function () { markFaces([face.id]); };
    card.appendChild(mark);
  }
  return card;
}

// The nearest neighbours by embedding (read only): which similarity still
// means "same person" is what 5c-2 needs to know.
function similarFaces(face) {
  api(LIBAPI + '/faces/' + face.id + '/similar?limit=24').then(function (list) {
    var box = el('div');
    box.appendChild(el('p', 'hint', tr(face.species ? 'facecheck.similar_hint_pet' : 'facecheck.similar_hint')));
    var grid = el('div', 'faces');
    grid.appendChild(faceCard(face, 1));
    list.forEach(function (n) { grid.appendChild(faceCard(n, n.similarity)); });
    box.appendChild(grid);
    openModal(tr(face.species ? 'facecheck.similar_pets' : 'facecheck.similar_btn'), box, [{ label: tr('app.close') }]);
  }).catch(failed);
}

// Open one photo in the viewer from the page, and come back to it when the
// viewer closes.
// `siblings` (optional): the faces shown with it, e.g. in a cluster card; the
// arrow keys then step through their photos (once per photo).
// `onBack(changed)` (optional): called when the viewer closes, `changed` true
// if faces were decided in it.
function openFacePhoto(face, siblings, onBack) {
  var list = [], seen = {}, at = 0;
  (siblings || [face]).forEach(function (f) {
    if (f.file == null || seen[f.file]) return;
    seen[f.file] = true;
    list.push(f);
  });
  list.forEach(function (f, k) { if (f.file === face.file) at = k; });
  if (!list.length) list = [face];
  openFiles(list, at, onBack);
}

// Opens the viewer over these files ({ file, kind, version }) instead of the
// timeline, starting at `at`; closing it goes back to where it was opened
// (and calls `onBack` as openFacePhoto says).
function openFiles(list, at, onBack) {
  var saved = state.data;
  state.data = {
    count: list.length,
    ids: list.map(function (f) { return f.file; }),
    kinds: list.map(function (f) { return { jpeg: 'j', png: 'p', heic: 'h', video: 'v' }[f.kind] || 'j'; }).join(''),
    days: list.map(function () { return 0; }),
    versions: list.map(function (f) { return ((f.version || '') + '00000000').slice(0, 8); }).join(''),
    live: [],
  };
  // Back to the thumbnail it was opened from (keyboard: Tab, Space, Space).
  var from = document.activeElement;
  lb.facesChanged = false;
  lb.restore = function () {
    state.data = saved;
    if (from && from.isConnected && from.focus) from.focus();
    if (onBack) onBack(lb.facesChanged);
  };
  openLightbox(at);
}

// ------------------------------------------------------------------ settings

// Settings: for now the calibration pages, which show what recognition found
// so thresholds and false finds can be judged on the real photos.
function loadSettings() {
  var page = $('page');
  clearNode(page);
  page.appendChild(el('h2', '', tr('settings.title')));
  var sec = el('section', 'settings-section');
  sec.appendChild(el('h3', '', tr('settings.calibration')));
  sec.appendChild(el('p', 'hint', tr('settings.calibration_hint')));
  var cards = el('div', 'settings-cards');
  [
    ['faces', tr('side.face_check'), tr('settings.faces_desc')],
    ['pets', tr('side.pet_check'), tr('settings.pets_desc')],
    ['text', tr('side.text_check'), tr('settings.text_desc')],
  ].forEach(function (c) {
    var card = el('button', 'settings-card');
    card.type = 'button';
    card.appendChild(el('span', 'title', c[1]));
    card.appendChild(el('span', 'hint', c[2]));
    var line = el('span', 'sub', tr('dups.looking'));
    card.appendChild(line);
    card.onclick = function () { showView(c[0]); };
    cards.appendChild(card);
    if (c[0] === 'text') {
      api(LIBAPI + '/text/stats').then(function (s) {
        line.textContent = s.read
          ? tr('textcheck.summary_short', { lines: s.lines, photos: trn('count.photos', s.with_text), read: s.read })
          : tr('settings.not_yet_text');
      }).catch(function () { line.textContent = ''; });
      return;
    }
    api(LIBAPI + '/faces/stats' + query({ kind: c[0] === 'pets' ? 'pets' : null })).then(function (s) {
      line.textContent = s.faces
        ? tr('facecheck.faces_in', { faces: trn(c[0] === 'pets' ? 'count.pets' : 'count.faces', s.faces), photos: trn('count.photos', s.looked - s.failed) })
        : s.looked ? tr('settings.looked_none', { photos: trn('count.photos', s.looked) })
          : tr(c[0] === 'pets' ? 'settings.not_yet_pets' : 'settings.not_yet_faces');
    }).catch(function () { line.textContent = ''; });
  });
  sec.appendChild(cards);
  page.appendChild(sec);
  page.appendChild(cleanupSection());
  page.appendChild(transferSection());
  page.appendChild(allowTrashSection());
  page.appendChild(mapsSection());
  page.appendChild(collectionSection());
  page.appendChild(exportSection());
  page.appendChild(eventPatternSection());
  page.appendChild(copyFoldersSection());
}

// Settings: the words read in the photos live on the drive (`recognition.db`);
// this forgets them all. Photos are read again by the next "Recognize text".
// "Data cleanup": what recognition found, forgotten again, as three cards in a
// row (text, faces, pets) with their buttons.
function cleanupSection() {
  var sec = el('section', 'settings-section');
  sec.appendChild(el('h3', '', tr('cleanup.title')));
  sec.appendChild(el('p', 'hint', tr('cleanup.hint')));
  var cards = el('div', 'settings-cards cleanup');
  cards.appendChild(textCleanupCard());
  cards.appendChild(forgetCard('faces'));
  cards.appendChild(forgetCard('pets'));
  sec.appendChild(cards);
  return sec;
}

function textCleanupCard() {
  var sec = el('div', 'cleanup-card');
  sec.appendChild(el('span', 'title', tr('settings.text')));
  sec.appendChild(el('p', 'hint', tr('settings.text_hint')));
  var line = el('p', 'sub', '');
  sec.appendChild(line);
  var del = el('button', 'btn quiet', tr('settings.text_delete'));
  del.type = 'button';
  del.disabled = true;
  api(LIBAPI + '/text/stats').then(function (s) {
    line.textContent = s.stored ? tr('settings.text_stored', { lines: s.stored, photos: trn('count.photos', s.read) }) : tr('settings.not_yet_text');
    del.disabled = !s.stored && !s.read;
  }).catch(function () {});
  del.onclick = function () {
    openModal(tr('settings.text_delete'), tr('settings.text_delete_confirm'), [
      { label: tr('app.cancel'), cls: 'quiet', focus: true },
      { label: tr('settings.text_delete'), cls: 'danger', onclick: function () {
        post(LIBAPI + '/text/delete-all', {}).then(function (r) {
          toast(tr('settings.text_deleted', { n: r.lines }));
          changed();
          if (state.filter.view === 'settings') loadSettings();
        }).catch(failed);
      } },
    ]);
  };
  var btns = el('div', 'btns');
  btns.appendChild(del);
  sec.appendChild(btns);
  return sec;
}

// Forget what recognition found, for faces or for pets (`forget.rs`): the
// automatic finds only (names and decisions stay and take hold again after the
// next "Recognize"), or everything including the user's own work.
function forgetCard(space) {
  var what = space === 'pets' ? tr('forget.what_pets') : tr('forget.what_faces');
  var sec = el('div', 'cleanup-card');
  sec.appendChild(el('span', 'title', space === 'pets' ? tr('forget.title_pets') : tr('forget.title_faces')));
  sec.appendChild(el('p', 'hint', tr('forget.hint', { what: what })));
  var line = el('p', 'sub', '');
  sec.appendChild(line);
  var auto = el('button', 'btn quiet', tr('forget.auto'));
  var all = el('button', 'btn quiet', tr('forget.all'));
  auto.type = all.type = 'button';
  auto.disabled = all.disabled = true;
  var counts = null;
  api(LIBAPI + '/recognition/forget').then(function (c) {
    counts = c[space];
    line.textContent = counts.finds || counts.decisions
      ? tr('forget.stored', { finds: counts.finds, photos: counts.photos, decisions: counts.decisions, people: counts.people })
      : tr('forget.nothing', { what: what });
    auto.disabled = !counts.finds && !counts.photos;
    all.disabled = !counts.finds && !counts.photos && !counts.decisions;
  }).catch(function () {});
  function run(everything) {
    post(LIBAPI + '/recognition/forget', { space: space, everything: everything }).then(function (r) {
      toast(tr('forget.done', { n: r.finds + r.decisions }));
      changed();
      if (state.filter.view === 'settings') loadSettings();
    }).catch(failed);
  }
  auto.onclick = function () {
    openModal(tr('forget.auto'), tr('forget.auto_confirm', { what: what, finds: counts.finds }), [
      { label: tr('app.cancel'), cls: 'quiet', focus: true },
      { label: tr('forget.auto'), cls: 'danger', onclick: function () { run(false); } },
    ]);
  };
  all.onclick = function () {
    openModal(tr('forget.all'), tr('forget.all_confirm', { what: what, finds: counts.finds, decisions: counts.decisions, people: counts.people }), [
      { label: tr('app.cancel'), cls: 'quiet', focus: true },
      { label: tr('forget.all'), cls: 'danger', onclick: function () { run(true); } },
    ]);
  };
  var btns = el('div', 'btns');
  btns.appendChild(auto);
  btns.appendChild(all);
  sec.appendChild(btns);
  return sec;
}

// The Text check (view `text`): which lines of the recognized text count for
// the search and the viewer, and the lines near the limits, with a crop of
// each, so the limits can be judged on the real photos.
function loadTextCheck() {
  var page = $('page');
  clearNode(page);
  page.appendChild(el('h2', '', tr('side.text_check')));
  page.appendChild(el('p', 'hint', tr('textcheck.hint')));
  var body = el('div', 'textcheck');
  page.appendChild(body);
  Promise.all([api(LIBAPI + '/text/stats'), api(LIBAPI + '/text/check')]).then(function (r) {
    if (state.filter.view !== 'text') return;
    renderTextCheck(body, r[0], r[1]);
  }).catch(function (e) { body.textContent = e.message; });
}

function renderTextCheck(body, s, check) {
  clearNode(body);
  if (!s.read && !s.stored) {
    body.appendChild(el('p', 'sub', tr('textcheck.none')));
    return;
  }
  var summary = el('p', 'sub tc-summary');
  body.appendChild(summary);
  var lim = check.limits;
  var controls = el('div', 'tc-controls');
  var sliders = {};
  var slider = function (key, label, min, max, step, value, fmt) {
    var row = el('label', 'tc-slider');
    row.appendChild(el('span', 'tc-label', label));
    var input = el('input');
    input.type = 'range'; input.min = min; input.max = max; input.step = step; input.value = value;
    var out = el('span', 'tc-value', fmt(value));
    input.oninput = function () { out.textContent = fmt(input.value); };
    input.onchange = apply;
    row.appendChild(input);
    row.appendChild(out);
    sliders[key] = { input: input, out: out, fmt: fmt };
    return row;
  };
  // The controls stay where they are while the numbers and the lines below are fetched again.
  var apply = function () {
    post(LIBAPI + '/text/limits', { min_score: sliders.score.input.value / 100, min_height: sliders.height.input.value / 1000 })
      .then(function () { refresh(); changed(); }).catch(failed);
  };
  controls.appendChild(slider('score', tr('textcheck.score'), 50, 100, 1, Math.round(lim.min_score * 100), function (v) { return (v / 100).toFixed(2); }));
  controls.appendChild(slider('height', tr('textcheck.height'), 4, 80, 1, Math.round(lim.min_height * 1000), function (v) { return (v / 10).toFixed(1) + ' %'; }));
  var reset = el('button', 'btn quiet', tr('textcheck.reset'));
  reset.type = 'button';
  reset.onclick = function () {
    post(LIBAPI + '/text/limits', {}).then(function (l) {
      sliders.score.input.value = Math.round(l.min_score * 100); sliders.score.out.textContent = sliders.score.fmt(sliders.score.input.value);
      sliders.height.input.value = Math.round(l.min_height * 1000); sliders.height.out.textContent = sliders.height.fmt(sliders.height.input.value);
      refresh(); changed();
    }).catch(failed);
  };
  controls.appendChild(reset);
  body.appendChild(controls);
  body.appendChild(el('p', 'hint', tr('textcheck.limits_hint')));
  var dyn = el('div', 'tc-dyn');
  body.appendChild(dyn);
  var fill = function (s, check) {
    summary.textContent = tr('textcheck.summary', { lines: s.lines, photos: trn('count.photos', s.with_text), read: s.read, stored: s.stored });
    dyn.textContent = '';
    if (!check.lines.length) { dyn.appendChild(el('p', 'sub', tr('textcheck.nothing_near'))); return; }
    var grid = el('div', 'tc-grid');
    check.lines.forEach(function (l) {
      var card = el('div', 'tc-card' + (l.shown ? ' in' : ' out'));
      var img = el('img');
      img.loading = 'lazy';
      img.alt = '';
      img.src = LIBAPI + '/files/' + l.id + '/text-crop' + query({ x: l.x, y: l.y, w: l.w, h: l.h });
      card.appendChild(img);
      card.appendChild(el('div', 'tc-text', l.text));
      var meta = el('div', 'tc-meta');
      meta.appendChild(el('span', '', l.score.toFixed(2) + ' · ' + (l.h * 100).toFixed(1) + ' %'));
      meta.appendChild(el('span', 'pill ' + (l.shown ? 'ok' : ''), tr(l.shown ? 'textcheck.counts' : 'textcheck.left_out')));
      card.appendChild(meta);
      grid.appendChild(card);
    });
    dyn.appendChild(grid);
  };
  var refresh = function () {
    Promise.all([api(LIBAPI + '/text/stats'), api(LIBAPI + '/text/check')]).then(function (r) {
      if (state.filter.view === 'text') fill(r[0], r[1]);
    }).catch(function (e) { dyn.textContent = e.message; });
  };
  fill(s, check);
}

// Settings: Carry over data to this drive (phase 15b), also with one drive
// only: from a userdata.json (a drive that is gone, or this drive's own after
// its index was lost), or from another drive as on Manage drives. Shown where
// there is something to take from: another drive, or this computer's files.
function transferSection() {
  var sec = el('section', 'settings-section');
  sec.hidden = true;
  sec.appendChild(el('h3', '', tr('transfer.title')));
  sec.appendChild(el('p', 'hint', tr('transfer.settings_hint')));
  var btn = el('button', 'btn', tr('transfer.button'));
  sec.appendChild(btn);
  api('/api/all/drives').then(function (list) {
    var me = list.filter(function (x) { return x.library === drives.current.id; })[0];
    if (!me || !me.online || !me.shown_in_all) return;
    var others = list.some(function (x) { return x.online && x.shown_in_all && x.library !== me.library; });
    if (!others && !state.exportOk) return;
    btn.onclick = function () { transferDialog(list, me, null); };
    sec.hidden = false;
  }).catch(function () {});
  return sec;
}

// Settings: whether photos can be moved to the trash from the photo view and
// the timeline selection. Off by default: shoebox leaves the originals alone.
function allowTrashSection() {
  var sec = el('section', 'settings-section');
  sec.appendChild(el('h3', '', tr('settings.allow_trash')));
  var label = el('label', 'check-row');
  var box = el('input');
  box.type = 'checkbox';
  box.checked = allowTrash;
  box.disabled = readOnly;
  box.onchange = function () {
    var want = box.checked;
    post(LIBAPI + '/allow-trash', { allow: want }).then(function (r) {
      allowTrash = !!r.allow;
      applyAllowTrash();
      toast(tr('settings.allow_trash_saved'));
    }).catch(function (e) { box.checked = allowTrash; failed(e); });
  };
  label.appendChild(box);
  label.appendChild(document.createTextNode(' ' + tr('settings.allow_trash_label')));
  sec.appendChild(label);
  sec.appendChild(el('p', 'hint', tr(readOnly ? 'settings.allow_trash_readonly' : 'settings.allow_trash_hint')));
  return sec;
}

// How event folders are named when the app creates them (the library's
// setting, e.g. "YYYY-MM Name"); the scanner reads every form.
var eventPattern = 'YYYY-MM Name';

function loadEventPattern() {
  return api(LIBAPI + '/event-pattern').then(function (r) { eventPattern = r.pattern; }).catch(function () {});
}

// The folder name for a year, month and event name, as the server makes it.
function eventFolderName(year, month, name, pattern) {
  var m = /^(YYYY|YY)([.-])MM([ _.-]?)Name$/.exec(pattern || eventPattern) || ['', 'YYYY', '-', ' '];
  var y = !year ? (m[1] === 'YYYY' ? '????' : '??')
    : m[1] === 'YYYY' ? ('000' + year).slice(-4) : ('0' + (year % 100)).slice(-2);
  return y + m[2] + (month < 10 ? '0' : '') + month + m[3] + name;
}

// The pattern written with the letters of the UI language (JJJJ-MM Name).
function patternLabel(pattern) {
  var keys = { YYYY: 'pattern.year4', YY: 'pattern.year2', MM: 'pattern.month', Name: 'pattern.name' };
  return pattern.replace(/YYYY|YY|MM|Name/g, function (m) { return tr(keys[m]); });
}

// Settings: how new event folders are named. Three choices, so the result
// always fits what the scanner reads (no free text to get wrong).
function eventPatternSection() {
  var sec = el('section', 'settings-section');
  sec.appendChild(el('h3', '', tr('settings.event_pattern')));
  sec.appendChild(el('p', 'hint', tr('settings.event_pattern_hint')));
  var row = el('div', 'copy-folder-row');
  var choose = function (label, options, current) {
    var wrap = el('label', 'pattern-choice', label + ' ');
    var sel = el('select');
    options.forEach(function (o) {
      var opt = el('option', '', o[1]);
      opt.value = o[0];
      if (o[0] === current) opt.selected = true;
      sel.appendChild(opt);
    });
    wrap.appendChild(sel);
    row.appendChild(wrap);
    return sel;
  };
  var m = /^(YYYY|YY)([.-])MM([ _.-]?)Name$/.exec(eventPattern) || ['', 'YYYY', '-', ' '];
  var year = choose(tr('settings.event_year'), [['YYYY', tr('settings.event_year4')], ['YY', tr('settings.event_year2')]], m[1]);
  var sep = choose(tr('settings.event_sep'), [['-', tr('settings.sep_dash')], ['.', tr('settings.sep_dot')]], m[2]);
  var gap = choose(tr('settings.event_gap'), [[' ', tr('settings.gap_space')], ['_', tr('settings.gap_underscore')],
    ['.', tr('settings.gap_dot')], ['-', tr('settings.gap_dash')], ['', tr('settings.gap_none')]], m[3]);
  var preview = el('p', 'hint', '');
  var error = el('p', 'error');
  var current = function () { return year.value + sep.value + 'MM' + gap.value + 'Name'; };
  var draw = function () {
    preview.textContent = tr('settings.event_preview', {
      example: eventFolderName(2020, 7, tr('pattern.example_name'), current()),
      pattern: patternLabel(current()),
    });
  };
  [year, sep, gap].forEach(function (sel) {
    sel.onchange = function () {
      error.textContent = '';
      draw();
      post(LIBAPI + '/event-pattern', { pattern: current() }).then(function (r) {
        eventPattern = r.pattern;
        toast(tr('settings.event_saved'));
      }).catch(function (e) { error.textContent = e.message; });
    };
  });
  draw();
  sec.appendChild(row);
  sec.appendChild(preview);
  sec.appendChild(error);
  return sec;
}

// Folder names whose files are copies, not originals (InDesign's "Links").
function copyFoldersSection() {
  var sec = el('section', 'settings-section');
  sec.appendChild(el('h3', '', tr('settings.copy_folders')));
  sec.appendChild(el('p', 'hint', tr('settings.copy_folders_hint')));
  var chips = el('div', 'tagline');
  var folders = [];
  var save = function (next) {
    return post(LIBAPI + '/duplicates/copy-folders', { folders: next }).then(function (r) {
      folders = r.folders;
      draw();
    }).catch(failed);
  };
  var draw = function () {
    chips.textContent = '';
    if (!folders.length) chips.appendChild(el('span', 'hint', tr('settings.copy_folders_none')));
    folders.forEach(function (name) {
      var chip = el('span', 'chip own', name + ' ');
      var x = el('button', 'chip-x', '✕');
      x.type = 'button';
      setTip(x, tr('settings.copy_folders_remove'));
      x.onclick = function () { save(folders.filter(function (f) { return f !== name; })); };
      chip.appendChild(x);
      chips.appendChild(chip);
    });
  };
  var row = el('div', 'copy-folder-row');
  var input = el('input');
  input.type = 'text';
  input.placeholder = tr('settings.copy_folders_placeholder');
  var add = el('button', 'btn', tr('settings.copy_folders_add'));
  var go = function () {
    var name = input.value.trim();
    if (!name) return;
    input.value = '';
    save(folders.concat([name]));
  };
  add.onclick = go;
  input.addEventListener('keydown', function (ev) { if (ev.key === 'Enter') go(); });
  row.appendChild(input);
  row.appendChild(add);
  sec.appendChild(chips);
  sec.appendChild(row);
  api(LIBAPI + '/duplicates/copy-folders').then(function (r) { folders = r.folders; draw(); }).catch(function () { draw(); });
  return sec;
}

// ------------------------------------------------------------------ faces: people, groups, unnamed (5c-3)
// The sidebar's "Faces" section, the people overview, a person's faces
// (confirmed, suggested, maybe, rejected), the unnamed clusters, groups,
// and the people in the info panel. Everything said about a face is a
// decision on the server; the browser keeps nothing. Every action has a
// visible button (no hover-only actions), so it all works by touch.

var people = { list: [], byId: {}, groups: [], open: {}, info: null, watch: null, stream: null, streamUrl: null, told: null };

function fold(s) { return String(s).normalize('NFC').toLowerCase(); }

// One request pair at a time (see `loadInfo`).
var peopleFlight = null, peopleAgain = false;

function loadPeople() {
  if (peopleFlight) { peopleAgain = true; return peopleFlight; }
  peopleFlight = fetchPeople().then(function () {
    peopleFlight = null;
    if (peopleAgain) { peopleAgain = false; loadPeople(); }
  });
  return peopleFlight;
}

function fetchPeople() {
  people.stale = false;
  return Promise.all([api(LIBAPI + '/people'), api(LIBAPI + '/groups')]).then(function (r) {
    people.list = r[0];
    people.groups = r[1];
    people.byId = {};
    r[0].forEach(function (p) { people.byId[p.id] = p; state.personNames[p.id] = p.name; state.personSpecies[p.id] = p.species; });
    r[1].forEach(function (g) { state.groupNames[g.id] = g.name; });
    peopleShown();
  }).catch(function () {});
}

// Show the people and groups as they are now. Also after a change the page
// made itself from the reply (`takePerson`, `countGroups`), without asking the
// server for the whole list again.
function peopleShown() {
  people.groups.forEach(function (g) { state.groupNames[g.id] = g.name; });
  sortPeople();
  renderFacesSection();
  renderChips();
  // A group in the search that is gone (its last person left): the search drops it.
  var f = state.filter, there = function (id) { return people.groups.some(function (g) { return g.id === id; }); };
  if (!isAll() && f.groups && !f.groups.every(there)) setFilter(withFilter({ groups: f.groups.filter(there) }));
}

// The server's order (`people::people`): by group (people without one last),
// then name, then id.
function sortPeople() {
  var position = {};
  people.groups.forEach(function (g) { position[g.id] = g.position; });
  var rank = function (p) { return position[p.group_id] == null ? Infinity : position[p.group_id]; };
  people.list.sort(function (a, b) {
    var ra = rank(a), rb = rank(b);
    if (ra !== rb) return ra < rb ? -1 : 1;
    var na = a.name.toLowerCase(), nb = b.name.toLowerCase();
    if (na !== nb) return na < nb ? -1 : 1;
    return a.id - b.id;
  });
}

// The reply to a change of one person (their group, their picture) is the
// person as they are now: it takes the place of their entry (or, with `add`,
// is added). False for someone the list does not have (a hidden person).
function takePerson(r, add) {
  var at = -1;
  people.list.forEach(function (p, k) { if (p.id === r.id) at = k; });
  if (at < 0 && !add) return false;
  if (at < 0) people.list.push(r); else people.list[at] = r;
  people.byId[r.id] = r;
  state.personNames[r.id] = r.name;
  state.personSpecies[r.id] = r.species;
  return true;
}

function personUpdated(r) {
  if (takePerson(r)) peopleShown();
}

// A person was deleted: gone from the list, and from their group (which goes
// with its last person).
function personRemoved(id) {
  var mine = people.byId[id];
  if (!mine) loadPeople();
  else {
    people.list = people.list.filter(function (x) { return x.id !== id; });
    delete people.byId[id];
    countGroups(mine.group_id, null);
    peopleShown();
  }
  peopleInfoChanged();
}

// `from` was merged into the person the reply describes. That one takes
// `from`'s group if they had none; the group `from` left may be empty then.
function peopleMerged(fromId, r) {
  var from = people.byId[fromId], into = people.byId[r.id];
  if (!from || !into) loadPeople();
  else {
    countGroups(into.group_id, r.group_id);
    countGroups(from.group_id, null);
    people.list = people.list.filter(function (x) { return x.id !== fromId; });
    delete people.byId[fromId];
    takePerson(r);
    peopleShown();
  }
  peopleInfoChanged();
}

// One person left group `from` for `to` (either may be none): the groups'
// counts follow, and a group nobody is in any more is gone, as on the server.
function countGroups(from, to) {
  if (from === to) return;
  people.groups.forEach(function (g) {
    if (g.id === from) g.people--;
    if (g.id === to) g.people++;
  });
  people.groups = people.groups.filter(function (g) { return g.people > 0; });
}

// Groups in their order with their people, then "No group" (only when
// someone is in it).
function groupedPeople() {
  var out = people.groups.map(function (g) { return { id: g.id, name: g.name, people: [] }; });
  var byGroup = {};
  out.forEach(function (s) { byGroup[s.id] = s; });
  var none = { id: null, name: tr('people.no_group'), people: [] };
  people.list.forEach(function (p) { (byGroup[p.group_id] || none).people.push(p); });
  if (none.people.length) out.push(none);
  return out;
}

// /api/info: whether there are faces at all, how many clusters wait for a
// name; keep watching while the clusters are being recomputed.
function facesInfo(info) {
  var before = people.info;
  people.info = info;
  var busy = info.clusters.stale || info.clusters.running || info.clusters.embedding;
  if (before && (before.clusters.stale || before.clusters.running || before.clusters.embedding) && !busy) {
    // Recomputed: the suggestions changed. Only the People page shows them;
    // anywhere else the list is asked for when that page opens.
    if (state.filter.view === 'people') loadPeople().then(loadPeoplePage);
    else people.stale = true;
    if (state.filter.view === 'person') loadPersonCounts();
  }
  if (busy) watchClusters();
  regroupLine(info);
  // Faces grouped anew in the background (after a decision, a recognize run):
  // the Unnamed page takes the new cards and suggestions, once when the grouping
  // is done and not at each step of it (asked for, running, done).
  if (before && !busy && clustersSignature(before) !== clustersSignature(info) && state.filter.view === 'unnamed') scheduleUnnamedRefresh();
  renderFacesSection();
}

// What a grouping changes. Not the people named: names wait for the next
// grouping (`pending`) and the cards that took them already show them.
function clustersSignature(info) {
  var c = info.clusters;
  return [c.unnamed, c.clusters, c.suggested, c.finished_at, c.stale, c.running, c.embedding].join(',');
}

// While the clusters are being recomputed the server says when something
// changed (`facesStream`); only without that stream (it could not connect)
// does the page look every 1.5 s.
function watchClusters() {
  if (people.watch || streamOpen()) return;
  people.watch = setTimeout(function () { people.watch = null; loadInfo(); }, 1500);
}

function streamOpen() {
  return !!people.stream && people.stream.readyState === 1 && people.streamUrl === LIBAPI + '/events';
}

// Server-sent events of the library shown: "faces" when clusters are asked
// for, being recomputed or done, or faces drawn by hand are being embedded.
// The status and the Unnamed page follow from `/api/info` (`facesInfo`). The
// browser reconnects by itself (an iPad waking up); a stream the server ended
// for good (a drive unplugged) is opened again at the next status check.
function facesStream() {
  if (!window.EventSource || !LIBAPI) return;
  var url = LIBAPI + '/events';
  if (people.stream && people.streamUrl === url && people.stream.readyState !== 2) return;
  if (people.stream) people.stream.close();
  var es = new EventSource(url);
  people.stream = es;
  people.streamUrl = url;
  es.addEventListener('faces', function () {
    clearTimeout(people.told);
    people.told = setTimeout(loadInfo, 50);
  });
  // Connected again: what happened meanwhile is in the status.
  es.onopen = function () { if (people.watch) { clearTimeout(people.watch); people.watch = null; } loadInfo(); };
  es.onerror = function () { if (es.readyState === 2 && people.info) facesInfo(people.info); };
}

// After a change to people or decisions (the Unnamed page follows at once,
// also when the change was made in the viewer over it).
//
// `decided` is the reply to naming faces; it carries the person as the list
// shows them (`person_data`): they are added (someone new) or replace their
// entry, so the whole list is not asked for again. Only a reply without them,
// or a person whose picture the server could not tell, asks for the list.
//
// `onUnnamed`: the change was made on the Unnamed page, whose card or faces
// already show it; the page is asked for its clusters once, when the grouping
// that follows is done (`facesInfo`), not at once as well.
function peopleChanged(decided, onUnnamed) {
  var d = decided && decided.person_data;
  if (d && (d.cover != null || d.cover_manual != null)) {
    var old = people.byId[d.id];
    // The suggestions come with the next grouping (and the list is asked for then).
    if (old) { d.suggested = old.suggested; d.maybe = old.maybe; }
    takePerson(d, true);
    peopleShown();
  } else loadPeople();
  peopleInfoChanged(onUnnamed);
}

// What follows from any change to people: the status (how many are named) and
// the Unnamed page (the names in its suggestions).
function peopleInfoChanged(onUnnamed) {
  loadInfo();
  if (state.filter.view === 'unnamed' && !onUnnamed) scheduleUnnamedRefresh();
}

function cropUrl(face) {
  if (face.manual != null) return LIBAPI + '/faces/manual/' + face.manual + '/crop';
  if (face.id != null) return LIBAPI + '/faces/' + face.id + '/crop';
  return null;
}

// A round face: the person's cover, or their initials.
function avatar(p, cls) {
  var a = el('span', 'avatar' + (cls ? ' ' + cls : '') + (p && p.species ? ' pet pet-' + p.species : ''));
  if (p && p.species) a.title = p.species;
  if (p && (p.cover != null || p.cover_manual != null)) {
    var img = el('img');
    img.alt = '';
    img.loading = 'lazy';
    img.src = p.cover != null ? LIBAPI + '/faces/' + p.cover + '/crop' : LIBAPI + '/faces/manual/' + p.cover_manual + '/crop';
    // A cover drawn on a video has no crop: show the initials.
    img.onerror = function () { img.remove(); a.textContent = initials(p); };
    a.appendChild(img);
  } else {
    a.textContent = initials(p);
  }
  return a;
}

function initials(p) {
  return p ? p.name.split(/\s+/).map(function (w) { return w.charAt(0); }).join('').slice(0, 2).toUpperCase() : '?';
}

// A face cut out of its photo's thumbnail here in the browser, for faces that
// have a decision: the server keeps no crop of them (only of faces still
// waiting for a name and of people's pictures). Same square with room around
// the face as the server's crops (faces.rs, `crop`); the thumbnail is
// already there from the grid.
var ZOOM_MARGIN = 0.25;

function zoomFace(face) {
  var box = el('span', 'zoom');
  if (face.file == null) return box;
  var inner = el('span');
  if (face.roll) inner.style.transform = 'rotate(' + face.roll + 'deg)';
  var img = el('img');
  img.alt = '';
  img.decoding = 'async';
  img.onload = function () {
    var iw = img.naturalWidth, ih = img.naturalHeight;
    if (!iw || !ih) return;
    var side = Math.max(1, Math.min(Math.max(face.w * iw, face.h * ih) * (1 + 2 * ZOOM_MARGIN), iw, ih));
    var x0 = Math.min(Math.max((face.x + face.w / 2) * iw - side / 2, 0), Math.max(iw - side, 0));
    var y0 = Math.min(Math.max((face.y + face.h / 2) * ih - side / 2, 0), Math.max(ih - side, 0));
    img.style.width = (iw / side * 100) + '%';
    img.style.height = (ih / side * 100) + '%';
    img.style.left = (-x0 / side * 100) + '%';
    img.style.top = (-y0 / side * 100) + '%';
    box.classList.add('ready');
  };
  img.src = LIBAPI + '/files/' + face.file + '/thumb?v=' + face.version;
  inner.appendChild(img);
  box.appendChild(inner);
  return box;
}

function faceImg(face, cls) {
  var a = el('span', 'avatar' + (cls ? ' ' + cls : '') + (face.species ? ' pet pet-' + face.species : ''));
  if (face.species) a.title = face.species;
  // The pet's icon sits in a disc in the corner of the big faces (the name of a person says it elsewhere).
  if (face.species && cls === 'big') {
    var badge = el('span', 'pet-badge');
    badge.appendChild(speciesIcon(face.species));
    a.appendChild(badge);
  }
  var url = cropUrl(face);
  if (face.state === 'ignored' && face.file != null) {
    a.appendChild(zoomFace(face));
  } else if (url) {
    var img = el('img');
    img.alt = '';
    img.loading = 'lazy';
    img.src = url;
    a.appendChild(img);
  }
  return a;
}

function showPerson(id) { closeLightboxQuietly(); setFilter({ folder: null, tags: [], people: [id], q: '' }); }
// The photos of anyone in the group.
function showGroup(id) { closeLightboxQuietly(); setFilter({ folder: null, tags: [], people: [], groups: [id], q: '' }); }

function closeLightboxQuietly() { if (!$('lightbox').hidden) closeLightbox(); }

// ---- drag and drop of people onto groups (a mouse; "Move to group…" works everywhere)

var PERSON_TYPE = 'application/x-shoebox-person';

function dragPerson(node, p) {
  node.draggable = true;
  node.addEventListener('dragstart', function (ev) {
    ev.dataTransfer.setData(PERSON_TYPE, String(p.id));
    ev.dataTransfer.setData('text/plain', p.name);
    ev.dataTransfer.effectAllowed = 'move';
  });
}

function dropOnGroup(node, groupId) {
  var ok = function (ev) { return Array.prototype.indexOf.call(ev.dataTransfer.types || [], PERSON_TYPE) >= 0; };
  node.addEventListener('dragover', function (ev) { if (ok(ev)) { ev.preventDefault(); node.classList.add('drop'); } });
  node.addEventListener('dragleave', function () { node.classList.remove('drop'); });
  node.addEventListener('drop', function (ev) {
    node.classList.remove('drop');
    if (!ok(ev)) return;
    ev.preventDefault();
    ev.stopPropagation();
    var id = parseInt(ev.dataTransfer.getData(PERSON_TYPE), 10);
    var p = people.byId[id];
    if (p && p.group_id !== groupId) setGroup(p, groupId);
  });
}

function setGroup(p, groupId) {
  var mine = people.byId[p.id];
  var before = mine ? mine.group_id : undefined;
  return post(LIBAPI + '/people/' + p.id + '/group', { group_id: groupId }).then(function (r) {
    var g = people.groups.filter(function (x) { return x.id === groupId; })[0];
    toast(g ? tr('people.is_in', { name: p.name, group: g.name }) : tr('people.is_in_none', { name: p.name }));
    // The reply is the person with their new group: no need for the whole list.
    if (before === undefined || !takePerson(r)) return loadPeople().then(function () { if (state.filter.view === 'people') loadPeoplePage(); });
    countGroups(before, groupId);
    peopleShown();
    if (state.filter.view === 'people') loadPeoplePage();
  }).catch(failed);
}

// ---- sidebar

$('nav-people').onclick = function () { openSection('faces-section'); showView('people'); };

function renderFacesSection() {
  var info = people.info;
  $('faces-section').hidden = !(info && (info.faces.faces || info.clusters.people));
  var tree = $('people-tree');
  tree.textContent = '';
  groupedPeople().forEach(function (s) {
    var key = s.id == null ? 'none' : s.id;
    var li = el('li');
    var row = el('div', 'row');
    var toggle = el('button', 'toggle');
    if (s.people.length) setArrow(toggle, !!people.open[key]);
    var name = el('button', 'name', s.name);
    setTip(name, tr(s.people.length ? (s.id == null ? 'people.group_toggle' : 'people.group_show') : 'people.group_empty'));
    var flip = function () { people.open[key] = !people.open[key]; renderFacesSection(); };
    toggle.onclick = flip;
    if (s.id == null) name.onclick = flip;
    else {
      // A group shows the photos of all its people and unfolds; the arrow only folds and unfolds.
      name.dataset.group = s.id;
      name.onclick = function () { people.open[key] = true; showGroup(s.id); renderFacesSection(); };
    }
    row.appendChild(toggle);
    row.appendChild(name);
    row.appendChild(el('span', 'count', String(s.people.length)));
    li.appendChild(row);
    dropOnGroup(row, s.id);
    if (people.open[key] && s.people.length) {
      var ul = el('ul');
      s.people.forEach(function (p) {
        var pli = el('li');
        var prow = el('div', 'row');
        prow.appendChild(avatar(p, 'tiny'));
        var b = petLabel('button', 'name', p.species, p.name);
        b.dataset.person = p.id;
        b.onclick = function () { showPerson(p.id); };
        prow.appendChild(b);
        prow.appendChild(el('span', 'count', I18n.number(p.photos)));
        dragPerson(prow, p);
        pli.appendChild(prow);
        ul.appendChild(pli);
      });
      li.appendChild(ul);
    }
    tree.appendChild(li);
  });
  var c = info && info.clusters;
  var uli = el('li');
  var urow = el('div', 'row');
  urow.appendChild(el('span', 'toggle', ''));
  var ub = el('button', 'name', c ? tr('people.unnamed_n', { count: trn('count.clusters', c.clusters) }) : tr('people.unnamed'));
  ub.dataset.view = 'unnamed';
  ub.onclick = function () { showView('unnamed'); };
  urow.appendChild(ub);
  uli.appendChild(urow);
  tree.appendChild(uli);
  markFacesSection();
}

function markFacesSection() {
  var f = state.filter;
  $('nav-people').classList.toggle('active', f.view === 'people');
  Array.prototype.forEach.call(document.querySelectorAll('#people-tree .name'), function (b) {
    var on = b.dataset.view ? f.view === b.dataset.view
      : b.dataset.group ? !f.view && f.groups.length === 1 && !f.people.length && f.groups[0] === parseInt(b.dataset.group, 10)
      : b.dataset.person ? (!f.view && f.people.indexOf(parseInt(b.dataset.person, 10)) >= 0) || (f.view === 'person' && f.id === parseInt(b.dataset.person, 10))
      : false;
    b.classList.toggle('active', on);
  });
}

// ---- a field that names someone

// The species to name these faces: the one most of them have ('cat', 'dog';
// the first on a tie), 'pet' if all are drawn pets of no species, '' unless
// every one is a pet. A stray dog among six cats is a wrong find.
function facesSpecies(faces) {
  if (!faces.length || !faces.every(function (f) { return f.species; })) return '';
  var n = { cat: 0, dog: 0 };
  faces.forEach(function (f) { if (f.species in n) n[f.species]++; });
  if (!n.cat && !n.dog) return 'pet';
  return n.dog > n.cat ? 'dog' : 'cat';
}

// What to change the detected pets `faces` to when they are all one species
// the detector may have got wrong ('cat' or 'dog'): the other one, else ''.
function otherSpecies(faces) {
  var s = faces.length ? faces[0].species : '';
  if (s !== 'cat' && s !== 'dog') return '';
  if (!faces.every(function (f) { return f.id != null && f.species === s; })) return '';
  return s === 'cat' ? 'dog' : 'cat';
}

function makeSpeciesLabel(to) { return tr(to === 'dog' ? 'pet.make.dog' : 'pet.make.cat'); }

// Say a cat is a dog or the other way round: the faces keep their decisions
// and take the new species' names in the name proposals.
function setSpecies(ids, to) {
  return post(LIBAPI + '/faces/species', { faces: ids, species: to }).then(function (r) {
    toast(tr('pet.changed', { count: animalCount(to, r.faces) }));
    return r;
  });
}

// Whether a person (or pet) of `p.species` is one `species` asks for: ''
// people only, 'cat' or 'dog' those (and pets of no species), 'pet' any pet.
function kindOf(p, species) {
  if (!species) return !p.species;
  if (!p.species) return false;
  return species === 'pet' || p.species === 'pet' || p.species === species;
}

// A text field offering people by group while typing (the autocomplete
// that makes naming quick); picking one calls done({person_id} or {name},
// label). opts: allowNew (offer a new person of the typed name), except
// (ids to leave out), placeholder, species (a species, or a function giving
// it when the list is drawn: the new one is then a "cat", "dog" or "pet"),
// onEscape (Escape with the list closed; in a dialog it closes the dialog).
// For the keyboard it is a combobox: the arrows move through the list while
// the focus stays in the field, so Tab goes on to the dialog's buttons.
var pfieldSeq = 0;
function personField(done, opts) {
  opts = opts || {};
  var wrap = el('div', 'pfield');
  var input = el('input');
  input.type = 'text';
  input.placeholder = opts.placeholder || tr('people.name_placeholder');
  input.autocomplete = 'off';
  input.setAttribute('autocapitalize', 'words');
  input.setAttribute('enterkeyhint', 'done');
  var list = el('div', 'plist');
  list.id = 'plist-' + (++pfieldSeq);
  list.setAttribute('role', 'listbox');
  // Not a Tab stop either (a browser makes a scrolling box without one of its
  // own; it would take the focus and then fold away).
  list.tabIndex = -1;
  list.hidden = true;
  input.setAttribute('role', 'combobox');
  input.setAttribute('aria-autocomplete', 'list');
  input.setAttribute('aria-controls', list.id);
  input.setAttribute('aria-expanded', 'false');
  wrap.appendChild(input);
  wrap.appendChild(list);
  var items = [], at = -1;
  var setOpen = function (open) {
    list.hidden = !open;
    input.setAttribute('aria-expanded', open ? 'true' : 'false');
    if (!open) input.removeAttribute('aria-activedescendant');
  };
  // Only the kind of face being named is offered: people for faces, cats for
  // a cat, dogs for a dog.
  var kind = function () { return typeof opts.species === 'function' ? opts.species() : opts.species || ''; };
  var pick = function (it) {
    setOpen(false);
    input.value = it.label;
    done(it.who, it.label);
  };
  var mark = function () {
    Array.prototype.forEach.call(list.querySelectorAll('.item'), function (b, k) {
      b.classList.toggle('at', k === at);
      b.setAttribute('aria-selected', k === at ? 'true' : 'false');
      if (k === at) { input.setAttribute('aria-activedescendant', b.id); b.scrollIntoView({ block: 'nearest' }); }
    });
  };
  // An option of the list: a button for mouse and touch, out of the Tab order.
  var option = function (cls, text) {
    var b = el('button', cls, text);
    b.type = 'button';
    b.tabIndex = -1;
    b.id = list.id + '-' + items.length;
    b.setAttribute('role', 'option');
    b.setAttribute('aria-selected', 'false');
    b.onmousedown = function (ev) { ev.preventDefault(); };
    return b;
  };
  // A titled part of the list (a group of options for screen readers).
  var group = function (title) {
    var g = el('div');
    g.setAttribute('role', 'group');
    var h = el('div', 'head', title);
    h.id = list.id + '-g' + list.childNodes.length;
    g.setAttribute('aria-labelledby', h.id);
    g.appendChild(h);
    list.appendChild(g);
    return g;
  };
  var isTaken = function (text) {
    return people.list.some(function (p) { return fold(p.name) === fold(text) && !(opts.except && opts.except.indexOf(p.id) >= 0); });
  };
  var render = function () {
    var text = input.value.trim(), low = fold(text), exact = null;
    list.textContent = '';
    items = [];
    at = -1;
    input.removeAttribute('aria-activedescendant');
    var offered = function (p) {
      return kindOf(p, kind()) && (!low || fold(p.name).indexOf(low) >= 0) && !(opts.except && opts.except.indexOf(p.id) >= 0);
    };
    var addItem = function (into, p) {
      var it = { who: { person_id: p.id }, label: p.name };
      if (fold(p.name) === low) exact = it;
      var b = option('item');
      b.appendChild(avatar(p, 'tiny'));
      b.appendChild(el('span', '', p.name));
      b.onclick = function () { pick(it); };
      items.push(it);
      into.appendChild(b);
    };
    var section = function (title, ps) {
      if (!ps.length) return;
      var g = group(title);
      ps.forEach(function (p) { addItem(g, p); });
    };
    // First the people named last, then (field empty) the most photographed,
    // then everyone by group; nobody twice.
    var shown = {}, top = [], frequent = [];
    top = people.list.filter(function (p) { return p.last_named && offered(p); })
      .sort(function (a, b) { return b.last_named - a.last_named; }).slice(0, RECENT_MAX);
    top.forEach(function (p) { shown[p.id] = true; });
    if (!low) {
      frequent = people.list.filter(function (p) { return offered(p) && !shown[p.id] && p.photos > 0; })
        .sort(function (a, b) { return b.photos - a.photos || a.name.localeCompare(b.name); }).slice(0, RECENT_MAX);
      frequent.forEach(function (p) { shown[p.id] = true; });
    }
    section(tr('suggest.recent'), top);
    section(tr('suggest.most_photos'), frequent);
    groupedPeople().forEach(function (s) {
      section(s.name, s.people.filter(function (p) { return offered(p) && !shown[p.id]; }));
    });
    if (opts.allowNew && text && !exact && isTaken(text)) {
      // A name is one person's, whatever the kind: not offered again as new.
      var taken = el('div', 'head', tr('people.name_taken', { name: text }));
      taken.setAttribute('role', 'option');
      taken.setAttribute('aria-disabled', 'true');
      list.appendChild(taken);
    } else if (opts.allowNew && text && !exact) {
      var it = { who: { name: text }, label: text };
      var sp = kind();
      var b = option('item new', tr(sp ? 'people.new.' + (PET_SPECIES[sp] ? sp : 'pet') : 'people.new_person', { name: text }));
      b.onclick = function () { pick(it); };
      items.push(it);
      list.appendChild(b);
    }
    wrap.exact = exact;
    setOpen(list.childNodes.length > 0 && document.activeElement === input);
  };
  input.addEventListener('focus', render);
  input.addEventListener('input', render);
  // Not when the field has the focus again by then: picking Cat or Dog in the
  // "Add face" dialog takes it away and gives it straight back.
  input.addEventListener('blur', function () { setTimeout(function () { if (document.activeElement !== input) setOpen(false); }, 150); });
  input.addEventListener('keydown', function (ev) {
    ev.stopPropagation(); // arrows, Space and Escape belong to the field, not the viewer
    if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
      if (!items.length) return;
      ev.preventDefault();
      setOpen(true);
      at = ev.key === 'ArrowDown' ? (at + 1) % items.length : (at + items.length - 1) % items.length;
      mark();
    } else if (ev.key === 'Enter') {
      ev.preventDefault();
      var text = input.value.trim();
      if (at >= 0) pick(items[at]);
      else if (wrap.exact) pick(wrap.exact);
      else if (text && opts.allowNew && !isTaken(text)) pick({ who: { name: text }, label: text });
    } else if (ev.key === 'Escape') {
      if (!list.hidden) { setOpen(false); return; }
      if (opts.onEscape) opts.onEscape();
      else if ($('modal').contains(input)) closeModal(); // as Escape anywhere else in a dialog
    }
  });
  wrap.input = input;
  // What is typed, as {person_id} for someone listed or {name} for a new
  // person (null if empty or not allowed).
  wrap.value = function () {
    var text = input.value.trim();
    if (!text) return null;
    if (wrap.exact) return wrap.exact.who;
    var hit = people.list.filter(function (p) { return fold(p.name) === fold(text) && kindOf(p, kind()); })[0];
    if (hit) return { person_id: hit.id };
    if (isTaken(text)) return null;
    return opts.allowNew ? { name: text } : null;
  };
  return wrap;
}

// ---- dialogs for people and groups

function nameDialog(title, value, label, save) {
  var body = el('div');
  var input = el('input');
  input.type = 'text';
  input.className = 'wide';
  input.value = value || '';
  body.appendChild(input);
  var error = el('p', 'error');
  body.appendChild(error);
  var go = function (btn) {
    if (!input.value.trim()) { error.textContent = tr('people.type_name'); return false; }
    btn.disabled = true;
    save(input.value.trim()).then(function () { closeModal(); }).catch(function (e) { btn.disabled = false; error.textContent = e.message; });
    return false;
  };
  var buttons = openModal(title, body, [{ label: tr('app.cancel'), cls: 'quiet' }, { label: label, onclick: go }]);
  input.addEventListener('keydown', function (ev) { if (ev.key === 'Enter') go(buttons[1]); });
  input.select();
}

function renamePerson(p) {
  nameDialog(tr('people.rename'), p.name, tr('people.rename'), function (name) {
    return post(LIBAPI + '/people/' + p.id + '/rename', { name: name }).then(function (r) {
      toast(tr('people.renamed', { name: r.name }));
      state.personNames[p.id] = r.name;
      personUpdated(r);
      peopleInfoChanged();
      if (state.filter.view) loadView(state.filter.view); else renderChips();
    });
  });
}

// Only for someone without faces (a misspelled name); others are merged.
function deletePersonDialog(p) {
  openModal(tr('people.delete_title'), tr('people.delete_text', { name: p.name }), [
    { label: tr('app.cancel'), cls: 'quiet' },
    { label: tr('app.delete'), cls: 'danger', onclick: function () {
      post(LIBAPI + '/people/' + p.id + '/delete').then(function () {
        toast(tr('people.deleted', { name: p.name }));
        personRemoved(p.id);
        if (state.filter.people.indexOf(p.id) >= 0) setFilter({ people: state.filter.people.filter(function (x) { return x !== p.id; }) });
        else if (state.filter.view === 'person' && state.filter.id === p.id) showView('people');
        else if (state.filter.view) loadView(state.filter.view);
      }).catch(failed);
    } },
  ]);
}

// A new group always starts with a person (their page: "New group…"): the
// person is its first member, so there is never an empty group.
function newGroup(p) {
  nameDialog(tr('people.new_group', { name: p.name }), '', tr('people.create'), function (name) {
    return post(LIBAPI + '/groups', { name: name, person_id: p.id }).then(function (g) {
      toast(tr('people.group_created', { name: g.name }));
      // The reply is the new group (with its id), and the person is its first member.
      var mine = people.byId[p.id];
      if (!mine) return loadPeople().then(function () { if (state.filter.view === 'people') loadPeoplePage(); });
      countGroups(mine.group_id, null);
      people.groups.push(g);
      mine.group_id = g.id;
      peopleShown();
      if (state.filter.view === 'people') loadPeoplePage();
    });
  });
}

function moveToGroupDialog(p) {
  var body = el('div');
  body.appendChild(el('p', '', tr('people.put_into', { name: p.name })));
  var list = el('div', 'taglist');
  var choice = function (label, id, current) {
    var b = el('button', current ? 'current' : '', label + (current ? ' ✓' : ''));
    b.onclick = function () { closeModal(); if (!current) setGroup(p, id); };
    list.appendChild(b);
  };
  people.groups.forEach(function (g) { choice(g.name, g.id, p.group_id === g.id); });
  choice(tr('people.no_group'), null, p.group_id == null);
  body.appendChild(list);
  body.appendChild(el('p', 'hint', tr('people.group_hint')));
  openModal(tr('people.to_group_title'), body, [
    { label: tr('people.new_group_btn'), cls: 'quiet', onclick: function () { setTimeout(function () { newGroup(p); }, 0); } },
    { label: tr('app.cancel'), cls: 'quiet' },
  ]);
}

function mergeDialog(p) {
  var body = el('div');
  body.appendChild(el('p', '', tr('people.merge_text', { name: p.name })));
  var into = null;
  var field = personField(function (who) { into = who.person_id; }, { except: [p.id], species: p.species || '', placeholder: tr('people.merge_placeholder') });
  body.appendChild(field);
  body.appendChild(el('p', 'hint', tr('people.merge_hint', { name: p.name })));
  var error = el('p', 'error');
  body.appendChild(error);
  openModal(tr('people.merge_title'), body, [{ label: tr('app.cancel'), cls: 'quiet' }, {
    label: tr('people.merge_title'), onclick: function (btn) {
      var who = field.value();
      into = into || (who && who.person_id);
      if (!into) { error.textContent = tr('people.choose_someone'); return false; }
      btn.disabled = true;
      post(LIBAPI + '/people/' + p.id + '/merge', { into: into }).then(function (r) {
        closeModal();
        toast(tr('people.merged', { name: p.name, into: r.name }));
        peopleMerged(p.id, r);
        if (state.filter.people.indexOf(p.id) >= 0 || (state.filter.view === 'person' && state.filter.id === p.id)) showPerson(r.id);
        else if (state.filter.view) loadView(state.filter.view);
      }).catch(function (e) { btn.disabled = false; error.textContent = e.message; });
      return false;
    },
  }]);
}

// Rename, reorder and delete groups.
function groupsDialog() {
  var body = el('div');
  var list = el('div', 'grouplist');
  body.appendChild(list);
  var render = function () {
    list.textContent = '';
    if (!people.groups.length) list.appendChild(el('p', 'hint', tr('people.groups_none')));
    people.groups.forEach(function (g, k) {
      var row = el('div', 'grow-row');
      row.appendChild(el('span', 'gname', g.name));
      row.appendChild(el('span', 'gcount', trn('count.people', g.people)));
      var btn = function (label, title, run) {
        var b = el('button', 'btn quiet small', label);
        setTip(b, title);
        b.setAttribute('aria-label', title);
        b.onclick = run;
        row.appendChild(b);
      };
      // Put group k at index `to` and save the new order.
      var moveTo = function (to, refocus) {
        if (to === k || to < 0 || to >= people.groups.length) return;
        var ids = people.groups.map(function (x) { return x.id; });
        ids.splice(k, 1);
        ids.splice(to, 0, g.id);
        post(LIBAPI + '/groups/reorder', { ids: ids }).then(function (gs) {
          people.groups = gs; render(); peopleShown();
          if (refocus) { var h = list.querySelectorAll('.grip')[to]; if (h) h.focus(); }
        }).catch(failed);
      };
      var grip = el('span', 'grip', '\u283F');
      grip.tabIndex = 0;
      grip.setAttribute('role', 'button');
      setTip(grip, tr('people.drag_handle'));
      grip.setAttribute('aria-label', tr('people.drag_handle'));
      grip.onkeydown = function (e) {
        if (!e.altKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return;
        e.preventDefault();
        moveTo(k + (e.key === 'ArrowUp' ? -1 : 1), true);
      };
      // Pointer events cover mouse, pen and touch with one code path.
      grip.onpointerdown = function (e) {
        if (e.button) return;
        e.preventDefault();
        grip.setPointerCapture(e.pointerId);
        row.classList.add('dragging');
        var rows = Array.prototype.slice.call(list.children);
        var target = k;
        var mark = function (ev) {
          var hit = document.elementFromPoint(ev.clientX, ev.clientY);
          var r = hit && hit.closest ? hit.closest('.grow-row') : null;
          var i = rows.indexOf(r);
          if (i >= 0) target = i;
          rows.forEach(function (x, j) { x.classList.toggle('over', j === target && j !== k); });
        };
        grip.onpointermove = mark;
        var end = function (ev) {
          grip.onpointermove = grip.onpointerup = grip.onpointercancel = null;
          row.classList.remove('dragging');
          rows.forEach(function (x) { x.classList.remove('over'); });
          if (ev.type === 'pointerup') moveTo(target);
        };
        grip.onpointerup = end;
        grip.onpointercancel = end;
      };
      row.insertBefore(grip, row.firstChild);
      btn(tr('people.rename'), tr('people.rename'), function () {
        nameDialog(tr('people.rename_group'), g.name, tr('people.rename'), function (name) {
          return post(LIBAPI + '/groups/' + g.id + '/rename', { name: name }).then(function (r) {
            people.groups.forEach(function (x) { if (x.id === r.id) x.name = r.name; });
            peopleShown();
            // After the name dialog has closed (it closes when this is done).
            setTimeout(groupsDialog, 0);
            if (state.filter.view === 'people') loadPeoplePage();
          });
        });
      });
      btn(tr('app.delete'), tr('app.delete'), function () {
        openModal(tr('people.delete_group_title'), tr('people.delete_group_text', { name: g.name, detail: g.people ? tr('people.delete_group_some', { count: trn('count.people', g.people) }) : tr('people.delete_group_none') }), [
          { label: tr('app.cancel'), cls: 'quiet', onclick: function () { setTimeout(groupsDialog, 0); } },
          { label: tr('app.delete'), cls: 'danger', onclick: function () {
            post(LIBAPI + '/groups/' + g.id + '/delete').then(function () {
              toast(tr('people.group_deleted', { name: g.name }));
              // Its people have no group afterwards.
              people.groups = people.groups.filter(function (x) { return x.id !== g.id; });
              people.list.forEach(function (x) { if (x.group_id === g.id) x.group_id = null; });
              delete state.groupNames[g.id];
              peopleShown();
              groupsDialog();
              if (state.filter.view === 'people') loadPeoplePage();
            }).catch(failed);
          } },
        ]);
      });
      list.appendChild(row);
    });
  };
  render();
  openModal(tr('people.groups_title'), body, [
    { label: tr('app.done'), onclick: function () { if (state.filter.view === 'people') loadPeoplePage(); } },
  ]);
}

function personMenuItems(p) {
  return [
    { label: tr('people.menu.rename'), run: function () { renamePerson(p); } },
    { label: tr('people.menu.move'), run: function () { moveToGroupDialog(p); } },
    { label: tr('people.menu.new_group'), run: function () { newGroup(p); } },
    { label: tr('people.menu.merge'), run: function () { mergeDialog(p); } },
  ].concat(p.faces === 0 ? [{ label: tr('people.menu.delete'), run: function () { deletePersonDialog(p); } }] : []);
}

// A ⋯ button that opens a menu below itself (works by touch).
function menuButton(items, title) {
  var b = el('button', 'more-btn', '⋯');
  b.type = 'button';
  setTip(b, title || tr('people.more'));
  b.setAttribute('aria-label', title || tr('people.more'));
  b.onclick = function (ev) {
    ev.preventDefault();
    ev.stopPropagation();
    var r = b.getBoundingClientRect();
    showMenu(r.left, r.bottom + 2, typeof items === 'function' ? items() : items);
  };
  return b;
}

// Above the grid when it shows one person's photos.
function personHead(box, f) {
  if (f.view || f.people.length !== 1) return;
  var p = people.byId[f.people[0]];
  if (!p) return;
  var head = el('div', 'person-head');
  head.appendChild(avatar(p, 'big'));
  var text = el('div', 'ptext');
  text.appendChild(petLabel('div', 'pname', p.species, p.name));
  var g = people.groups.filter(function (x) { return x.id === p.group_id; })[0];
  text.appendChild(el('div', 'pmeta', trn('count.photos', p.photos) + ' · ' + (g ? g.name : tr('people.no_group_lower'))));
  head.appendChild(text);
  var review = p.suggested + p.maybe;
  var faces = el('button', 'btn quiet', review ? tr('people.check', { count: trn('count.faces', review) }) : tr('side.faces'));
  faces.onclick = function () { showView('person', p.id, p.suggested ? 'suggested' : p.maybe ? 'maybe' : 'confirmed'); };
  head.appendChild(faces);
  head.appendChild(menuButton(personMenuItems(p), tr('people.more_for', { name: p.name })));
  box.insertBefore(head, box.firstChild);
}

// ---- people overview

// Groups folded on the People page (kept while the page is in use, not saved).
var pageClosed = {};

function foldGroup(sec, grid, fold, open) {
  setArrow(fold, open);
  fold.setAttribute('aria-expanded', String(open));
  sec.classList.toggle('folded', !open);
  grid.hidden = !open;
}

function loadPeoplePage() {
  var page = $('page');
  if (state.filter.view !== 'people') return;
  clearNode(page);
  page.appendChild(el('h2', '', tr('side.faces')));
  var c = people.info && people.info.clusters;
  var sub = el('p', 'sub');
  sub.appendChild(document.createTextNode((c
    ? tr('people.overview_unnamed', { people: trn('count.people', people.list.length), faces: trn('count.faces', c.unnamed), clusters: trn('count.clusters', c.clusters) })
    : tr('people.overview', { people: trn('count.people', people.list.length) })) + ' '));
  if (c && c.clusters) {
    var name = el('button', 'link', tr('people.name_them'));
    name.onclick = function () { showView('unnamed'); };
    sub.appendChild(name);
  }
  page.appendChild(sub);
  var bar = el('div', 'toolbar');
  var gs = el('button', 'btn quiet', tr('people.groups_btn'));
  gs.onclick = groupsDialog;
  var check = el('button', 'btn quiet', tr('settings.calibration'));
  setTip(check, tr('people.calibration_hint'));
  check.onclick = function () { showView('settings'); };
  bar.appendChild(gs);
  bar.appendChild(check);
  page.appendChild(bar);
  var sections = groupedPeople();
  if (!people.list.length) page.appendChild(el('p', 'sub', tr('people.nobody_named')));
  sections.forEach(function (s) {
    var key = s.id == null ? 'none' : s.id;
    var sec = el('section', 'pgroup');
    var h = el('h3');
    var fold = el('button', 'toggle');
    fold.type = 'button';
    h.appendChild(fold);
    h.appendChild(el('span', 'gname', s.name));
    h.appendChild(el('span', 'n', String(s.people.length)));
    sec.appendChild(h);
    var grid = el('div', 'people');
    // The title folds the group's people away; all groups start open.
    var flipGroup = function () {
      pageClosed[key] = !pageClosed[key];
      foldGroup(sec, grid, fold, !pageClosed[key]);
    };
    foldGroup(sec, grid, fold, !pageClosed[key]);
    h.onclick = flipGroup;
    s.people.forEach(function (p) { grid.appendChild(personTile(p)); });
    if (!s.people.length) grid.appendChild(el('p', 'hint', tr('people.group_empty_hint')));
    sec.appendChild(grid);
    dropOnGroup(sec, s.id);
    page.appendChild(sec);
  });
  // "No group" as a drop target even when nobody is in it.
  if (people.groups.length && !sections.some(function (s) { return s.id == null; })) {
    var none = el('section', 'pgroup empty-drop');
    none.appendChild(el('h3', '', tr('people.no_group')));
    dropOnGroup(none, null);
    page.appendChild(none);
  }
}

function personTile(p) {
  var t = el('div', 'person');
  t.dataset.id = p.id;
  var a = el('a', 'ptile');
  a.href = '#';
  a.onclick = function (ev) { ev.preventDefault(); showPerson(p.id); };
  a.appendChild(avatar(p, 'big'));
  a.appendChild(petLabel('span', 'pname', p.species, p.name));
  a.appendChild(el('span', 'pmeta', trn('count.photos', p.photos)));
  t.appendChild(a);
  if (p.suggested + p.maybe) {
    var badge = el('button', 'pbadge', tr('people.to_check', { n: p.suggested + p.maybe }));
    setTip(badge, tr('people.suggested_for', { name: p.name }));
    badge.onclick = function () { showView('person', p.id, p.suggested ? 'suggested' : 'maybe'); };
    t.appendChild(badge);
  }
  t.appendChild(menuButton(personMenuItems(p), tr('people.more_for', { name: p.name })));
  dragPerson(t, p);
  return t;
}

// ---- a person's faces

var PERSON_TABS = [['confirmed', 'person.tab.confirmed'], ['suggested', 'person.tab.suggested'], ['maybe', 'person.tab.maybe'], ['rejected', 'person.tab.rejected']];
var pp = { person: null, tab: 'confirmed', faces: [], total: 0, picking: false, picked: {} };

function loadPersonPage() {
  var f = state.filter, page = $('page');
  var tab = PERSON_TABS.some(function (t) { return t[0] === f.tab; }) ? f.tab : 'confirmed';
  clearNode(page);
  pp.tab = tab;
  pp.faces = [];
  pp.picked = {};
  pp.picking = false;
  return api(LIBAPI + '/people/' + f.id).then(function (p) {
    if (state.filter.view !== 'person' || state.filter.id !== p.id) return;
    pp.person = p;
    state.personNames[p.id] = p.name;
    state.personSpecies[p.id] = p.species;
    var head = el('div', 'person-head');
    head.appendChild(avatar(p, 'big'));
    var text = el('div', 'ptext');
    text.appendChild(petLabel('h2', 'pname', p.species, p.name));
    text.appendChild(el('div', 'pmeta', tr('person.meta', { photos: trn('count.photos', p.photos), faces: p.species ? animalCount(p.species, p.faces) : trn('count.faces', p.faces) })));
    head.appendChild(text);
    var photos = el('button', 'btn quiet', tr('person.photos'));
    photos.onclick = function () { showPerson(p.id); };
    head.appendChild(photos);
    head.appendChild(menuButton(personMenuItems(p), tr('people.more_for', { name: p.name })));
    page.appendChild(head);
    var tabs = el('div', 'tabs');
    tabs.id = 'person-tabs';
    PERSON_TABS.forEach(function (t) {
      var b = el('button', 'tab' + (t[0] === tab ? ' active' : ''));
      b.dataset.tab = t[0];
      b.onclick = function () { setFilter({ view: 'person', id: p.id, tab: t[0] }); };
      tabs.appendChild(b);
    });
    page.appendChild(tabs);
    personTabCounts(p);
    page.appendChild(el('p', 'sub', tr('person.sub.' + tab, { name: p.name })));
    // Stays at the top while the faces scroll under it.
    var bar = el('div', 'toolbar sticky');
    var pick = el('button', 'btn quiet', tr('facecheck.select'));
    pick.id = 'pp-pick';
    pick.onclick = function () { pp.picking = !pp.picking; if (!pp.picking) clearPersonPicks(); updatePersonPick(); };
    bar.appendChild(pick);
    if (tab === 'suggested') {
      var all = el('button', 'btn quiet', tr('person.confirm_all'));
      all.onclick = function () { personAction('confirm', pp.faces.map(function (x) { return x.id; }).filter(function (x) { return x != null; })); };
      bar.appendChild(all);
    }
    page.appendChild(bar);
    var grid = el('div', 'faces has-drag');
    grid.id = 'pp-grid';
    grid.dragSite = function () {
      if (!pp.picking) return null;
      var cardOf = function (x) { return document.querySelector('#pp-grid .face[data-key="' + faceKey(x) + '"]'); };
      return {
        sel: '.face',
        itemOf: function (tile) { return tile.face && faceKey(tile.face) ? tile.face : undefined; },
        get: function (x) { return !!pp.picked[faceKey(x)]; },
        set: function (x, on) {
          if (on) pp.picked[faceKey(x)] = x; else delete pp.picked[faceKey(x)];
          var c = cardOf(x);
          if (c) c.classList.toggle('sel', on);
        },
        done: function (x) { if (x) pp.anchor = faceKey(x); updatePersonPick(); },
      };
    };
    page.appendChild(grid);
    var more = el('button', 'btn quiet', tr('app.show_more'));
    more.id = 'pp-more';
    more.hidden = true;
    more.onclick = function () { morePersonFaces(grid, more, tab); };
    page.appendChild(more);
    page.appendChild(personPickBar(tab));
    updatePersonPick();
    return morePersonFaces(grid, more, tab);
  }).catch(function (e) {
    if (e.status === 404) { page.appendChild(el('p', 'sub', tr('person.missing'))); return; }
    failed(e);
  });
}

function personTabCounts(p) {
  var counts = { confirmed: p.faces, suggested: p.suggested, maybe: p.maybe };
  Array.prototype.forEach.call(document.querySelectorAll('#person-tabs .tab'), function (b) {
    var t = PERSON_TABS.filter(function (x) { return x[0] === b.dataset.tab; })[0];
    b.textContent = tr(t[1]) + (counts[t[0]] != null ? ' (' + I18n.number(counts[t[0]]) + ')' : '');
  });
}

// The counts in the tabs after the suggestions were recomputed.
function loadPersonCounts() {
  var id = state.filter.id;
  api(LIBAPI + '/people/' + id).then(function (p) {
    if (state.filter.view === 'person' && state.filter.id === id) { pp.person = p; personTabCounts(p); }
  }).catch(function () {});
}

function morePersonFaces(grid, more, tab) {
  more.disabled = true;
  return api(LIBAPI + '/people/' + pp.person.id + '/faces' + query({ state: tab, offset: pp.faces.length, limit: 200 })).then(function (r) {
    if (state.filter.view !== 'person') return;
    pp.total = r.total;
    r.faces.forEach(function (face) { pp.faces.push(face); grid.appendChild(personFaceCard(face, tab)); });
    if (!pp.total) grid.appendChild(el('p', 'sub', tr('person.none')));
    more.disabled = false;
    more.hidden = pp.faces.length >= pp.total;
  }).catch(failed);
}

// The faces shown whose key `gone(key)` says left the tab's list.
function dropPersonFaces(gone) {
  pp.faces = pp.faces.filter(function (x) {
    var k = faceKey(x);
    if (!gone(k)) return true;
    var card = document.querySelector('#pp-grid .face[data-key="' + k + '"]');
    if (card) card.remove();
    delete pp.picked[k];
    return false;
  });
  updatePersonPick();
  personGridEmptied();
}

// Back from the viewer, where faces were decided: the faces shown that left
// this tab go (as after a decision here), the counts follow. A face that came
// into the tab shows with the next visit. The list is asked for up to what is
// shown and a page more, so a face that moved down stays; past the server's
// limit the page is built again.
function personFacesAgain() {
  var id = pp.person && pp.person.id, tab = pp.tab, n = pp.faces.length;
  if (state.filter.view !== 'person' || state.filter.id !== id) return;
  loadPersonCounts();
  if (!n) return;
  if (n + 200 > 2000) { loadPersonPage(); return; }
  api(LIBAPI + '/people/' + id + '/faces' + query({ state: tab, offset: 0, limit: n + 200 })).then(function (r) {
    if (state.filter.view !== 'person' || state.filter.id !== id || pp.tab !== tab) return;
    var still = {};
    r.faces.forEach(function (x) { still[faceKey(x)] = true; });
    pp.total = r.total;
    dropPersonFaces(function (k) { return k && !still[k]; });
    var more = $('pp-more');
    if (more && !more.disabled) more.hidden = pp.faces.length >= pp.total;
  }).catch(failed);
}

// The last faces shown left the list (e.g. "Confirm all shown"): the next
// ones come without a click on "Show more", or the line that there are none.
function personGridEmptied() {
  var grid = $('pp-grid'), more = $('pp-more');
  if (pp.faces.length || !grid || !more || more.disabled) return;
  morePersonFaces(grid, more, pp.tab);
}

function faceKey(face) { return face.id != null ? 'f' + face.id : face.manual != null ? 'm' + face.manual : null; }

function personFaceCard(face, tab) {
  var p = pp.person;
  var card = el('div', 'face' + (face.small ? ' small' : ''));
  var key = faceKey(face);
  card.dataset.key = key;
  card.face = face;
  var a = el('a', 'crop');
  a.href = '#';
  setTip(a, tr('facecheck.open_photo'));
  var url = cropUrl(face);
  if (tab === 'confirmed' && face.file != null) {
    // Confirmed faces have no stored crop: cut out of the photo's thumbnail.
    a.appendChild(zoomFace(face));
  } else if (url) {
    var img = el('img');
    img.alt = '';
    img.loading = 'lazy';
    img.src = url;
    img.onerror = function () { card.classList.add('broken'); };
    a.appendChild(img);
  } else {
    card.classList.add('broken');
  }
  a.onclick = function (ev) {
    ev.preventDefault();
    if (pp.picking) {
      if (!key) return;
      var span = ev.shiftKey && pickSpan(pp.faces, faceKey, pp.anchor, key);
      if (span) {
        span.forEach(function (x) {
          var k = faceKey(x);
          if (!k) return;
          pp.picked[k] = x;
          var c = document.querySelector('#pp-grid .face[data-key="' + k + '"]');
          if (c) c.classList.add('sel');
        });
      } else {
        if (pp.picked[key]) delete pp.picked[key]; else pp.picked[key] = face;
        card.classList.toggle('sel', !!pp.picked[key]);
        pp.anchor = key;
      }
      updatePersonPick();
      return;
    }
    // Suggested and maybe: the arrow keys step through the faces to check.
    if (face.file != null) openFacePhoto(face, tab === 'suggested' || tab === 'maybe' ? pp.faces : null, function (changed) { if (changed) personFacesAgain(); });
  };
  card.appendChild(a);
  var caption = face.similarity != null ? tr('person.alike', { pct: Math.round(face.similarity * 100) }) : Math.round(face.px) + ' px';
  if (face.kept) caption = tr('person.kept');
  else if (face.manual != null) caption = tr('face.drawn');
  card.appendChild(el('div', 'meta', caption));
  if (face.small) card.appendChild(el('span', 'tag', tr('facecheck.tag_small')));
  var acts = el('div', 'acts');
  var act = function (label, title, cls, run) {
    var b = el('button', cls, label);
    setTip(b, title);
    b.setAttribute('aria-label', title);
    b.onclick = function (ev) { ev.stopPropagation(); run(); };
    acts.appendChild(b);
  };
  if (tab === 'suggested' || tab === 'maybe') {
    act('✓', tr('person.it_is', { name: p.name }), 'yes', function () { personAction('confirm', [face.id]); });
    act('✗', tr('person.not', { name: p.name }), 'no', function () { personAction('reject', [face.id]); });
  } else if (tab === 'rejected') {
    act('↺', tr('person.undo_not', { name: p.name }), 'undo', function () { personAction('unreject', [face.id]); });
  } else if (!face.kept) {
    card.appendChild(menuButton(function () { return confirmedFaceMenu(face); }, tr('person.more_face')));
  }
  card.appendChild(acts);
  return card;
}

function confirmedFaceMenu(face) {
  var p = pp.person, items = [];
  if (face.manual != null) {
    items.push({ label: tr('person.name_else'), run: function () { nameFacesDialog([face], personRefresh); } });
    items.push({ label: tr('person.delete_drawn'), run: function () { personAction('undo', [], [face.manual]); } });
    return items;
  }
  items.push({ label: tr('person.not', { name: p.name }), run: function () { personAction('reject', [face.id]); } });
  items.push({ label: tr('person.name_else'), run: function () { nameFacesDialog([face], personRefresh); } });
  items.push({ label: tr('menu.use_as', { name: p.name }), run: function () {
    post(LIBAPI + '/people/' + p.id + '/cover', { face: face.id }).then(function (r) { toast(tr('menu.picture_changed')); personUpdated(r); }).catch(failed);
  } });
  items.push({ label: tr(p.species ? 'pet.not_a.pet' : 'facecheck.not_face'), run: function () { personAction('not-face', [face.id]); } });
  return items;
}

function personRefresh() { if (state.filter.view === 'person') loadPersonPage(); }

// Confirm, reject, undo a rejection, not a face, forget, for this person's
// page; the faces leave the list they were in.
function personAction(action, ids, manual) {
  var p = pp.person;
  ids = (ids || []).filter(function (x) { return x != null; });
  manual = manual || [];
  if (!ids.length && !manual.length) return;
  var body = { faces: ids, manual: manual };
  if (action === 'reject' || action === 'unreject') body.person_id = p.id;
  post(LIBAPI + '/faces/' + action, body).then(function (r) {
    var gone = {};
    ids.forEach(function (x) { gone['f' + x] = true; });
    manual.forEach(function (x) { gone['m' + x] = true; });
    dropPersonFaces(function (k) { return gone[k]; });
    var count = trn('count.faces', r.faces);
    toast({
      confirm: tr('person.toast.confirm', { count: count, name: p.name }),
      reject: tr('person.toast.reject', { count: count, name: p.name }),
      unreject: tr('person.toast.unreject', { count: count, name: p.name }),
      'not-face': tr(p.species ? 'facecheck.marked_pet' : 'facecheck.marked', { count: count }),
      ignore: tr('unnamed.ignored', { count: count }),
      undo: tr('person.toast.undo'),
    }[action]);
    peopleChanged();
    loadPersonCounts();
  }).catch(failed);
}

// The bar of the timeline's selection: close, count (or what to do), Select
// all, then the tools. Confirm and Undo are icons with a quick label, the other
// actions keep their words (they carry a name).
function personPickBar(tab) {
  var bar = el('div', 'selbar tools');
  bar.id = 'pp-bar';
  bar.setAttribute('role', 'toolbar');
  bar.setAttribute('aria-label', tr('sel.bar'));
  bar.hidden = true;
  var done = el('button', 'ib');
  done.innerHTML = SEL_ICONS.close;
  setTip(done, tr('app.done'));
  done.setAttribute('aria-label', tr('app.done'));
  done.onclick = function () { pp.picking = false; clearPersonPicks(); updatePersonPick(); };
  bar.appendChild(done);
  var count = el('span', 'sel-count');
  count.id = 'pp-count';
  count.setAttribute('aria-live', 'polite');
  bar.appendChild(count);
  var p = pp.person;
  var all = el('button', 'sel-all', tr('sel.all'));
  all.id = 'pp-all';
  all.onclick = function () {
    var on = Object.keys(pp.picked).length < pp.faces.filter(faceKey).length;
    pp.faces.forEach(function (face) {
      var k = faceKey(face);
      if (!k) return;
      if (on) pp.picked[k] = face; else delete pp.picked[k];
      var card = document.querySelector('#pp-grid .face[data-key="' + k + '"]');
      if (card) card.classList.toggle('sel', on);
    });
    updatePersonPick();
  };
  bar.appendChild(all);
  var sep = el('span', 'sep break');
  sep.setAttribute('aria-hidden', 'true');
  bar.appendChild(sep);
  var chosen = function (run) {
    return function () {
      var faces = Object.keys(pp.picked).map(function (k) { return pp.picked[k]; });
      run(faces, faces.map(function (x) { return x.id; }).filter(function (x) { return x != null; }));
    };
  };
  var iconTool = function (svg, label, cls, run) {
    var b = el('button', 'ib pp-act' + (cls ? ' ' + cls : ''));
    b.innerHTML = svg;
    setTip(b, label);
    b.setAttribute('aria-label', label);
    b.onclick = chosen(run);
    bar.appendChild(b);
  };
  var button = function (label, cls, run) {
    var b = el('button', 'btn pp-act ' + (cls || ''), label);
    b.onclick = chosen(run);
    bar.appendChild(b);
  };
  if (tab === 'suggested' || tab === 'maybe') {
    iconTool(SEL_ICONS.check, tr('person.confirm'), 'ok', function (faces, ids) { personAction('confirm', ids); });
    button(tr('person.not', { name: p.name }), 'quiet', function (faces, ids) { personAction('reject', ids); });
  } else if (tab === 'rejected') {
    iconTool(SEL_ICONS.undo, tr('person.undo'), '', function (faces, ids) { personAction('unreject', ids); });
  } else {
    button(tr('person.not', { name: p.name }), 'quiet', function (faces, ids) { personAction('reject', ids); });
    button(tr('person.name_btn'), '', function (faces) { nameFacesDialog(faces, personRefresh); });
    // Someone the user does not want to keep: ignored, so not asked about again.
    button(tr('unnamed.ignore'), 'quiet', function (faces, ids) { personAction('ignore', ids); });
    button(tr(p.species ? 'pet.not_a.pet' : 'facecheck.not_face'), 'quiet', function (faces, ids) { personAction('not-face', ids); });
  }
  return bar;
}

function clearPersonPicks() {
  pp.anchor = null;
  pp.picked = {};
  Array.prototype.forEach.call(document.querySelectorAll('#pp-grid .face.sel'), function (c) { c.classList.remove('sel'); });
}

function updatePersonPick() {
  var bar = $('pp-bar');
  if (!bar) return;
  var n = Object.keys(pp.picked).length;
  bar.hidden = !pp.picking;
  $('pp-pick').classList.toggle('active', pp.picking);
  $('pp-count').textContent = n ? trn('count.faces', n) : tr(window.matchMedia('(pointer: fine)').matches ? 'person.hint_mouse' : 'facecheck.tap');
  $('pp-count').classList.toggle('hint', !n);
  var total = pp.faces.filter(faceKey).length;
  $('pp-all').textContent = tr(total && n >= total ? 'sel.none' : 'sel.all');
  $('pp-all').disabled = !total;
  Array.prototype.forEach.call(bar.querySelectorAll('.pp-act'), function (b) {
    // The icon tools are dimmed, not disabled, like the timeline's.
    if (b.classList.contains('ib')) b.setAttribute('aria-disabled', n ? 'false' : 'true'); else b.disabled = !n;
  });
}

// Name several faces at once (detected or drawn ones): everyone listed, or
// a new person.
function nameFacesDialog(faces, done) {
  var body = el('div');
  var kind = facesSpecies(faces);
  // "Which cat is this?" for a cat, "dog" for a dog, "pet" for any other: the
  // same question for one face or many, since they are all one animal.
  var question = !kind ? (faces.length === 1 ? tr('person.who_one') : tr('person.who_many', { n: faces.length }))
    : kind === 'cat' ? tr('person.who_cat') : kind === 'dog' ? tr('person.who_dog') : tr('person.who_pet');
  body.appendChild(el('p', '', question));
  // One request at a time (the dialog stays open until the reply).
  var saving = false, buttons = [];
  var save = function (who) {
    if (saving) return;
    saving = true;
    buttons.forEach(function (b) { b.disabled = true; });
    field.input.disabled = true;
    var ids = faces.map(function (x) { return x.id; }).filter(function (x) { return x != null; });
    var drawn = faces.filter(function (x) { return x.id == null && x.manual != null; });
    var req = ids.length ? post(LIBAPI + '/faces/assign', Object.assign({ faces: ids }, who)) : Promise.resolve({ faces: 0 });
    req.then(function (r) {
      // A drawn face nothing was detected at: drawn again with the new name.
      return Promise.all(drawn.map(function (d) {
        return post(LIBAPI + '/faces/undo', { manual: [d.manual] }).then(function () {
          return post(LIBAPI + '/faces/manual', Object.assign({ file: d.file, box: [d.x, d.y, d.w, d.h], pet: d.species ? true : undefined }, who));
        });
      })).then(function () { return r; });
    }).then(function (r) {
      closeModal();
      toast(tr('person.named', { count: kind ? animalCount(kind, faces.length) : trn('count.faces', faces.length), name: r.person ? r.person.name : who.name || state.personNames[who.person_id] || '' }));
      peopleChanged();
      if (done) done();
    }).catch(failed);
  };
  var field = personField(function (who) { save(who); }, { allowNew: true, placeholder: tr('people.name_placeholder'), species: facesSpecies(faces) });
  body.appendChild(field);
  buttons = openModal(tr('person.name'), body, [{ label: tr('app.cancel'), cls: 'quiet' }, {
    label: tr('person.name'), onclick: function () {
      var who = field.value();
      if (!who) return false;
      save(who);
      return false;
    },
  }]);
  field.input.focus();
}

// ---- unnamed clusters

// box, more, singles: the page's parts, for `refreshUnnamed`; epoch counts
// the actions and loads started, so a refresh that crossed one is redone.
var un = { shown: 0, total: 0, unnamed: 0, kind: 'all', box: null, more: null, singles: null, epoch: 0, timer: null };
var UN_KINDS = [['all', 'unnamed.kind.all'], ['faces', 'unnamed.kind.faces'], ['pets', 'unnamed.kind.pets']];
// Which kind the page lists is a browser setting, kept over a reload.
var UN_KIND_KEY = 'shoebox-unnamed-kind';
try {
  var savedKind = localStorage.getItem(UN_KIND_KEY);
  if (UN_KINDS.some(function (o) { return o[0] === savedKind; })) un.kind = savedKind;
} catch (e) { /* private window */ }
var PAGE_CLUSTERS = 30;

// Size of the face circles: a browser setting for the whole application, like
// the maps switch (nothing is kept on the drive or in the library).
var FACE_SIZE = { min: 64, max: 150, step: 2, key: 'shoebox-face-size' };

function faceSize() {
  var n = 64;
  try { n = parseInt(localStorage.getItem(FACE_SIZE.key), 10); } catch (e) { /* private window */ }
  return isNaN(n) ? FACE_SIZE.min : Math.max(FACE_SIZE.min, Math.min(FACE_SIZE.max, n));
}

function faceSizeControl(page) {
  var size = faceSize();
  page.style.setProperty('--face-size', size + 'px');
  var label = el('label', 'face-size');
  setTip(label, tr('unnamed.size'));
  label.appendChild(el('span', 'dot s'));
  var input = el('input');
  input.type = 'range';
  input.min = FACE_SIZE.min;
  input.max = FACE_SIZE.max;
  input.step = FACE_SIZE.step;
  input.value = size;
  input.setAttribute('aria-label', tr('unnamed.size'));
  label.appendChild(input);
  label.appendChild(el('span', 'dot l'));
  var out = el('output', '', size + ' px');
  label.appendChild(out);
  input.oninput = function () {
    page.style.setProperty('--face-size', input.value + 'px');
    out.textContent = input.value + ' px';
    try { localStorage.setItem(FACE_SIZE.key, input.value); } catch (e) { /* ignore */ }
  };
  return label;
}

function loadUnnamed() {
  var page = $('page');
  clearNode(page);
  page.appendChild(regroupElements());
  page.appendChild(el('h2', '', tr('people.unnamed')));
  var sub = el('p', 'sub', tr('dups.looking'));
  sub.id = 'un-sub';
  page.appendChild(sub);
  // Pets are named like people, but listed apart when asked.
  var kinds = el('select');
  kinds.id = 'un-kind';
  kinds.setAttribute('aria-label', tr('unnamed.which'));
  UN_KINDS.forEach(function (o) {
    var opt = el('option', '', tr(o[1]));
    opt.value = o[0];
    opt.selected = o[0] === un.kind;
    kinds.appendChild(opt);
  });
  kinds.onchange = function () {
    un.kind = kinds.value;
    try { localStorage.setItem(UN_KIND_KEY, un.kind); } catch (e) { /* ignore */ }
    loadUnnamed();
  };
  var bar = el('div', 'toolbar');
  bar.appendChild(kinds);
  bar.appendChild(faceSizeControl(page));
  page.appendChild(bar);
  var box = el('div', 'clusters');
  box.id = 'un-box';
  page.appendChild(box);
  var more = el('button', 'btn quiet', tr('app.show_more'));
  more.hidden = true;
  more.onclick = function () { moreClusters(box, more); };
  page.appendChild(more);
  un.shown = 0;
  var singles = singlesSection();
  page.appendChild(singles.section);
  page.appendChild(singles.bar);
  un.box = box;
  un.more = more;
  un.singles = singles;
  regroup.said = '';
  drawRegroup();
  return Promise.all([moreClusters(box, more), singles.load()]);
}

// When the next regrouping brings new suggestions (the server's rules,
// `/api/info` `clusters`: `pending`, `cards_left`, `wait_ms` and `pause_ms`
// while decisions wait; `stale` or `running` with `done` of `total` while it
// runs). A line at the top right of the Unnamed page, level with the title,
// that stays at the top in a box while the cards scroll. Its circle empties
// over the pause and fills again with each decision; while the regrouping runs
// it fills with the progress. The seconds are counted down here from
// `wait_ms`; the server tells of every change (`facesStream`). Screen readers
// hear the line only when the number of cards changes and when the regrouping
// starts and ends (`#un-live`), not every second.
var regroup = { c: null, at: 0, timer: null, said: '', floatOn: false };

function regroupElements() {
  var frag = document.createDocumentFragment();
  var line = el('div', 'regroup');
  line.id = 'un-next';
  line.hidden = true;
  var tick = el('span', 'tick');
  tick.setAttribute('aria-hidden', 'true');
  line.appendChild(tick);
  line.appendChild(el('span', 'text'));
  frag.appendChild(line);
  var live = el('span', 'sr-only');
  live.id = 'un-live';
  live.setAttribute('aria-live', 'polite');
  frag.appendChild(live);
  if (!regroup.floatOn) {
    regroup.floatOn = true;
    $('scroller').addEventListener('scroll', regroupFloat, { passive: true });
  }
  return frag;
}

// In a box once the page has scrolled under it.
function regroupFloat() {
  var line = $('un-next');
  if (line) line.classList.toggle('floating', $('scroller').scrollTop > 8);
}

function regroupLine(info) {
  regroup.c = info.clusters;
  regroup.at = Date.now();
  drawRegroup();
}

function reducedMotion() {
  return !!(window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches);
}

// The circle at `deg`, reached in `ms` (0: at once, also when it fills up
// again after a decision).
function setTick(tick, deg, ms) {
  tick.style.transitionDuration = ms + 'ms';
  if (!ms) tick.classList.add('jump');
  tick.style.setProperty('--regroup-deg', deg.toFixed(1) + 'deg');
  if (!ms) {
    void tick.offsetWidth;
    tick.classList.remove('jump');
  }
}

function drawRegroup() {
  clearTimeout(regroup.timer);
  regroup.timer = null;
  var line = $('un-next'), live = $('un-live');
  if (!line || state.filter.view !== 'unnamed') return;
  var c = regroup.c;
  var mode = !c ? null : c.running || c.stale ? 'running' : c.pending ? 'waiting' : null;
  var tick = line.firstChild, text = line.lastChild, say = regroup.said;
  line.hidden = !mode;
  if (mode === 'running') {
    var part = c.running && c.total ? Math.min(1, c.done / c.total) : 0;
    setTick(tick, 360 * part, line.dataset.mode === 'running' && !reducedMotion() ? 1000 : 0);
    text.textContent = tr('unnamed.regrouping');
    say = tr('unnamed.regrouping');
  } else if (mode === 'waiting') {
    var left = Math.max(0, c.wait_ms - (Date.now() - regroup.at));
    var pause = Math.max(c.pause_ms || 0, left, 1);
    var secs = Math.ceil(left / 1000);
    text.textContent = c.cards_left > 0 ? trn('unnamed.next', c.cards_left, { s: secs }) : tr('unnamed.next_soon', { s: secs });
    // Smoothly to where it will be at the next second (with reduced motion
    // it steps once a second); filled up again at once.
    var next = left % 1000 || 1000;
    var smooth = !reducedMotion() && left > 0;
    var deg = 360 * Math.max(0, left - (smooth ? next : 0)) / pause;
    var before = parseFloat(tick.style.getPropertyValue('--regroup-deg')) || 0;
    var refill = line.dataset.mode !== 'waiting' || 360 * left / pause > before + 1;
    if (refill && smooth) setTick(tick, 360 * left / pause, 0);
    setTick(tick, deg, smooth ? next : 0);
    if (refill && !smooth) setTick(tick, deg, 0);
    say = c.cards_left > 0 ? trn('unnamed.next_cards', c.cards_left) : '';
    if (left > 0) regroup.timer = setTimeout(drawRegroup, next);
  } else if (line.dataset.mode === 'running') {
    say = tr('unnamed.regrouped_done');
  }
  line.dataset.mode = mode || '';
  if (live && say !== regroup.said) {
    regroup.said = say;
    live.textContent = say;
  }
}

// Leaving the Unnamed page: decisions that wait are regrouped soon (the
// server decides; nothing if none wait).
function leftUnnamed() {
  clearTimeout(regroup.timer);
  regroup.timer = null;
  var c = people.info && people.info.clusters;
  if (!c || !c.pending) return;
  post(LIBAPI + '/clusters/left').then(loadInfo).catch(function () {});
}

// Bring the Unnamed page up to date in place: the cards and single faces shown
// so far are asked for again; a card whose faces are the same (same
// generation) stays as it is (what is typed, what is selected, opened) and only
// takes the new suggestion; cards and faces that are gone leave, new ones come
// in where they belong. Waits while an action is under way.
function scheduleUnnamedRefresh(delay) {
  clearTimeout(un.timer);
  un.timer = setTimeout(refreshUnnamed, delay == null ? 50 : delay);
}

function refreshUnnamed() {
  un.timer = null;
  if (state.filter.view !== 'unnamed' || !un.box || !document.body.contains(un.box)) return;
  if (document.querySelector('#page .cluster.busy, #page .singles-bar.busy')) { scheduleUnnamedRefresh(500); return; }
  var epoch = un.epoch, box = un.box, more = un.more, singles = un.singles;
  var kind = un.kind === 'all' ? null : un.kind;
  Promise.all([
    api(LIBAPI + '/clusters' + query({ size: 'large', offset: 0, limit: Math.max(un.shown, PAGE_CLUSTERS), samples: 8, kind: kind })),
    api(LIBAPI + '/clusters' + query({ size: 'small', offset: 0, limit: Math.max(singles.shown(), SINGLES_PAGE), samples: 2, kind: kind })),
  ]).then(function (r) {
    if (state.filter.view !== 'unnamed' || box !== un.box) return;
    if (epoch !== un.epoch) { scheduleUnnamedRefresh(); return; }
    var large = r[0];
    var cards = {};
    Array.prototype.forEach.call(box.children, function (card) { if (card.cluster) cards[card.cluster.generation] = card; });
    var keep = large.clusters.map(function (c) {
      var card = cards[c.generation];
      if (card) { delete cards[c.generation]; card.sync(c); } else card = clusterCard(c);
      return card;
    });
    Object.keys(cards).forEach(function (g) { cards[g].remove(); });
    keep.forEach(function (card, k) { if (box.children[k] !== card) box.insertBefore(card, box.children[k] || null); });
    un.total = large.total;
    un.unnamed = large.unnamed;
    un.shown = large.clusters.length;
    more.hidden = un.shown >= un.total;
    unnamedSummary();
    singles.sync(r[1]);
  }).catch(function () { /* the next change or status check tries again */ });
}

// Clusters of one or two faces are not worth a card each: their faces are
// listed together as thumbnails, picked one by one (tap, Shift-tap for a
// range) and named, ignored or marked "not a face" from the bar below.
var SINGLES_PAGE = 60;

function singlesSection() {
  var st = { faces: [], picked: {}, anchor: null, shown: 0, total: 0, count: 0, tiles: {} };
  var section = el('section', 'singles has-drag');
  section.dragSite = function () {
    return {
      sel: '.cface',
      itemOf: function (tile) { return tile.face; },
      get: function (x) { return !!st.picked[x.id]; },
      set: function (x, on) {
        if (on) st.picked[x.id] = true; else delete st.picked[x.id];
        if (st.tiles[x.id]) st.tiles[x.id].classList.toggle('sel', on);
      },
      done: function (x) { if (x) st.anchor = x.id; update(); },
    };
  };
  section.hidden = true;
  var head = el('h3');
  var hint = el('p', 'sub', tr('unnamed.singles_hint'));
  var grid = el('div', 'sface-grid');
  var more = el('button', 'btn quiet', tr('app.show_more'));
  more.hidden = true;
  section.appendChild(head);
  section.appendChild(hint);
  section.appendChild(grid);
  section.appendChild(more);

  var bar = el('div', 'selbar singles-bar');
  bar.hidden = true;
  var count = el('span');
  bar.appendChild(count);
  var field = personField(function () { name(); }, { allowNew: true, placeholder: tr('unnamed.who'), species: function () { return facesSpecies(picked()); } });
  bar.appendChild(field);
  var nameBtn = actionButton(NAME_ICON, 'name');
  nameBtn.classList.add('primary');
  var ignore = actionButton(IGNORE_ICON, 'ignore');
  var notFace = actionButton(NOT_FACE_ICON, 'not-face');
  var change = actionButton(CAT_ICON, 'change');
  var done = actionButton(lineIcon('<path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/>'), 'clear');
  setAction(done, tr('unnamed.clear'), tr('unnamed.select_done_hint'));
  [nameBtn, ignore, notFace, change, done].forEach(function (b) { bar.appendChild(b); });

  var picked = function () { return st.faces.filter(function (f) { return st.picked[f.id]; }); };
  var update = function () {
    var chosen = picked(), n = chosen.length;
    var pets = n > 0 && chosen.every(function (f) { return f.species; });
    head.textContent = tr('unnamed.singles', { count: trn('count.faces', st.count) });
    section.hidden = !st.faces.length && st.shown >= st.total;
    count.textContent = trn('count.faces', n);
    bar.hidden = !n;
    setAction(nameBtn, tr('unnamed.name_n', { n: n }), tr('unnamed.name_selected_hint'));
    setAction(ignore, tr('unnamed.ignore_n', { n: n }), tr('unnamed.ignore_hint'));
    notFace.innerHTML = pets ? NOT_PET_ICON : NOT_FACE_ICON;
    setAction(notFace, tr(pets ? 'unnamed.notface_n_pet' : 'unnamed.notface_n', { n: n }), tr(pets ? 'unnamed.notface_hint_pet' : 'unnamed.notface_hint'));
    var to = otherSpecies(chosen);
    change.hidden = !to;
    if (to) {
      change.innerHTML = to === 'cat' ? CAT_ICON : DOG_ICON;
      setAction(change, makeSpeciesLabel(to), '');
    }
    more.hidden = st.shown >= st.total;
  };
  var tile = function (face) {
    var t = el('a', 'cface' + (st.picked[face.id] ? ' sel' : '') + (face.small ? ' small' : ''));
    t.href = '#';
    setTip(t, tr('facecheck.select'));
    t.appendChild(faceImg(face, 'big'));
    var open = el('span', 'copen', '↗');
    setTip(open, tr('facecheck.open_photo'));
    open.onclick = function (ev) { ev.preventDefault(); ev.stopPropagation(); openFacePhoto(face, st.faces); };
    t.appendChild(open);
    // Keyboard: Tab between thumbnails (or ←/→), Enter selects (the click
    // below), Space opens the photo and, in the viewer, closes it again.
    t.onkeydown = function (ev) {
      // While the viewer is open the thumbnail keeps the focus (to come back to);
      // its keys belong to the viewer then, so Space can close it.
      if (!$('lightbox').hidden) return;
      if (ev.altKey || ev.metaKey || ev.ctrlKey) return;
      if (ev.key === ' ') {
        ev.preventDefault();
        ev.stopPropagation(); // the viewer's own Space (close) must not see this one
        openFacePhoto(face, st.faces);
      } else if (ev.key === 'ArrowLeft' || ev.key === 'ArrowRight') {
        var next = ev.key === 'ArrowLeft' ? t.previousElementSibling : t.nextElementSibling;
        if (next) { ev.preventDefault(); next.focus(); }
      }
    };
    t.onclick = function (ev) {
      ev.preventDefault();
      var span = ev.shiftKey && pickSpan(st.faces, function (x) { return x.id; }, st.anchor, face.id);
      if (span) {
        span.forEach(function (x) { st.picked[x.id] = true; if (st.tiles[x.id]) st.tiles[x.id].classList.add('sel'); });
      } else {
        if (st.picked[face.id]) delete st.picked[face.id]; else st.picked[face.id] = true;
        t.classList.toggle('sel', !!st.picked[face.id]);
        st.anchor = face.id;
      }
      update();
    };
    t.face = face;
    st.tiles[face.id] = t;
    return t;
  };
  var load = function () {
    more.disabled = true;
    un.epoch++;
    return api(LIBAPI + '/clusters' + query({ size: 'small', offset: st.shown, limit: SINGLES_PAGE, samples: 2, kind: un.kind === 'all' ? null : un.kind })).then(function (r) {
      if (state.filter.view !== 'unnamed') return;
      st.total = r.total;
      st.count = r.unnamed;
      st.shown += r.clusters.length;
      r.clusters.forEach(function (c) {
        c.faces.forEach(function (face) { st.faces.push(face); grid.appendChild(tile(face)); });
      });
      more.disabled = false;
      update();
    }).catch(failed);
  };
  more.onclick = load;

  var act = function (action, who) {
    var chosen = picked();
    if (!chosen.length) return;
    var ids = chosen.map(function (f) { return f.id; });
    var pets = chosen.every(function (f) { return f.species; });
    bar.classList.add('busy');
    un.epoch++;
    // open: refused (409) if one of them was decided elsewhere in the meantime.
    var request = action === 'assign' ? post(LIBAPI + '/faces/assign', Object.assign({ faces: ids, open: true }, who))
      : post(LIBAPI + '/faces/' + action, { faces: ids, open: true });
    request.then(function (r) {
      bar.classList.remove('busy');
      var n = pets ? animalCount(facesSpecies(chosen), ids.length) : trn('count.faces', ids.length);
      toast(action === 'assign' ? tr('person.named', { count: n, name: r.person ? r.person.name : who.name || '' })
        : action === 'ignore' ? tr('unnamed.ignored', { count: n }) : tr(pets ? 'facecheck.marked_pet' : 'facecheck.marked', { count: n }));
      var gone = {};
      ids.forEach(function (id) { gone[id] = true; if (st.tiles[id]) st.tiles[id].remove(); delete st.tiles[id]; });
      st.faces = st.faces.filter(function (f) { return !gone[f.id]; });
      st.picked = {};
      st.anchor = null;
      st.count -= ids.length;
      field.input.value = '';
      update();
      peopleChanged(r, true);
    }).catch(function (e) {
      bar.classList.remove('busy');
      if (e.status === 409) toast(tr('unnamed.regrouped')); else failed(e);
      refreshUnnamed();
    });
  };
  change.onclick = function () {
    var chosen = picked(), to = otherSpecies(chosen);
    if (!to) return;
    bar.classList.add('busy');
    setSpecies(chosen.map(function (f) { return f.id; }), to).then(function () {
      bar.classList.remove('busy');
      peopleChanged();
      loadUnnamed();
    }).catch(function (e) { bar.classList.remove('busy'); failed(e); });
  };
  var name = function () {
    var who = field.value();
    if (!who) { field.input.focus(); return; }
    act('assign', who);
  };
  onAction(nameBtn, name);
  onAction(ignore, function () { act('ignore'); });
  onAction(notFace, function () { act('not-face'); });
  onAction(done, function () {
    Object.keys(st.picked).forEach(function (id) { if (st.tiles[id]) st.tiles[id].classList.remove('sel'); });
    st.picked = {};
    st.anchor = null;
    update();
  });
  // The faces as the server lists them now (`refreshUnnamed`): tiles of faces
  // that are gone leave (and their ticks), new ones come in, the others stay.
  var sync = function (r) {
    var list = [];
    r.clusters.forEach(function (c) { c.faces.forEach(function (face) { list.push(face); }); });
    var now = {};
    list.forEach(function (face) { now[face.id] = true; });
    Object.keys(st.picked).forEach(function (id) { if (!now[id]) delete st.picked[id]; });
    if (st.anchor != null && !now[st.anchor]) st.anchor = null;
    var tiles = {};
    list.forEach(function (face) { tiles[face.id] = st.tiles[face.id] || tile(face); });
    Object.keys(st.tiles).forEach(function (id) { if (!tiles[id]) st.tiles[id].remove(); });
    st.tiles = tiles;
    list.forEach(function (face, k) { if (grid.children[k] !== tiles[face.id]) grid.insertBefore(tiles[face.id], grid.children[k] || null); });
    st.faces = list;
    st.total = r.total;
    st.count = r.unnamed;
    st.shown = r.clusters.length;
    update();
  };
  return { section: section, bar: bar, load: load, sync: sync, shown: function () { return st.shown; } };
}

function unnamedSummary() {
  var sub = $('un-sub');
  if (!sub) return;
  var pets = un.kind === 'pets';
  sub.textContent = un.total
    ? tr(pets ? 'unnamed.summary_pets' : 'unnamed.summary', { faces: trn(pets ? 'count.pets' : 'count.faces', un.unnamed), clusters: trn('count.clusters', un.total) })
    : tr(pets ? 'unnamed.done_pets' : 'unnamed.done');
}

function moreClusters(box, more) {
  more.disabled = true;
  un.epoch++;
  return api(LIBAPI + '/clusters' + query({ size: 'large', offset: un.shown, limit: PAGE_CLUSTERS, samples: 8, kind: un.kind === 'all' ? null : un.kind })).then(function (r) {
    if (state.filter.view !== 'unnamed') return;
    un.total = r.total;
    un.unnamed = r.unnamed;
    unnamedSummary();
    r.clusters.forEach(function (c) { box.appendChild(clusterCard(c)); });
    un.shown += r.clusters.length;
    more.disabled = false;
    more.hidden = un.shown >= un.total;
  }).catch(failed);
}

function clusterCard(c) {
  var card = el('div', 'cluster has-drag');
  card.dragSite = function () {
    if (!card.picking) return null;
    return {
      sel: '.cface',
      itemOf: function (tile) { return tile.face; },
      get: function (x) { return !!card.picked[x.id]; },
      set: function (x, on) {
        if (on) card.picked[x.id] = true; else delete card.picked[x.id];
        if (tiles[x.id]) tiles[x.id].classList.toggle('sel', on);
      },
      done: function (x) { if (x) card.anchor = x.id; update(); },
    };
  };
  card.cluster = c;
  card.picked = {};
  card.picking = false;
  var head = el('div', 'chead');
  var count = el('span', 'csize');
  head.appendChild(count);
  // Open a card (all faces, or Select) and it takes the whole width; this
  // button stays at the top of the screen while the faces scroll by.
  var close = el('button', 'btn small cclose', tr('unnamed.close'));
  setTip(close, tr('unnamed.close_hint'));
  close.hidden = true;
  head.appendChild(close);
  card.appendChild(head);
  var grid = el('div', 'cfaces');
  card.appendChild(grid);
  var faces = c.faces.slice();
  var expanded = false;
  // Face id → its tile, kept while the face is in the card: drawn again (a
  // refresh, Select) the card reuses them, with their pictures.
  var tiles = {};
  var tile = function (face) {
    var f = tiles[face.id];
    if (!f) {
      f = tiles[face.id] = el('a');
      f.href = '#';
      f.appendChild(faceImg(face, 'big'));
      f.onclick = function (ev) {
        ev.preventDefault();
        var face = f.face;
        if (card.picking) {
          var span = ev.shiftKey && pickSpan(faces, function (x) { return x.id; }, card.anchor, face.id);
          if (span) {
            span.forEach(function (x) { card.picked[x.id] = true; if (tiles[x.id]) tiles[x.id].classList.add('sel'); });
          } else {
            if (card.picked[face.id]) delete card.picked[face.id]; else card.picked[face.id] = true;
            f.classList.toggle('sel', !!card.picked[face.id]);
            card.anchor = face.id;
          }
          update();
          return;
        }
        openFacePhoto(face, faces);
      };
      // Same keys as the single faces: Space opens the photo (and closes the
      // viewer again), ←/→ move between thumbnails.
      f.onkeydown = function (ev) {
        if (!$('lightbox').hidden) return;
        if (ev.altKey || ev.metaKey || ev.ctrlKey) return;
        if (ev.key === ' ') {
          ev.preventDefault();
          ev.stopPropagation();
          f.click();
        } else if (ev.key === 'ArrowLeft' || ev.key === 'ArrowRight') {
          var next = ev.key === 'ArrowLeft' ? f.previousElementSibling : f.nextElementSibling;
          if (next && next.classList.contains('cface')) { ev.preventDefault(); next.focus(); }
        }
      };
    }
    f.face = face;
    f.className = 'cface' + (card.picked[face.id] ? ' sel' : '') + (face.small ? ' small' : '');
    setTip(f, tr(card.picking ? 'facecheck.select' : 'facecheck.open_photo'));
    return f;
  };
  // All faces of a big group can be thousands: `growGrid` puts them on the
  // page as they are scrolled to.
  var renderFaces = function () {
    var now = {};
    faces.forEach(function (face) { now[face.id] = true; });
    Object.keys(tiles).forEach(function (id) { if (!now[id]) delete tiles[id]; });
    var all = null;
    if (c.size > faces.length) {
      all = el('button', 'cmore', '+' + I18n.number(c.size - faces.length));
      setTip(all, tr('unnamed.show_all', { n: c.size }));
      all.onclick = function () {
        api(LIBAPI + '/clusters/' + c.id + '/faces' + query({ generation: c.generation })).then(function (list) {
          faces = list;
          expanded = true;
          renderFaces();
          update();
        }).catch(function (e) { stale(e); });
      };
    }
    growGrid(grid, faces, tile, all);
  };
  var suggestion = el('div', 'csugg');
  card.appendChild(suggestion);
  // One toolbar, fixed to the bottom of the card: it stays in view while a long
  // card scrolls and rests at the card's bottom edge at the end.
  var row = el('div', 'crow cbar');
  var field = personField(function (who) { act('name', who); }, { allowNew: true, placeholder: tr('unnamed.who'), species: facesSpecies(c.faces) });
  row.appendChild(field);
  var nameBtn = actionButton(NAME_ICON, 'name');
  nameBtn.classList.add('primary');
  onAction(nameBtn, function () {
    var who = field.value();
    if (!who) { field.input.focus(); return; }
    act('name', who);
  });
  row.appendChild(nameBtn);
  var tools = row;
  var ignore = actionButton(IGNORE_ICON, 'ignore');
  onAction(ignore, function () { act('ignore', {}); });
  // A card of pets says "not a pet": the finds that are no pet at all.
  var petCard = c.faces.length > 0 && c.faces.every(function (f) { return f.species; });
  var notFace = actionButton(petCard ? NOT_PET_ICON : NOT_FACE_ICON, 'not-face');
  onAction(notFace, function () { act('not-face', {}); });
  var pick = actionButton(SELECT_ICON, 'pick');
  // Select needs all the faces to choose from.
  var loadAll = function () {
    if (faces.length >= c.size) return Promise.resolve();
    return api(LIBAPI + '/clusters/' + c.id + '/faces' + query({ generation: c.generation })).then(function (list) {
      faces = list;
    });
  };
  onAction(pick, function () {
    card.picking = !card.picking;
    if (!card.picking) { card.picked = {}; card.anchor = null; }
    else expanded = true;
    loadAll().then(function () {
      renderFaces();
      update();
    }).catch(function (e) { stale(e); });
  });
  close.onclick = function () {
    card.picking = false;
    card.picked = {};
    expanded = false;
    faces = faces.slice(0, 8);
    renderFaces();
    update();
    card.scrollIntoView({ block: 'nearest' });
  };
  var change = actionButton(CAT_ICON, 'change');
  onAction(change, function () {
    var chosen = faces.filter(function (f) { return card.picked[f.id]; }), to = otherSpecies(chosen);
    if (!to) return;
    card.classList.add('busy');
    setSpecies(chosen.map(function (f) { return f.id; }), to).then(function () {
      card.classList.remove('busy');
      peopleChanged();
      loadUnnamed();
    }).catch(function (e) { card.classList.remove('busy'); stale(e); });
  });
  tools.appendChild(ignore);
  tools.appendChild(notFace);
  tools.appendChild(change);
  tools.appendChild(pick);
  card.appendChild(row);

  var picked = function () { return Object.keys(card.picked).map(Number); };
  var update = function () {
    var n = picked().length;
    var pets = c.faces.length > 0 && c.faces.every(function (f) { return f.species; });
    count.textContent = trn(pets ? 'count.pets' : 'count.faces', c.size);
    if (card.picking) {
      count.appendChild(document.createTextNode(' · '));
      if (n) count.appendChild(el('b', '', tr('unnamed.selected', { n: n })));
      else count.appendChild(document.createTextNode(tr('unnamed.tap')));
    }
    pick.classList.toggle('active', card.picking);
    setAction(pick, tr(card.picking ? 'unnamed.select_done' : 'facecheck.select'), tr(card.picking ? 'unnamed.select_done_hint' : 'unnamed.select_hint'));
    card.classList.toggle('picking', card.picking);
    card.classList.toggle('open', expanded || card.picking);
    close.hidden = !(expanded || card.picking);
    var some = card.picking && n;
    var off = card.picking && !n;
    setAction(nameBtn, some ? tr('unnamed.name_n', { n: n }) : tr('person.name'), tr(some ? 'unnamed.name_selected_hint' : 'unnamed.name_hint'), off);
    setAction(ignore, some ? tr('unnamed.ignore_n', { n: n }) : tr('unnamed.ignore'), tr('unnamed.ignore_hint'), off);
    setAction(notFace, some ? tr(petCard ? 'unnamed.notface_n_pet' : 'unnamed.notface_n', { n: n }) : tr(petCard ? 'pet.not_a.pet' : 'facecheck.not_face'),
      tr(petCard ? 'unnamed.notface_hint_pet' : 'unnamed.notface_hint'), off);
    var to = card.picking ? otherSpecies(faces.filter(function (f) { return card.picked[f.id]; })) : '';
    change.hidden = !to;
    if (to) {
      change.innerHTML = to === 'cat' ? CAT_ICON : DOG_ICON;
      setAction(change, makeSpeciesLabel(to), '');
    }
    suggestion.textContent = '';
    if (c.suggestion && !card.picking) {
      var s = c.suggestion;
      suggestion.appendChild(el('span', '', tr('unnamed.looks_like') + ' '));
      suggestion.appendChild(el('b', '', s.person.name));
      suggestion.appendChild(el('span', '', ' ' + tr('unnamed.of', { faces: s.faces, size: c.size }) + ' '));
      var yes = el('button', 'btn small', '✓ ' + s.person.name);
      setTip(yes, tr('unnamed.name_all', { size: c.size, name: s.person.name }));
      yes.onclick = function () { act('name', { person_id: s.person.id }); };
      suggestion.appendChild(yes);
      if (s.faces < c.size) {
        // Only the faces that really look like them; the rest stays.
        var only = el('button', 'btn small', tr('unnamed.only', { n: s.faces }));
        setTip(only, tr('unnamed.only_hint', { n: s.faces, name: s.person.name, rest: c.size - s.faces }));
        only.onclick = function () {
          loadAll().then(function () {
            var ids = faces.filter(function (x) { return x.state === 'suggested' && x.person && x.person.id === s.person.id; })
              .map(function (x) { return x.id; });
            if (!ids.length) return;
            act('name', { person_id: s.person.id }, ids);
          }).catch(function (e) { stale(e); });
        };
        suggestion.appendChild(only);
      }
    }
    suggestion.hidden = !suggestion.firstChild;
  };
  var stale = function (e) {
    if (e.status === 409) {
      toast(tr('unnamed.regrouped'));
      refreshUnnamed();
    } else failed(e);
  };
  // The same faces, listed again (`refreshUnnamed`): a new number and
  // suggestion; what is typed and selected stays.
  card.sync = function (n) {
    c.id = n.id;
    c.size = n.size;
    c.suggestion = n.suggestion;
    if (expanded || card.picking) {
      api(LIBAPI + '/clusters/' + c.id + '/faces' + query({ generation: c.generation })).then(function (list) {
        faces = list;
        renderFaces();
        update();
      }).catch(function () {});
    } else {
      faces = n.faces.slice();
      renderFaces();
    }
    update();
  };
  // Name, ignore or "not a face": the whole card, or the faces selected in
  // it. The card's generation makes sure only the faces shown are meant.
  var act = function (action, who, only) {
    var body = Object.assign({ generation: c.generation }, who);
    var some = only || (card.picking ? picked() : null);
    if (some) {
      if (!some.length) return;
      body.faces = some;
    }
    card.classList.add('busy');
    un.epoch++;
    post(LIBAPI + '/clusters/' + c.id + '/' + action, body).then(function (r) {
      card.classList.remove('busy');
      var n = petCard ? animalCount(facesSpecies(c.faces), r.faces) : trn('count.faces', r.faces);
      toast(action === 'name' ? tr('person.named', { count: n, name: r.person.name })
        : action === 'ignore' ? tr('unnamed.ignored', { count: n }) : tr(petCard ? 'facecheck.marked_pet' : 'facecheck.marked', { count: n }));
      un.unnamed -= r.faces;
      if (!r.cluster) {
        card.remove();
        un.total -= 1;
        un.shown -= 1;
      } else {
        c.id = r.cluster.id;
        c.generation = r.cluster.generation;
        c.size = r.cluster.size;
        faces = faces.filter(function (x) { return !some || some.indexOf(x.id) < 0; });
        card.picked = {};
        card.picking = false;
        field.input.value = '';
        renderFaces();
        update();
      }
      unnamedSummary();
      peopleChanged(r, true);
    }).catch(function (e) { card.classList.remove('busy'); stale(e); });
  };
  renderFaces();
  update();
  return card;
}

// ---- info panel: the people on a photo (5c-3)

// The faces of the photo with who they are: a name (confirmed), a
// suggestion with ✓/✗, "+ Name" for unnamed ones, and a ⋯ menu (not a face,
// ignore, …). Pointing at one (or tapping its crop) highlights its box.
function infoFaces(row, info) {
  var box = el('div', 'pfaces');
  var faces = info.faces || [];
  // "Select" turns the list into a checklist (picking mode) with a bar to
  // ignore or mark several faces at once. The pick belongs to one photo.
  if (!lb.pick || lb.pick.file !== info.id) lb.pick = { file: info.id, on: false, ids: {} };
  var pick = lb.pick;
  var pickable = faces.filter(function (f) { return f.id != null; });
  if (!pickable.length) pick.on = false;
  box.classList.toggle('picking', pick.on);
  if (!info.faces) box.appendChild(el('div', 'note', tr('info.faces_unlooked')));
  else if (!faces.length) box.appendChild(el('div', 'note', tr(info.kind === 'video' ? 'info.faces_video' : 'info.no_faces')));
  pick.list = pickable;
  faces.forEach(function (f, k) { box.appendChild(infoFace(info, f, k, pick)); });
  if (pick.on) {
    box.appendChild(infoPickBar(info, pickable, pick));
    infoPickDrag(box, pickable, pick);
  }
  var tools = el('div', 'ptools');
  // A video's boxes are only a formality: nothing is drawn on it.
  if (faces.length && info.kind !== 'video') {
    var show = toolButton(FRAME_ICON, tr(lb.showFaces ? 'info.hide_boxes' : 'info.show_boxes'));
    show.classList.toggle('on', !!lb.showFaces);
    show.onclick = function () { lb.showFaces = !lb.showFaces; renderPanel(); };
    tools.appendChild(show);
  }
  // Only a stack of faces nobody has confirmed yet is worth ticking together;
  // one confirmed face is dealt with on its own (its ⋯ menu).
  var unconfirmed = pickable.filter(function (f) { return f.state !== 'confirmed' && f.state !== 'ignored'; });
  if (unconfirmed.length > 1 && !pick.on) {
    var select = toolButton(SELECT_ICON, tr('info.select') + ': ' + tr('info.select_hint'));
    select.onclick = function () { pick.on = true; pick.ids = {}; pick.anchor = null; renderPanel(); };
    tools.appendChild(select);
  }
  if (lb.details && state.data) {
    var isVideo = state.data.kinds[state.open] === 'v';
    var add = toolButton(ADD_FACE_ICON, tr('info.add_face') + ': ' + tr(isVideo ? 'info.add_person_hint' : 'info.add_face_hint'));
    add.onclick = startDrawing;
    if (lb.details.view_turn) {
      add.disabled = true;
      setTip(add, tr('info.add_face_turned'));
      add.setAttribute('aria-label', add.getAttribute('data-tip'));
    }
    tools.appendChild(add);
  }
  box.appendChild(tools);
  row(tr('info.people'), box);
}

// Click and drag over the rows of the list in picking mode (mouse or pen; touch
// keeps scrolling): every row the pointer has covered since the press, and only
// those, takes the opposite of the state of the row pressed; rows it leaves
// again get back what they had. The panel is drawn again when the button goes up.
function infoPickDrag(box, pickable, pick) {
  box.addEventListener('pointerdown', function (ev) {
    if (ev.button !== 0 || ev.shiftKey || ev.pointerType === 'touch') return;
    var start = ev.target.closest('.pface.pickable');
    if (!start || ev.target.closest('.pname')) return;
    var d = { y: ev.clientY, on: !pick.ids[start.dataset.face], before: {}, moved: false, last: null };
    var rows = Array.prototype.slice.call(box.querySelectorAll('.pface.pickable'));
    var move = function (e) {
      if (!box.isConnected) { end(); return; }
      if (!d.moved && Math.abs(e.clientY - d.y) < DRAG_START) return;
      d.moved = true;
      var y1 = Math.min(d.y, e.clientY), y2 = Math.max(d.y, e.clientY);
      rows.forEach(function (row) {
        var id = row.dataset.face, r = row.getBoundingClientRect();
        var covered = r.bottom >= y1 && r.top <= y2;
        if (!(id in d.before)) d.before[id] = !!pick.ids[id];
        var on = covered ? d.on : d.before[id];
        if (on) pick.ids[id] = true; else delete pick.ids[id];
        row.classList.toggle('picked', on);
        row.querySelector('input').checked = on;
        if (covered) d.last = id;
      });
    };
    var end = function () {
      document.removeEventListener('pointermove', move);
      document.removeEventListener('pointerup', end);
      document.removeEventListener('pointercancel', end);
      if (!d.moved) return;
      // The click that follows the button going up selects nothing more.
      pick.swallow = true;
      setTimeout(function () { pick.swallow = false; }, 0);
      if (d.last != null) pick.anchor = Number(d.last);
      if (box.isConnected) renderPanel();
    };
    document.addEventListener('pointermove', move);
    document.addEventListener('pointerup', end);
    document.addEventListener('pointercancel', end);
  });
  // Keeps Shift-click and dragging from selecting the page's text.
  box.addEventListener('mousedown', function (ev) {
    if (ev.target.closest('.pface.pickable')) ev.preventDefault();
  });
}

// The bar under the list in picking mode: how many are ticked, a shortcut to
// tick every face nobody has named, and Ignore / Not a face for the ticked.
function infoPickBar(info, pickable, pick) {
  var bar = el('div', 'pickbar');
  var chosen = pickable.filter(function (f) { return pick.ids[f.id]; });
  var n = chosen.length;
  var pets = n > 0 && chosen.every(function (f) { return f.species; });
  var head = el('div', 'pickhead');
  head.appendChild(el('span', 'pickcount', n ? trn('count.faces', n) : tr('info.select_none')));
  var unnamed = pickable.filter(function (f) { return f.state !== 'confirmed' && f.state !== 'ignored'; });
  if (unnamed.length) {
    var all = el('button', 'link', tr('info.select_unnamed'));
    all.onclick = function () { unnamed.forEach(function (f) { pick.ids[f.id] = true; }); renderPanel(); };
    head.appendChild(all);
  }
  bar.appendChild(head);
  var row = el('div', 'pickacts');
  var ignore = actionButton(IGNORE_ICON, 'ignore');
  setAction(ignore, n ? tr('unnamed.ignore_n', { n: n }) : tr('unnamed.ignore'), tr('unnamed.ignore_hint'), !n);
  var notFace = actionButton(pets ? NOT_PET_ICON : NOT_FACE_ICON, 'not-face');
  setAction(notFace, n ? tr(pets ? 'unnamed.notface_n_pet' : 'unnamed.notface_n', { n: n }) : tr('facecheck.not_face'),
    tr(pets ? 'unnamed.notface_hint_pet' : 'unnamed.notface_hint'), !n);
  var cancel = el('button', 'btn quiet', tr('app.cancel'));
  var act = function (action) {
    var ids = chosen.map(function (f) { return f.id; });
    bar.classList.add('busy');
    post(LIBAPI + '/faces/' + action, { faces: ids }).then(function () {
      var count = trn('count.faces', ids.length);
      toast(action === 'ignore' ? tr('unnamed.ignored', { count: count }) : tr(pets ? 'facecheck.marked_pet' : 'facecheck.marked', { count: count }));
      pick.on = false;
      pick.ids = {};
      infoFacesChanged(info.id);
    }).catch(function (e) { bar.classList.remove('busy'); failed(e); });
  };
  onAction(ignore, function () { act('ignore'); });
  onAction(notFace, function () { act('not-face'); });
  cancel.onclick = function () { pick.on = false; pick.ids = {}; renderPanel(); };
  [ignore, notFace].forEach(function (b) { row.appendChild(b); });
  var to = otherSpecies(chosen);
  if (to) {
    var change = actionButton(to === 'cat' ? CAT_ICON : DOG_ICON, 'change');
    setAction(change, makeSpeciesLabel(to), '');
    onAction(change, function () {
      bar.classList.add('busy');
      setSpecies(chosen.map(function (f) { return f.id; }), to).then(function () {
        pick.on = false;
        pick.ids = {};
        infoFacesChanged(info.id);
      }).catch(function (e) { bar.classList.remove('busy'); failed(e); });
    });
    row.appendChild(change);
  }
  row.appendChild(cancel);
  bar.appendChild(row);
  return bar;
}

function infoFace(info, f, k, pick) {
  var line = el('div', 'pface');
  if (pick && pick.on) {
    // Picking mode: the whole row is a checkbox; its own buttons are hidden.
    if (f.id == null) {
      line.classList.add('nopick');
    } else {
      line.classList.add('pickable');
      if (pick.ids[f.id]) line.classList.add('picked');
      line.dataset.face = f.id;
      var tick = el('input');
      tick.type = 'checkbox';
      tick.checked = !!pick.ids[f.id];
      tick.setAttribute('aria-label', tr('facecheck.select'));
      // One click handler for the row and its box: Shift ticks everything
      // from the last one clicked to this one, as in the other face lists.
      line.onclick = function (ev) {
        if (ev.target.closest('.pname')) return;
        ev.preventDefault();
        if (pick.swallow) return;
        var span = ev.shiftKey && pickSpan(pick.list, function (x) { return x.id; }, pick.anchor, f.id);
        if (span) span.forEach(function (x) { pick.ids[x.id] = true; });
        else if (pick.ids[f.id]) delete pick.ids[f.id]; else pick.ids[f.id] = true;
        if (!span) pick.anchor = f.id;
        renderPanel();
      };
      line.appendChild(tick);
    }
  }
  // A named person shows their picture (no crop of the face is kept); the
  // box on the photo shows which face it is.
  var pic = f.state === 'confirmed' && f.person ? avatar(people.byId[f.person.id] || { name: f.person.name }, 'small') : faceImg(f, 'small');
  setTip(pic, tr('info.show_where'));
  pic.onclick = function () { lb.hover = lb.hover === k ? null : k; drawFaces(); };
  line.appendChild(pic);
  line.addEventListener('mouseenter', function () { lb.hover = k; drawFaces(); });
  line.addEventListener('mouseleave', function () { if (lb.hover === k) { lb.hover = null; drawFaces(); } });
  var who = el('span', 'who');
  line.appendChild(who);
  var changed = function () { infoFacesChanged(info.id); };
  var send = function (action, body) { return post(LIBAPI + '/faces/' + action, body).then(changed).catch(failed); };
  var ids = f.id != null ? [f.id] : [];
  if (f.state === 'confirmed') {
    var name = el('button', 'pname', f.person.name);
    setTip(name, tr('info.photos_of', { name: f.person.name }));
    name.onclick = function () { showPerson(f.person.id); };
    who.appendChild(name);
    if (f.kept) who.appendChild(el('span', 'note', ' ' + tr('info.face_kept')));
  } else if (f.state === 'suggested' || f.state === 'maybe') {
    who.appendChild(el('span', 'guess', tr(f.state === 'maybe' ? 'info.guess_maybe' : 'info.guess', { name: f.person.name })));
    var yes = el('button', 'yes', '✓');
    setTip(yes, tr('person.it_is', { name: f.person.name }));
    yes.setAttribute('aria-label', yes.getAttribute('data-tip'));
    yes.onclick = function () { send('confirm', { faces: ids }); };
    var no = el('button', 'no', '✗');
    setTip(no, tr('person.not', { name: f.person.name }));
    no.setAttribute('aria-label', no.getAttribute('data-tip'));
    no.onclick = function () { send('reject', { faces: ids, person_id: f.person.id }); };
    who.appendChild(yes);
    who.appendChild(no);
  } else if (f.state === 'ignored') {
    who.appendChild(el('span', 'note', tr(f.species ? 'info.stranger_pet' : 'info.stranger')));
  }
  if (f.state !== 'confirmed') {
    var add = el('button', 'add', tr(f.state === 'suggested' || f.state === 'maybe' ? 'info.other_person' : 'info.plus_name'));
    add.onclick = function () {
      var field = personField(function (person) {
        send('assign', Object.assign({ faces: ids }, person));
      }, { allowNew: true, placeholder: tr('unnamed.who'), species: f.species, onEscape: function () { field.replaceWith(add); } });
      add.replaceWith(field);
      field.input.focus();
    };
    who.appendChild(add);
  }
  if (f.species) who.appendChild(petLabel('span', 'note', f.species, speciesName(f.species)));
  if (f.small) who.appendChild(el('span', 'note', ' ' + tr('facecheck.tag_small')));
  if (f.kept) return line;
  line.appendChild(menuButton(function () {
    var items = [];
    if (f.manual != null) {
      items.push({ label: tr('person.name_else'), run: function () { nameFacesDialog([Object.assign({ file: info.id }, f)], changed); } });
      items.push({ label: tr('person.delete_drawn'), run: function () { send('undo', { manual: [f.manual] }); } });
      return items;
    }
    if (f.state === 'confirmed') {
      items.push({ label: tr('person.not', { name: f.person.name }), run: function () { send('reject', { faces: ids, person_id: f.person.id }); } });
      items.push({ label: tr('person.name_else'), run: function () { nameFacesDialog([f], changed); } });
      items.push({ label: tr('menu.use_as', { name: f.person.name }), run: function () {
        post(LIBAPI + '/people/' + f.person.id + '/cover', { face: f.id }).then(function (r) { toast(tr('menu.picture_changed')); personUpdated(r); }).catch(failed);
      } });
    }
    if (f.state !== 'ignored') items.push({ label: tr(f.species ? 'info.ignore_stranger_pet' : 'info.ignore_stranger'), run: function () { send('ignore', { faces: ids }); } });
    if (f.state) items.push({ label: tr('info.forget'), run: function () { send('undo', { faces: ids }); } });
    if (f.rejected && f.rejected.length) {
      f.rejected.forEach(function (pid) {
        var n = state.personNames[pid] || tr('info.someone');
        items.push({ label: tr('info.maybe_after_all', { name: n }), run: function () { send('unreject', { faces: ids, person_id: pid }); } });
      });
    }
    // A named pet is the species their person is; only the others need saying.
    var to = f.state === 'confirmed' ? '' : otherSpecies([f]);
    if (to) items.push({ label: makeSpeciesLabel(to), run: function () { setSpecies(ids, to).then(changed).catch(failed); } });
    items.push({ label: tr(f.species ? 'pet.not_a.pet' : 'facecheck.not_face'), run: function () { send('not-face', { faces: ids }); } });
    return items;
  }, tr('person.more_face')));
  return line;
}

// After a change in the info panel: its faces again, and the people.
function infoFacesChanged(id) {
  lb.facesChanged = true;
  peopleChanged();
  if (!lb.details || lb.details.id !== id) return;
  api(LIBAPI + '/files/' + id).then(function (info) {
    if (!lb.details || lb.details.id !== id) return;
    lb.details = info;
    lb.hover = null;
    renderPanel();
  }).catch(function () {});
}

// ---- drawing a missed face

// "+ Add face": the panel steps aside, a box is dragged over the photo
// (mouse, pen or finger), then named. A video (never scanned for faces) gets
// no drawing: a small box is placed in a corner (a little further along for
// each person already there) and the dialog opens at once. The person is
// linked to the whole clip and no box is ever shown.
function startDrawing() {
  var stage = $('stage'), img = stage.querySelector('img, video');
  if (!img || !lb.details) return;
  stopDrawing();
  if (img.tagName === 'VIDEO') {
    var n = (lb.details.faces || []).length % 8;
    nameDrawnFace(lb.details, [Math.round((0.02 + 0.1 * n) * 10000) / 10000, 0.02, 0.08, 0.08]);
    return;
  }
  var info = lb.details;
  var panel = $('lb-panel');
  lb.drawing = { panelWasOpen: !panel.hidden };
  panel.hidden = true;
  drawFaces();
  var r = img.getBoundingClientRect(), s = stage.getBoundingClientRect();
  var layer = el('div', 'draw-layer');
  layer.style.left = (r.left - s.left) + 'px';
  layer.style.top = (r.top - s.top) + 'px';
  layer.style.width = r.width + 'px';
  layer.style.height = r.height + 'px';
  var rect = el('div', 'draw-box');
  rect.hidden = true;
  layer.appendChild(rect);
  stage.appendChild(layer);
  var hint = el('div', 'draw-hint');
  hint.appendChild(el('span', '', tr('draw.hint')));
  var cancel = el('button', 'btn quiet small', tr('app.cancel'));
  cancel.onclick = function () { stopDrawing(); };
  hint.appendChild(cancel);
  $('lightbox').appendChild(hint);
  lb.drawing.layer = layer;
  lb.drawing.hint = hint;
  var start = null, box = null;
  var at = function (ev) {
    var lr = layer.getBoundingClientRect();
    return [Math.min(Math.max(ev.clientX - lr.left, 0), lr.width), Math.min(Math.max(ev.clientY - lr.top, 0), lr.height)];
  };
  layer.addEventListener('pointerdown', function (ev) {
    ev.preventDefault();
    layer.setPointerCapture(ev.pointerId);
    start = at(ev);
    box = null;
  });
  layer.addEventListener('pointermove', function (ev) {
    if (!start) return;
    var p = at(ev);
    box = [Math.min(start[0], p[0]), Math.min(start[1], p[1]), Math.abs(p[0] - start[0]), Math.abs(p[1] - start[1])];
    rect.hidden = false;
    rect.style.left = box[0] + 'px';
    rect.style.top = box[1] + 'px';
    rect.style.width = box[2] + 'px';
    rect.style.height = box[3] + 'px';
  });
  var end = function () {
    if (!start) return;
    start = null;
    if (!box || box[2] < 8 || box[3] < 8) { rect.hidden = true; return; }
    var w = layer.clientWidth, h = layer.clientHeight;
    var frac = [box[0] / w, box[1] / h, box[2] / w, box[3] / h].map(function (v) { return Math.round(v * 10000) / 10000; });
    frac[2] = Math.min(frac[2], 1 - frac[0]);
    frac[3] = Math.min(frac[3], 1 - frac[1]);
    nameDrawnFace(info, frac);
  };
  layer.addEventListener('pointerup', end);
  layer.addEventListener('pointercancel', function () { start = null; rect.hidden = true; });
}

function stopDrawing() {
  var d = lb.drawing;
  if (!d) return;
  lb.drawing = null;
  if (d.layer) d.layer.remove();
  if (d.hint) d.hint.remove();
  if (d.panelWasOpen && !$('lightbox').hidden) { $('lb-panel').hidden = false; renderPanel(); }
}

function nameDrawnFace(info, frac) {
  var body = el('div');
  var question = el('p', '', tr('unnamed.who'));
  question.id = 'draw-who';
  body.appendChild(question);
  // What was drawn: a person's face, or a cat or a dog the detector missed (a
  // pet is embedded as a whole, a face by its eyes, nose and mouth). Whoever
  // draws a pet knows which one it is, and nothing changes it later.
  var kind = '';
  // For the keyboard a radio group: one Tab stop, the arrows change the kind.
  var kinds = el('div', 'kinds');
  kinds.setAttribute('role', 'radiogroup');
  kinds.setAttribute('aria-labelledby', question.id);
  var field, hint;
  var update = function () {
    Array.prototype.forEach.call(kinds.children, function (b) {
      var on = b.dataset.kind === kind;
      b.classList.toggle('on', on);
      b.setAttribute('aria-checked', on ? 'true' : 'false');
      b.tabIndex = on ? 0 : -1;
    });
    hint.textContent = tr(video ? 'draw.note_video' : kind ? 'draw.note_pet' : 'draw.note');
    field.input.value = '';
    field.input.dispatchEvent(new Event('input'));
  };
  var video = info.kind === 'video';
  [['', 'person', 'draw.kind.person'], ['cat', 'cat', 'draw.kind.cat'], ['dog', 'dog', 'draw.kind.dog']].forEach(function (k) {
    var b = el('button');
    b.appendChild(icon(k[1]));
    b.appendChild(document.createTextNode(tr(k[2])));
    b.type = 'button';
    b.setAttribute('role', 'radio');
    b.dataset.kind = k[0];
    b.onclick = function () { kind = k[0]; update(); field.input.focus(); };
    kinds.appendChild(b);
  });
  kinds.addEventListener('keydown', function (ev) {
    var step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[ev.key];
    if (!step) return;
    ev.preventDefault();
    var bs = Array.prototype.slice.call(kinds.children);
    var next = bs[(bs.indexOf(document.activeElement) + step + bs.length) % bs.length];
    kind = next.dataset.kind;
    update();
    next.focus();
  });
  // One request at a time: the dialog stays open until the reply, so a second
  // click, Enter or pick meanwhile would add the face again. (A failure opens
  // the error dialog over this one, so nothing needs enabling again.)
  var saving = false, buttons = [];
  var save = function (who) {
    if (saving) return;
    saving = true;
    buttons.forEach(function (b) { b.disabled = true; });
    field.input.disabled = true;
    post(LIBAPI + '/faces/manual', Object.assign({ file: info.id, box: frac, species: kind || undefined }, who)).then(function () {
      closeModal();
      stopDrawing();
      toast(tr(kind ? 'draw.added_pet' : 'draw.added'));
      infoFacesChanged(info.id);
    }).catch(failed);
  };
  field = personField(function (who) { save(who); }, { allowNew: true, placeholder: tr('people.name_placeholder'), species: function () { return kind; } });
  body.appendChild(kinds);
  body.appendChild(field);
  hint = el('p', 'hint');
  hint.id = 'draw-note';
  field.input.setAttribute('aria-describedby', hint.id);
  body.appendChild(hint);
  update();
  buttons = openModal(tr('draw.title'), body, [
    video ? null : { label: tr('draw.again'), cls: 'quiet', onclick: function () { var r = lb.drawing && lb.drawing.layer.querySelector('.draw-box'); if (r) r.hidden = true; } },
    { label: tr('app.cancel'), cls: 'quiet', onclick: function () { stopDrawing(); } },
    { label: tr('draw.save'), onclick: function () { var who = field.value(); if (!who) return false; save(who); return false; } },
 ].filter(Boolean));
  field.input.focus();
}

// ------------------------------------------------------------------ start

// ------------------------------------------------------------------ several drives

// Each drive has its own library; the routes carry its id. The drive that is
// shown is remembered; switching reloads the page. A drive that is not
// plugged in is listed as offline and the rest keeps working.
var drives = { list: [], current: null, offlineTimer: null, scope: 'one', allLibs: [] };

function chosenLibrary(libs) {
  var saved = null;
  try { saved = localStorage.getItem('shoebox-library'); } catch (e) { /* private window */ }
  drives.scope = saved === 'all' && libs.length > 1 ? 'all' : 'one';
  document.body.classList.toggle('scope-all', drives.scope === 'all');
  return libs.filter(function (l) { return l.id === saved; })[0]
    || libs.filter(function (l) { return l.online; })[0] || libs[0];
}

// `id` is a drive's id or 'all' (the common timeline); `hash` opens it with a filter.
function switchLibrary(id, hash) {
  try { localStorage.setItem('shoebox-library', id); } catch (e) { /* ignore */ }
  location.hash = hash || '';
  location.reload();
}

// A comparison stays on when another drive is picked: it is dropped there, with a note, if that drive cannot show it.
function keepCompare() { return state.filter && state.filter.compare && !state.filter.view ? 'compare=' + state.filter.compare : ''; }

function renderDriveList() {
  var multi = drives.list.length > 1;
  $('drives-box').hidden = !multi;
  $('nav-drives').hidden = !multi;
  var ul = $('drive-list');
  ul.textContent = '';
  if (multi) {
    var all = el('li', 'all-entry' + (isAll() ? ' current' : ''));
    var ab = el('button');
    ab.appendChild(el('span', 'dot'));
    ab.appendChild(el('span', 'name', tr('drives.all')));
    setTip(ab, tr('drives.all_hint'));
    ab.onclick = function () { if (!isAll()) switchLibrary('all', keepCompare()); };
    all.appendChild(ab);
    ul.appendChild(all);
  }
  drives.list.forEach(function (l) {
    var li = el('li', (!isAll() && l.id === drives.current.id ? 'current ' : '') + (l.online ? '' : 'offline'));
    var b = el('button');
    b.appendChild(el('span', 'dot'));
    b.appendChild(el('span', 'name', l.name));
    if (!l.online) b.appendChild(el('span', 'tag', tr('drives.offline')));
    else if (l.role === 'backup') b.appendChild(el('span', 'tag', tr('drives.backup')));
    b.title = l.online ? l.name : l.reason ? tr('drives.not_connected_reason', { name: l.name, reason: l.reason }) : tr('drives.not_connected', { name: l.name });
    b.onclick = function () { if (isAll() || l.id !== drives.current.id) switchLibrary(l.id, keepCompare()); };
    li.appendChild(b);
    ul.appendChild(li);
  });
}

// The chosen drive is offline: say so, and come back by itself when it is plugged in.
function showOffline() {
  var page = $('page');
  $('page').hidden = false;
  $('sizer').hidden = true;
  $('empty').hidden = true;
  clearNode(page);
  page.appendChild(el('h2', '', tr('drives.offline_title', { name: drives.current.name })));
  page.appendChild(el('p', 'sub', tr('drives.offline_text')));
  var overview = el('button', 'btn quiet', tr('drives.show_all'));
  overview.onclick = function () { location.hash = '#view=drives'; location.reload(); };
  page.appendChild(overview);
  $('title').textContent = drives.current.name;
  clearInterval(drives.offlineTimer);
  drives.offlineTimer = setInterval(function () {
    api('/api/libraries').then(function (libs) {
      var now = libs.filter(function (l) { return l.id === drives.current.id; })[0];
      if (now && now.online) location.reload();
    }).catch(function () {});
  }, 4000);
}

var drivesTab = 'people';

// Groups of drives: an original followed by its backups (the confirmed ones, or
// an undecided drive that looks like a copy of it), so a backup always sits
// next to the drive it copies.
function driveOrder(list, backups) {
  var primaryOf = {};
  backups.forEach(function (b) { if (b.primary) primaryOf[b.library] = b.primary; });
  list.forEach(function (d) { if (!primaryOf[d.library] && d.suggested_backup_of) primaryOf[d.library] = d.suggested_backup_of; });
  var names = list.map(function (d) { return d.name; });
  var isCopy = function (d) { return primaryOf[d.library] && names.indexOf(primaryOf[d.library]) >= 0 && primaryOf[d.library] !== d.name; };
  var out = [];
  list.forEach(function (d) {
    if (isCopy(d)) return;
    out.push([d].concat(list.filter(function (c) { return isCopy(c) && primaryOf[c.library] === d.name; })));
  });
  return out;
}

// A Unix time for a drive card: "today, 14:05", "4 days ago, 7:30", and from a
// week on the date ("21 Sep 2026, 19:12"). The title has the exact date.
function whenOf(t) {
  var d = new Date(t * 1000), now = new Date();
  var midnight = function (x) { return new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime(); };
  var days = Math.round((midnight(now) - midnight(d)) / 86400000);
  var time = d.toLocaleTimeString(I18n.lang, { hour: 'numeric', minute: '2-digit', hourCycle: 'h23' });
  var text = days <= 0 ? tr('time.today_at', { time: time })
    : days <= 7 ? trn('time.days_ago_at', days, { time: time })
    : I18n.date(d, { day: 'numeric', month: 'short', year: 'numeric' }) + ', ' + time;
  var span = el('b', '', text);
  span.title = d.toLocaleString(I18n.lang, { dateStyle: 'full', timeStyle: 'short', hourCycle: 'h23' });
  return span;
}

// One footer cell: "Scanned today, 14:05" with an optional (i) hint after the date.
function footCell(label, t, hint) {
  var c = el('span', 'cell', label + ' ');
  if (!t) { c.appendChild(el('b', '', tr('time.never'))); return c; }
  c.appendChild(whenOf(t));
  if (hint) { var i = el('i', 'info', 'i'); setTip(i, hint); c.appendChild(i); }
  return c;
}

// Carry over data (phase 15a): bring what was made on one drive (tags, hearts,
// people, dates, places) to the same photos on another one, matched by content.
// Nothing on the source changes and no photo is touched; what the target has
// stays. `list` is /api/all/drives, `to` the receiving drive's entry and
// `preferred` the id of the drive to start with. The source can also be a
// userdata.json on this computer (phase 15b): `file` is the one chosen
// ({path, folder}); only a browser on this computer can choose one.
function transferDialog(list, to, preferred, file) {
  var label = function (x) { return x.volume || x.name; };
  var sources = list.filter(function (x) { return x.online && x.shown_in_all && x.library !== to.library; });
  var canFile = !!state.exportOk;
  if (!sources.length && !canFile) return;
  // No other drive: the only source is a file, so choose it first.
  if (!sources.length && !file) { transferFilePicker(list, to, null); return; }
  var body = el('div', 'transfer');
  var from = el('select');
  sources.forEach(function (x) {
    var o = el('option', '', label(x));
    o.value = x.library;
    if (x.library === preferred && !file) o.selected = true;
    from.appendChild(o);
  });
  if (file) {
    var chosen = el('option', '', file.name);
    chosen.value = '#chosen';
    chosen.selected = true;
    from.appendChild(chosen);
  }
  if (canFile) {
    var other = el('option', '', tr('transfer.from_file'));
    other.value = '#file';
    from.appendChild(other);
  }
  var pick = el('p', 'transfer-from');
  pick.appendChild(el('span', '', tr('transfer.from') + ' '));
  pick.appendChild(from);
  var explain = el('p', '');
  var boxes = el('div', 'transfer-boxes');
  var note = el('p', 'hint');
  [pick, explain, boxes, note].forEach(function (n) { body.appendChild(n); });
  var kinds = [['tags', 'transfer.tags', 'count.photos'], ['hearts', 'transfer.hearts', 'count.photos'], ['people', 'transfer.people', 'count.faces'], ['dates', 'transfer.dates', 'count.photos'], ['places', 'transfer.places', 'count.photos'], ['hidden', 'transfer.hidden', 'count.photos']];
  var checks = {}, seq = 0, go = null, anything = false, last = from.value;
  // What the routes are told about the source.
  var source = function () { return from.value === '#chosen' ? { file: file.path, to: to.library } : { from: from.value, to: to.library }; };
  var sync = function () {
    go.disabled = !anything || !kinds.some(function (k) { return checks[k[0]] && checks[k[0]].checked; });
  };
  var show = function (p) {
    if (p.file) {
      var when = I18n.dateTime(new Date(p.written_at * 1000), { dateStyle: 'medium', timeStyle: 'short' });
      explain.textContent = tr('transfer.explain_file', { file: p.file, date: when, to: label(to) });
    } else {
      var f = sources.filter(function (x) { return x.library === from.value; })[0];
      explain.textContent = tr('transfer.explain', { from: label(f), to: label(to) });
    }
    boxes.textContent = '';
    anything = false;
    kinds.forEach(function (k) {
      var hidden = k[0] === 'hidden';
      // Hidden photos and folders are counted together; the box is not ticked unless the user does.
      var n = hidden ? (p.counts.hidden || 0) + (p.counts.hidden_folders || 0) : p.counts[k[0]];
      var line = el('label', 'transfer-box' + (n ? '' : ' off'));
      var cb = el('input');
      cb.type = 'checkbox';
      cb.checked = !!n && !hidden;
      cb.disabled = !n;
      cb.onchange = sync;
      checks[k[0]] = cb;
      line.appendChild(cb);
      var amount = trn(k[2], n);
      if (hidden) {
        var parts = [];
        if (p.counts.hidden) parts.push(trn('count.photos', p.counts.hidden));
        if (p.counts.hidden_folders) parts.push(trn('count.folders', p.counts.hidden_folders));
        amount = parts.join(', ') || trn('count.photos', 0);
        setTip(line, tr('transfer.hidden_hint'));
      }
      line.appendChild(document.createTextNode(' ' + tr(k[1], { n: amount })));
      boxes.appendChild(line);
      if (n) anything = true;
    });
    note.textContent = !anything ? tr(p.file ? 'transfer.none_file' : 'transfer.none', { to: label(to) })
      : p.source_emptied ? tr('transfer.before_wipe', { from: label(sources.filter(function (x) { return x.library === from.value; })[0]) }) : '';
    sync();
  };
  var load = function () {
    var mine = ++seq;
    go.disabled = true;
    post('/api/all/transfer/plan', source()).then(function (p) {
      if (mine === seq) show(p);
    }).catch(function (e) {
      if (mine !== seq) return;
      // A file that cannot be read: say why and let another source be chosen.
      if (from.value === '#chosen') { explain.textContent = ''; boxes.textContent = ''; note.textContent = e.message; anything = false; sync(); return; }
      closeModal(); failed(e);
    });
  };
  from.onchange = function () {
    if (from.value === '#file') { from.value = last; transferFilePicker(list, to, file); return; }
    last = from.value;
    load();
  };
  var buttons = openModal(tr('transfer.title'), body, [
    { label: tr('app.cancel'), cls: 'quiet' },
    {
      label: tr('transfer.go'),
      onclick: function (btn) {
        var req = source();
        req.boxes = {};
        kinds.forEach(function (k) { req.boxes[k[0]] = !!(checks[k[0]] && checks[k[0]].checked); });
        btn.disabled = true;
        post('/api/all/transfer', req).then(function (r) {
          closeModal();
          toast(r.counts.photos || r.counts.hidden_folders ? tr('transfer.done', { photos: trn('count.photos', r.counts.photos) }) : tr('transfer.nothing_new', { to: label(to) }));
          if (state.filter && state.filter.view === 'drives') loadDrivesPage();
          refreshDrives();
        }).catch(function (e) { btn.disabled = false; failed(e); });
        return false;
      },
    },
  ]);
  go = buttons[1];
  load();
}

// Choose the userdata.json to carry over from, in this computer's folders;
// then back to the dialog with it (or, on Cancel, with what was there before).
function transferFilePicker(list, to, file) {
  var base = '/api/lib/' + to.library;
  var others = list.some(function (x) { return x.online && x.shown_in_all && x.library !== to.library; });
  api(base + '/export/places').then(function (places) {
    pathPicker({
      title: tr('transfer.pick_title'),
      hint: tr('transfer.pick_hint'),
      base: base,
      places: places,
      start: file ? file.folder : null,
      files: true,
      use: tr('transfer.use_file'),
      onPick: function (path, folder) {
        var name = path.split(/[\\/]/).pop();
        transferDialog(list, to, null, { path: path, folder: folder, name: name });
      },
      onCancel: function () { if (file || others) transferDialog(list, to, null, file); else closeModal(); },
    });
  }).catch(failed);
}

function loadDrivesPage() {
  var page = $('page');
  clearNode(page);
  page.appendChild(el('h2', '', tr('side.manage_drives')));
  page.appendChild(el('p', 'sub', tr('drives.page_sub')));
  var cards = el('div', 'drive-cards'), tabs = el('div', 'tabs underline'), tabBox = el('div');
  page.appendChild(cards);
  page.appendChild(tabs);
  page.appendChild(tabBox);
  var role = function (d, value, of) {
    post('/api/all/role', { library: d.library, role: value, of: of || null }).then(function () { toast(tr('drives.role_toast', { name: d.name, role: tr('drives.role.' + value) })); loadDrivesPage(); refreshDrives(); }).catch(failed);
  };
  Promise.all([api('/api/all/drives'), api('/api/all/backups'), api('/api/all/transfer').catch(function () { return []; })]).then(function (res) {
    var list = res[0], backups = res[1], hints = res[2];
    var primaryOf = {};
    backups.forEach(function (b) { if (b.primary) primaryOf[b.library] = b.primary; });
    driveOrder(list, backups).forEach(function (group) {
      var box = el('div', 'drive-group');
      cards.appendChild(box);
      group.forEach(function (d) {
        var of = primaryOf[d.library] || d.suggested_backup_of;
        // The drive it copies, by its disk name (its folder's name only where that is all there is).
        var ofDisk = (list.filter(function (x) { return x.name === of; })[0] || {}).volume || of;
        var kind = !d.online ? 'off' : d.role === 'backup' ? 'bk' : d.role === 'unknown' ? 'undecided' : 'orig';
        var card = el('div', 'drive-card ' + kind);
        // The same row on every card says what the drive is.
        var strip = el('div', 'strip');
        strip.appendChild(el('span', '', kind === 'off' ? tr('drives.offline') : kind === 'bk' ? (of ? tr('drives.strip_backup', { primary: ofDisk }) : tr('drives.role.backup')) : tr('drives.role.' + d.role)));
        if (d.online && !d.shown_in_all) strip.appendChild(el('span', 'r', tr('drives.not_in_timeline')));
        card.appendChild(strip);
        var body = el('div', 'body');
        var top = el('div', 'top');
        var name = el('div', 'name');
        name.appendChild(el('span', '', d.name));
        name.appendChild(el('span', 'on', d.online ? '● ' + tr('drives.online') : ''));
        top.appendChild(name);
        if (d.online) {
          var sel = el('select');
          setTip(sel, tr('drives.role_hint'));
          [['unknown', tr('drives.role.unknown')], ['separate', tr('drives.role.separate')], ['backup', tr('drives.role.backup')]].forEach(function (o) {
            var opt = el('option', '', o[1]); opt.value = o[0]; if (d.role === o[0]) opt.selected = true; sel.appendChild(opt);
          });
          sel.onchange = function () { role(d, sel.value); };
          top.appendChild(sel);
        }
        body.appendChild(top);
        // The disk the folder is on: the folder's own name says little when the
        // drive holds more than photos.
        if (d.volume) {
          var where = el('div', 'where');
          where.appendChild(icon('drive', 'ico'));
          where.appendChild(el('b', '', d.volume));
          where.appendChild(el('span', '', ' · ' + d.folder));
          body.appendChild(where);
        }
        if (d.online && d.role === 'backup') {
          var slot = el('div', 'backup-status');
          renderBackup(slot, backups.filter(function (x) { return x.library === d.library; })[0], d);
          body.appendChild(slot);
        } else if (d.online) {
          var orig = el('div', 'backup-status');
          renderOriginal(orig, d, backups.filter(function (x) { return x.primary_library === d.library; })[0]);
          body.appendChild(orig);
        }
        if (d.online && d.role === 'unknown' && d.suggested_backup_of) {
          var bn = el('div', 'banner warn');
          bn.appendChild(el('div', '', tr('drives.looks_backup', { name: d.name, other: d.suggested_backup_of })));
          var row = el('div', 'row');
          var yes = el('button', 'btn', tr('drives.yes_backup')); yes.onclick = function () { role(d, 'backup'); };
          var no = el('button', 'btn quiet', tr('drives.no_backup')); no.onclick = function () { role(d, 'separate'); };
          row.appendChild(yes); row.appendChild(no);
          bn.appendChild(row);
          body.appendChild(bn);
        }
        // Carry over data: only between drives that take part (not backups, not
        // drives that may be one), and only where there is another drive or,
        // on this computer, a userdata.json to take from.
        if (d.online && d.shown_in_all && (state.exportOk || list.some(function (x) { return x.online && x.shown_in_all && x.library !== d.library; }))) {
          var mine = hints.filter(function (h) { return h.to === d.library; }).sort(function (a, b) { return b.counts.photos - a.counts.photos; });
          var transfer = el('div', 'transfer-note');
          if (mine.length) {
            var src = list.filter(function (x) { return x.library === mine[0].from; })[0];
            transfer.appendChild(el('span', '', tr('transfer.hint', { from: (src && src.volume) || mine[0].from_name, photos: trn('count.photos', mine[0].counts.photos) }) + ' '));
          }
          var cb = el('button', 'btn' + (mine.length ? '' : ' quiet'), tr('transfer.button'));
          cb.onclick = function () { transferDialog(list, d, mine.length ? mine[0].from : null); };
          transfer.appendChild(cb);
          body.appendChild(transfer);
        }
        card.appendChild(body);
        if (d.online) {
          var foot = el('div', 'foot');
          foot.appendChild(footCell(tr('drives.scanned'), d.last_scan));
          foot.appendChild(footCell(tr(d.role === 'backup' ? 'drives.last_backup' : 'drives.new_files'), d.last_new_files, tr('drives.new_files_hint')));
          card.appendChild(foot);
        }
        box.appendChild(card);
      });
    });
  }).catch(failed);

  var show = function () {
    tabs.textContent = '';
    [['people', tr('drives.people_across')], ['dups', tr('drives.dups_across')]].forEach(function (t) {
      var b = el('button', drivesTab === t[0] ? 'on' : '', t[1]);
      b.onclick = function () { drivesTab = t[0]; show(); };
      tabs.appendChild(b);
    });
    tabBox.textContent = '';
    var mine = tabBox, tab = drivesTab;
    var alive = function () { return mine.parentNode && drivesTab === tab; };
    if (tab === 'dups') return renderDuplicatesAcross(tabBox, alive, true);
    api('/api/all/people').then(function (r) {
      if (!alive()) return;
      if (r.offline.length) tabBox.appendChild(el('p', 'sub', tr('drives.people_offline', { names: r.offline.join(', ') })));
      if (!r.people.length) tabBox.appendChild(el('p', 'sub', tr('drives.nobody')));
      var grid = el('div', 'people-grid');
      r.people.forEach(function (p) {
        var c = el('div', 'person-merged');
        c.appendChild(el('div', 'n', p.name));
        c.appendChild(el('div', 'sub', trn('count.photos', p.photos) + (p.group ? ' · ' + p.group : '')));
        // The drive first (its disk name; the folder name only where that is all there is).
        var pills = el('div', 'pills');
        p.libraries.forEach(function (l) {
          var pill = el('span', 'pill drive', (l.volume || l.name) + ' · ' + l.faces);
          if (l.volume) setTip(pill, tr('drives.person_folder', { name: l.name }));
          pills.appendChild(pill);
        });
        c.appendChild(pills);
        var go = el('button', 'btn quiet', tr('cmp.show_photos'));
        go.onclick = function () { switchLibrary('all', 'person=' + encodeURIComponent(p.name)); };
        c.appendChild(go);
        grid.appendChild(c);
      });
      tabBox.appendChild(grid);
    }).catch(failed);
  };
  show();
}

// The common timeline came in: which drives its ids refer to, and what is left out.
function allTimelineLoaded(data) {
  drives.allLibs = data.libs;
  otherCopies = {};
  var note = $('all-note');
  if (!note) {
    note = el('div', 'note-bar');
    note.id = 'all-note';
    $('scroller').insertBefore(note, $('filters'));
  }
  var out = data.left_out.map(function (x) { return x.name + ' (' + x.reason.split(';')[0] + ')'; });
  if (data.flat && data.libs.length === 2) {
    note.textContent = tr('cmp.pairs_note', { original: data.libs[0].volume || data.libs[0].name, backup: data.libs[1].volume || data.libs[1].name, n: I18n.number(data.count) });
    $('status').textContent = tr('drives.status', { photos: trn('count.photos', data.count), drives: trn('count.drives', data.libs.length) });
    return;
  }
  note.textContent = tr('drives.note', { libs: data.libs.map(function (l) { return l.name; }).join(', ') }) + (out.length ? tr('drives.note_out', { out: out.join(', ') }) : '');
  $('status').textContent = tr('drives.status', { photos: trn('count.photos', data.count), drives: trn('count.drives', data.libs.length) })
    + (data.hidden ? ' · ' + trn('count.hidden_copies', data.hidden) : '');
}

// "Show in Finder" for a file on one of the drives (named by drive and path).
function revealOn(library, path) {
  post('/api/all/reveal', { library: library, path: path }).then(function (r) {
    toast(tr('reveal.shown', { app: r.app === 'file manager' ? tr('reveal.file_manager') : r.app }));
  }).catch(failed);
}

// A backup drive against the drive it copies, from the two indexes (nothing is
// read from the photos): how much of it is on the backup, what is new since the
// last backup, what differs, what only the backup has. Bit rot on the backup
// itself is found by "Backup check" with re-reading in the launcher
// (`shoebox backup … --deep`).
function renderBackup(slot, b, d) {
  slot.textContent = '';
  if (!b || !b.primary) {
    slot.appendChild(el('p', 'sub', (b && b.note) || tr('drives.backup_none')));
    return;
  }
  if (b.compared) {
    var pct = b.covered >= b.compared ? 100 : Math.floor(b.covered * 100 / b.compared);
    var prog = el('div', 'prog');
    var t = el('div', 't');
    var lead = el('span');
    lead.appendChild(el('b', '', I18n.number(b.covered)));
    lead.appendChild(document.createTextNode(' ' + tr('drives.bar_of', { total: I18n.number(b.compared) })));
    t.appendChild(lead);
    t.appendChild(el('b', '', pct + ' %'));
    prog.appendChild(t);
    var bar = el('div', 'meter');
    var fill = el('i');
    fill.style.width = pct + '%';
    bar.appendChild(fill);
    prog.appendChild(bar);
    slot.appendChild(prog);
  }
  if (b.unhashed) slot.appendChild(el('div', 'sub', tr('drives.backup_unhashed', { files: trn('count.files', b.unhashed), primary: b.primary }).trim()));
  // "Show photos" opens the timeline with that comparison (the first one on
  // both drives of the pair, in one list).
  slot.appendChild(backupLists([
    [b.different, 'drives.sum_different', b.different_files, b.primary_library, 'warn', function () { switchLibrary('all', 'compare=different'); }],
    [b.extra, 'drives.sum_extra', b.extra_files, d.library, '', function () { switchLibrary(d.library, 'compare=backup'); }]
  ], b));
}

// The original's card: how many files, and what is not on its backup yet.
function renderOriginal(slot, d, b) {
  slot.textContent = '';
  // The count as in the backup's bar line (a bold number), so the cards line up.
  var line = el('div', 'statline');
  var t = el('div', 't');
  var lead = el('span');
  var text = trn('count.files', d.files), num = I18n.number(d.files), at = text.indexOf(num);
  if (at >= 0) {
    lead.appendChild(document.createTextNode(text.slice(0, at)));
    lead.appendChild(el('b', '', num));
    lead.appendChild(document.createTextNode(text.slice(at + num.length)));
  } else lead.textContent = text;
  t.appendChild(lead);
  line.appendChild(t);
  slot.appendChild(line);
  if (b && b.missing) {
    slot.appendChild(backupLists([
      [b.missing, 'drives.sum_missing', b.missing_files, d.library, 'warn', function () { switchLibrary(d.library, 'compare=original'); }]
    ], b));
  }
}

function backupLists(lists, b) {
  var wrap = el('div', 'lists');
  lists.forEach(function (l) {
    if (!l[0]) return;
    var det = el('details');
    var sum = el('summary');
    var go = el('button', 'btn quiet small go', tr('cmp.show_photos'));
    go.onclick = function (ev) { ev.preventDefault(); ev.stopPropagation(); l[5](); };
    sum.appendChild(go);
    sum.appendChild(el('b', l[4], I18n.number(l[0])));
    sum.appendChild(document.createTextNode(' ' + tr(l[1])));
    if (l[1] === 'drives.sum_extra') sum.appendChild(el('span', 'sub', ' ' + tr('drives.sum_extra_hint', { primary: b.primary })));
    det.appendChild(sum);
    var ul = el('ul', 'files');
    l[2].forEach(function (f) {
      var li = el('li');
      li.appendChild(el('span', 'path', f.path));
      li.appendChild(el('span', 'size', formatBytes(f.size)));
      if (state.reveal && l[3]) {
        var show = el('button', 'btn quiet reveal');
        show.appendChild(folderIcon());
        show.title = state.reveal;
        show.setAttribute('aria-label', state.reveal);
        show.onclick = function () { revealOn(l[3], f.path); };
        li.appendChild(show);
      }
      ul.appendChild(li);
    });
    if (l[0] > l[2].length) ul.appendChild(el('li', 'more', tr('drives.more', { n: l[0] - l[2].length })));
    det.appendChild(ul);
    wrap.appendChild(det);
  });
  return wrap;
}

// The drives and the status are no longer asked for on a timer: when the
// person comes back to the page (tab, window) after at least 15 s they are
// asked for once, which also notices a scan or a drive plugged in meanwhile.
// Coming back to the page asks for the drives again only where they can have
// changed something: with one drive there is no list and nothing to switch, and
// the Unnamed page (naming faces, in and out of other windows) shows none of it.
function driveRefreshNeeded() {
  if (drives.list.length < 2 && drives.current && drives.current.online) return false;
  return !(state.filter && state.filter.view === 'unnamed');
}

function watchPageReturn() {
  var last = Date.now();
  var check = function () {
    if (document.hidden || Date.now() - last < 60000) return;
    last = Date.now();
    if (driveRefreshNeeded()) refreshDrives().catch(function () {});
    if (!isAll() && drives.current && drives.current.online) loadInfo();
  };
  document.addEventListener('visibilitychange', check);
  window.addEventListener('focus', check);
}

function refreshDrives() {
  return api('/api/libraries').then(function (libs) {
    return api('/api/all/drives').then(function (list) { drives.roles = list; return list; }, function () { drives.roles = null; return []; }).then(function (roles) {
      libs.forEach(function (l) { var r = roles.filter(function (x) { return x.library === l.id; })[0]; l.role = r ? r.role : 'unknown'; });
      drives.list = libs;
      drives.current = libs.filter(function (l) { return drives.current && l.id === drives.current.id; })[0] || chosenLibrary(libs);
      renderDriveList();
    });
  });
}

// The messages first: everything below builds its text from them.
I18n.ready.then(function () {
  var sel = $('lang');
  Object.keys(I18n.langs).forEach(function (code) {
    var o = el('option', '', I18n.langs[code]);
    o.value = code;
    if (code === I18n.lang) o.selected = true;
    sel.appendChild(o);
  });
  sel.onchange = function () { I18n.setLang(sel.value); };
  return api('/api/libraries');
}).then(function (libs) {
  drives.list = libs;
  drives.current = chosenLibrary(libs);
  LIBAPI = '/api/lib/' + drives.current.id;
  return api('/api/session');
}).then(function (s) {
  if (!s.authenticated) {
    if (!s.pin_enabled) $('login-text').textContent = tr('login.local_only');
    showLogin();
    return;
  }
  return refreshDrives().then(loadCompare).then(function () {
    watchPageReturn();
    if (drives.current.online) { loadEventPattern(); loadAllowTrash(); }
    if (!drives.current.online) {
      // Nothing of this library can be loaded: only the overview of the drives can be shown.
      if (readHash().view === 'drives') { state.filter = readHash(); applyFilter(); } else showOffline();
      return;
    }
    state.filter = readHash();
    $('search').value = state.filter.q;
    if (isAll()) {
      // The common timeline: no folders, tags or faces of its own; the drives answer by name.
      $('title').textContent = tr('drives.all');
      document.title = tr('drives.all') + ' · shoebox';
      applyFilter();
      // Only for the label of the "show in the file manager" button.
      api(LIBAPI + '/info').then(function (i) { state.reveal = revealLabel(i.reveal); }).catch(function () {});
      return;
    }
    return Promise.all([loadFolders(), loadInfo(), loadOwnTags(), loadPeople(), loadGeo()]).then(function () {
      applyFilter();
    });
  });
}).catch(function (e) { console.error(e); }).then(function () {
  // Folders, tags and faces load separately; show them once all are there.
  $('sidebar').classList.remove('loading');
});
