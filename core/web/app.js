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

var HEADER_H = 44;
var OVERSCAN = 600; // px above and below the viewport that stay rendered
var KIND_BADGE = { v: '▶', h: '', j: '', p: '' };

var state = {
  filter: { folder: null, tags: [], people: [], q: '', view: null, id: null, tab: null },
  selecting: false,
  selected: {},      // id -> true
  anchor: null,      // index of the last photo clicked while selecting (Shift-click ranges)
  data: null,        // timeline columns from /api/timeline
  live: {},          // still id -> video id
  folders: [],
  folderById: {},
  rows: [],          // layout rows: {top, h, type: 'h'|'r', ...}
  rendered: {},      // row index -> element
  cols: 1,
  cell: 100,
  open: -1,          // index of the item in the lightbox
  indexVersion: null,
  loadSeq: 0,
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
    return r.json().then(function (body) { $('login-error').textContent = body.error || 'Wrong PIN'; });
  }).catch(function (e) { $('login-error').textContent = String(e); });
});

// ------------------------------------------------------------------ filters (in the URL hash)

var VIEWS = ['duplicates', 'trash', 'faces', 'people', 'unnamed', 'person', 'drives'];

function readHash() {
  var f = { folder: null, tags: [], people: [], q: '', view: null, id: null, tab: null };
  location.hash.replace(/^#/, '').split('&').forEach(function (kv) {
    var i = kv.indexOf('=');
    if (i < 0) return;
    var k = kv.slice(0, i), v = decodeURIComponent(kv.slice(i + 1));
    if (k === 'folder') f.folder = parseInt(v, 10) || null;
    // Over all drives tags and people are named: ids differ from drive to drive.
    var all = isAll();
    if (k === 'tag') { var t = all ? v : parseInt(v, 10); if (t && f.tags.indexOf(t) < 0) f.tags.push(t); }
    if (k === 'person') { var pp = all ? v : parseInt(v, 10); if (pp && f.people.indexOf(pp) < 0) f.people.push(pp); }
    if (k === 'q') f.q = v;
    if (k === 'view' && VIEWS.indexOf(v) >= 0) f.view = v;
    if (k === 'id') f.id = parseInt(v, 10) || null;
    if (k === 'tab') f.tab = v;
  });
  // Over all drives only these pages exist (the others belong to one drive).
  if (isAll() && f.view && f.view !== 'duplicates' && f.view !== 'drives') f.view = null;
  return f;
}

// A filter without a view shows the grid. Its terms all have to match: a
// folder, any number of tags and people (`tag`, `person` repeated in the
// URL) and free text.
function setFilter(f) {
  var h = query({ view: f.view, id: f.id, tab: f.tab, folder: f.folder, tag: f.tags || [], person: f.people || [], q: f.q }).replace(/^\?/, '');
  if (h === location.hash.replace(/^#/, '')) { applyFilter(); return; }
  location.hash = h; // triggers hashchange -> applyFilter
}

// The current filter with some terms changed.
function withFilter(changes) {
  var f = state.filter;
  return Object.assign({ folder: f.folder, tags: f.tags.slice(), people: f.people.slice(), q: f.q }, changes);
}

function showView(view, id, tab) { setFilter({ view: view, id: id, tab: tab, folder: null, tags: [], people: [], q: '' }); }

function applyFilter() {
  state.filter = readHash();
  $('search').value = state.filter.q;
  renderChips();
  // Still typing: offer what narrows the new search down.
  if (document.activeElement === $('search')) suggest($('search').value);
  markActiveFolder();
  closeSidebarOnPhone();
  var view = state.filter.view;
  $('page').hidden = !view;
  $('sizer').hidden = !!view;
  $('nav-dups').classList.toggle('active', view === 'duplicates');
  $('nav-trash').classList.toggle('active', view === 'trash');
  $('nav-faces').classList.toggle('active', view === 'faces');
  $('nav-drives').classList.toggle('active', view === 'drives');
  markFacesSection();
  updateSections();
  if (view) {
    endSelection();
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
  var add = function (label, clear) {
    var c = el('span', 'chip', label);
    var x = el('button', '', '✕');
    x.title = 'Remove from the search';
    x.onclick = clear;
    c.appendChild(x);
    box.appendChild(c);
  };
  if (f.folder && state.folderById[f.folder]) {
    var folder = state.folderById[f.folder];
    add('📁 ' + (folder.path || folder.name), function () { setFilter(withFilter({ folder: null })); });
    if (folder.path) {
      var edit = el('button', 'edit', '✎');
      edit.title = 'Rename or move this folder';
      edit.onclick = function () { renameFolder(folder); };
      box.lastChild.insertBefore(edit, box.lastChild.lastChild);
    }
  }
  f.tags.forEach(function (id) {
    add('# ' + (state.tagNames[id] || id), function () {
      setFilter(withFilter({ tags: f.tags.filter(function (t) { return t !== id; }) }));
    });
  });
  f.people.forEach(function (id) {
    add('👤 ' + (state.personNames[id] || id), function () {
      setFilter(withFilter({ people: f.people.filter(function (p) { return p !== id; }) }));
    });
  });
  if (f.q) add('“' + f.q + '”', function () { setFilter(withFilter({ q: '' })); });
  var terms = box.children.length;
  if (terms) {
    var more = el('button', 'chip more', '+');
    more.title = 'Add a tag, person or folder to the search';
    more.onclick = function () { $('search').value = ''; $('search').focus(); suggest(''); };
    box.appendChild(more);
  }
  // One click for the whole search; the ✕ on a chip drops just that term.
  if (terms >= 2) {
    var clear = el('button', 'chip clear', 'Clear all');
    clear.title = 'Show all photos again';
    clear.onclick = function () { setFilter({ folder: null, tags: [], people: [], q: '' }); };
    box.appendChild(clear);
  }
  personHead(box, f);
  box.hidden = !box.firstChild;
}
state.tagNames = {};
state.personNames = {};

// ------------------------------------------------------------------ search box

// Typing searches the text right away (words in paths and tag names); the
// list below the box offers tags (counted within the current search) and
// folders, and picking one adds it to the search as a chip.
var searchTimer = null;
var sugg = { items: [], at: -1, seq: 0 };

$('search').addEventListener('input', function () {
  clearTimeout(searchTimer);
  var v = this.value;
  searchTimer = setTimeout(function () { setFilter(withFilter({ q: v.trim() })); }, 300);
  suggest(v);
});
$('search').addEventListener('focus', function () { suggest(this.value); });
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
    clearTimeout(searchTimer);
    closeSuggest();
    setFilter(withFilter({ q: this.value.trim() }));
  } else if (ev.key === 'Escape') {
    closeSuggest();
  } else if (ev.key === 'Backspace' && this.value === '') {
    // Like a token field: the last chip goes.
    var f = state.filter;
    if (f.people.length) setFilter(withFilter({ people: f.people.slice(0, -1) }));
    else if (f.tags.length) setFilter(withFilter({ tags: f.tags.slice(0, -1) }));
    else if (f.folder) setFilter(withFilter({ folder: null }));
  }
});

function suggest(text) {
  var seq = ++sugg.seq, f = state.filter;
  if (f.view) { closeSuggest(); return; }
  if (isAll()) { suggestAll(text, seq); return; }
  var needle = text.trim();
  var within = { q: needle, tag: f.tags, person: f.people, folder: f.folder };
  Promise.all([
    api(LIBAPI + '/tags' + query(Object.assign({ limit: 8 }, within))),
    api(LIBAPI + '/people/search' + query(Object.assign({ limit: 6 }, within))).catch(function () { return []; }),
  ]).then(function (r) {
    if (seq !== sugg.seq) return;
    var items = r[1].map(function (p) {
      state.personNames[p.id] = p.name;
      return { kind: 'person', id: p.id, label: p.name, count: p.photos };
    });
    r[0].forEach(function (t) {
      state.tagNames[t.id] = t.name;
      items.push({ kind: 'tag', id: t.id, label: t.name, count: t.count, folderTag: t.kind === 'folder' });
    });
    if (needle && !f.folder) {
      var low = needle.toLowerCase();
      state.folders.filter(function (fo) { return fo.path && fo.count && fo.name.toLowerCase().indexOf(low) >= 0; })
        .slice(0, 4)
        .forEach(function (fo) { items.push({ kind: 'folder', id: fo.id, label: fo.path, count: fo.count }); });
    }
    showSuggest(items);
  }).catch(function () {});
}

// Over all drives: tags and people by name (the drives share names, not ids).
var allPeople = { at: 0, list: [] };
function suggestAll(text, seq) {
  var needle = text.trim(), low = needle.toLowerCase(), f = state.filter;
  var people = Date.now() - allPeople.at < 30000 ? Promise.resolve(allPeople.list)
    : api('/api/all/people').then(function (r) { allPeople = { at: Date.now(), list: r.people }; return r.people; });
  Promise.all([api('/api/all/tags' + query({ q: needle, limit: 8 })), people.catch(function () { return []; })]).then(function (r) {
    if (seq !== sugg.seq) return;
    var items = r[1].filter(function (p) { return f.people.indexOf(p.name) < 0 && (!low || p.name.toLowerCase().indexOf(low) >= 0); })
      .slice(0, 6).map(function (p) { return { kind: 'person', id: p.name, label: p.name, count: p.photos }; });
    r[0].filter(function (t) { return f.tags.indexOf(t.name) < 0; }).forEach(function (t) {
      items.push({ kind: 'tag', id: t.name, label: t.name, count: t.count, folderTag: t.kind === 'folder' });
    });
    showSuggest(items);
  }).catch(function () {});
}

function showSuggest(items) {
  var box = $('suggest');
  box.textContent = '';
  sugg.items = items;
  sugg.at = -1;
  var last = null;
  items.forEach(function (it, i) {
    if (it.kind !== last) {
      var f = state.filter, narrowed = !isAll() && (f.tags.length || f.people.length || f.folder);
      var head = it.kind === 'tag' ? (narrowed ? 'Tags in these photos' : 'Tags')
        : it.kind === 'person' ? (narrowed ? 'People in these photos' : 'People') : 'Folders';
      box.appendChild(el('div', 'head', head));
      last = it.kind;
    }
    var b = el('button', 'item');
    b.type = 'button';
    b.setAttribute('role', 'option');
    b.appendChild(el('span', 'label', ({ folder: '📁 ', person: '👤 ' }[it.kind] || '# ') + it.label));
    b.appendChild(el('span', 'count', it.count.toLocaleString()));
    b.onmousedown = function (ev) { ev.preventDefault(); }; // keep the focus in the box
    b.onclick = function () { pickSuggest(it); };
    b.dataset.i = i;
    box.appendChild(b);
  });
  box.hidden = !items.length || document.activeElement !== $('search');
}

function markSuggest() {
  Array.prototype.forEach.call($('suggest').querySelectorAll('.item'), function (b) {
    b.classList.toggle('at', String(sugg.at) === b.dataset.i);
  });
}

function closeSuggest() { $('suggest').hidden = true; sugg.at = -1; }

function pickSuggest(it) {
  clearTimeout(searchTimer);
  $('search').value = '';
  var f = state.filter;
  if (it.kind === 'tag') setFilter(withFilter({ tags: f.tags.indexOf(it.id) < 0 ? f.tags.concat([it.id]) : f.tags, q: '' }));
  else if (it.kind === 'person') setFilter(withFilter({ people: f.people.indexOf(it.id) < 0 ? f.people.concat([it.id]) : f.people, q: '' }));
  else setFilter(withFilter({ folder: it.id, q: '' }));
}

// Suggestions for the tag fields (info panel, "Add tag…"): every tag.
var suggestSeq = 0;
function suggestTags(text) {
  var seq = ++suggestSeq;
  if (!text.trim()) return;
  api(LIBAPI + '/tags' + query({ q: text.trim(), limit: 12 })).then(function (tags) {
    if (seq !== suggestSeq) return;
    var list = $('tag-list');
    list.textContent = '';
    tags.forEach(function (t) {
      state.tagNames[t.id] = t.name;
      var o = el('option');
      o.value = t.name;
      o.label = t.count + '';
      list.appendChild(o);
    });
  }).catch(function () {});
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
var ARROW_DOWN = '<svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true"><path fill="currentColor" d="M7.41 8.59 12 13.17l4.59-4.58L18 10l-6 6-6-6z"/></svg>';
var ARROW_RIGHT = '<svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true"><path fill="currentColor" d="M8.59 16.59 13.17 12 8.59 7.41 10 6l6 6-6 6z"/></svg>';
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
  var count = el('span', 'count', f.count.toLocaleString());
  row.appendChild(toggle);
  row.appendChild(name);
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
    setFilter({ folder: f.id, tags: [], people: [], q: state.filter.q });
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
  Array.prototype.forEach.call(document.querySelectorAll('#own-tags .name'), function (b) {
    b.classList.toggle('active', state.filter.tags.indexOf(parseInt(b.dataset.tag, 10)) >= 0);
  });
}

$('all').onclick = function () { setFilter({ folder: null, tags: [], people: [], q: '' }); };
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
  return f.view === 'people' || f.view === 'unnamed' || f.view === 'person' || (!f.view && f.people.length > 0);
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
  c.onclick = function () {
    sectionClosed[c.dataset.section] = !sectionClosed[c.dataset.section];
    try { localStorage.setItem('sectionClosed', JSON.stringify(sectionClosed)); } catch (e) { /* ignore */ }
    updateSections();
  };
});

function closeSidebarOnPhone() { document.body.classList.remove('side-open'); }

// ------------------------------------------------------------------ timeline

function loadTimeline(resetScroll) {
  var seq = ++state.loadSeq;
  var f = state.filter;
  var url = isAll() ? '/api/all/timeline' + query({ tag: f.tags, person: f.people, q: f.q })
    : LIBAPI + '/timeline' + query({ folder: f.folder, tag: f.tags, person: f.people, q: f.q });
  return api(url).then(function (data) {
    if (seq !== state.loadSeq) return;
    if (isAll()) allTimelineLoaded(data);
    state.anchor = null; // indices change with the new list
    var unnamed = !isAll() && (f.tags.some(function (id) { return !state.tagNames[id]; }) || f.people.some(function (id) { return !state.personNames[id]; }));
    data.tags.forEach(function (t) { state.tagNames[t.id] = t.name; });
    data.people.forEach(function (p) { state.personNames[p.id] = p.name; });
    if (unnamed) renderChips(); // a link with tags this page has not seen yet
    state.data = data;
    state.live = {};
    data.live.forEach(function (p) { state.live[p[0]] = p[1]; });
    var scroller = $('scroller');
    var keep = resetScroll ? 0 : scroller.scrollTop;
    relayout();
    scroller.scrollTop = keep;
    render();
    fillYears();
    $('empty').hidden = data.count > 0;
  }).catch(function (e) { console.error(e); });
}

// Month groups over the (already sorted) items.
function groups() {
  var d = state.data, out = [];
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
  if (!y) return 'No date';
  if (!m) return String(y);
  return new Date(y, m - 1, 1).toLocaleDateString(undefined, { month: 'long', year: 'numeric' });
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
  state.cell = width / state.cols;
  var y = 0;
  groups().forEach(function (g) {
    state.rows.push({ type: 'h', top: y, h: HEADER_H, label: monthLabel(g.month), n: g.end - g.start, month: g.month, start: g.start, end: g.end });
    y += HEADER_H;
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
  Object.keys(state.rendered).forEach(function (k) {
    if (!keep[k]) { state.rendered[k].remove(); delete state.rendered[k]; }
  });
  // The month at the top of the viewport.
  var r = rowAt(Math.max(0, top));
  for (; r >= 0; r--) if (state.rows[r].type === 'h') break;
  $('current-month').textContent = r >= 0 ? state.rows[r].label : '';
}

function thumbUrl(i) {
  var d = state.data;
  return fileBase(d.ids[i]) + '/thumb?v=' + d.versions.substr(i * 8, 8);
}

function buildRow(row) {
  if (row.type === 'h') {
    var h = el('div', 'sec', row.label);
    h.appendChild(el('span', 'n', row.n.toLocaleString()));
    // While selecting: the whole month at once (also where Shift is missing, the iPad).
    var pick = el('button', 'pick');
    pick.type = 'button';
    pick.dataset.start = row.start;
    pick.dataset.end = row.end;
    pick.textContent = allSelected(row.start, row.end) ? 'Deselect' : 'Select all';
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

// Videos the server has no poster for (no ffmpeg, or ffmpeg could not read
// them): grab a frame in the browser, two videos at a time. The frames are
// kept for the session only; nothing is stored on the drive.
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
      c.toBlob(function (b) { finish(b ? URL.createObjectURL(b) : null); }, 'image/jpeg', 0.8);
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
  var placeholder = el('option', '', 'Year');
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

var lb = { video: null, details: null, showFaces: false, hover: null, drawing: null };

function openLightbox(i) {
  state.open = i;
  $('lightbox').hidden = false;
  showItem();
}

function closeLightbox() {
  stopDrawing();
  stopMedia();
  $('stage').textContent = '';
  $('lightbox').hidden = true;
  // Opened from a page (face check), not from the grid: back to the page.
  if (lb.restore) { state.open = -1; lb.restore(); lb.restore = null; return; }
  // Keep the grid where the viewer left off, with the last item seen
  // focused (Tab and the arrow keys continue from there).
  var d = state.data, i = state.open;
  state.open = -1;
  if (!d || i < 0) return;
  focusCell(i, true);
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
  stopMedia();
  stage.textContent = '';
  $('lb-prev').hidden = i <= 0;
  $('lb-next').hidden = i >= d.count - 1;
  $('lb-download').href = fileBase(id) + '/original?download=1';
  $('lb-live').hidden = !state.live[id];
  var rot = $('lb-rotate');
  rot.hidden = isAll() || kind === 'v';
  rot.disabled = kind !== 'j';
  rot.title = kind === 'j' ? 'Rotate left (r) · Option-click or Shift+R: right'
    : 'Only JPEG photos can be rotated for now (HEIC, PNG and RAW cannot)';
  $('lb-title').textContent = '';
  lb.hover = null;
  stopDrawing();

  if (kind === 'v') {
    var v = el('video');
    v.controls = true;
    v.autoplay = true;
    v.playsInline = true;
    v.preload = 'metadata';
    v.poster = frames.cache[id] || thumbUrl(i);
    v.src = fileBase(id) + '/original';
    stage.appendChild(v);
  } else {
    var img = el('img');
    img.alt = '';
    img.src = thumbUrl(i); // instant, sharpened when the full image arrives
    stage.appendChild(img);
    img.onload = drawFaces;
    var full = new Image();
    full.onload = function () { if (state.open === i) img.src = full.src; };
    full.src = viewUrl(i);
    // Warm up the neighbours.
    [i + 1, i - 1].forEach(function (j) {
      if (j >= 0 && j < d.count && d.kinds[j] !== 'v') new Image().src = viewUrl(j);
    });
  }

  api(fileBase(id)).then(function (info) {
    if (state.open !== i) return;
    lb.details = info;
    $('lb-title').textContent = formatDate(info) + ' · ' + info.name;
    if (!$('lb-panel').hidden) renderPanel();
    drawFaces();
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
  if (isAll()) return;
  if ($('lb-panel').hidden || !info || !info.faces || !img || img.hidden) return;
  if (info.id !== state.data.ids[state.open]) return;
  var r = img.getBoundingClientRect(), s = stage.getBoundingClientRect();
  info.faces.forEach(function (f, k) {
    var hot = lb.hover === k;
    if (!lb.showFaces && !hot) return;
    var b = el('div', 'face-box' + (hot ? ' hot' : '') + (f.manual != null ? ' drawn' : ''));
    b.style.left = (r.left - s.left + f.x * r.width) + 'px';
    b.style.top = (r.top - s.top + f.y * r.height) + 'px';
    b.style.width = (f.w * r.width) + 'px';
    b.style.height = (f.h * r.height) + 'px';
    if (f.person && f.state !== 'ignored') {
      b.appendChild(el('span', 'face-name', f.person.name + (f.state === 'confirmed' ? '' : '?')));
    }
    b.title = f.score != null ? 'score ' + f.score.toFixed(2) : 'drawn by hand';
    stage.appendChild(b);
  });
}
window.addEventListener('resize', drawFaces);

function viewUrl(i) {
  var d = state.data;
  return fileBase(d.ids[i]) + '/view?v=' + d.versions.substr(i * 8, 8);
}

function formatDate(info) {
  var s = info.taken || info.sort_date;
  if (!s) return '';
  var p = s.split(/[-T:]/).map(Number);
  var date = new Date(p[0], p[1] - 1, p[2], p[3] || 0, p[4] || 0);
  if (info.date_source === 'folder') return date.toLocaleDateString(undefined, { month: 'long', year: 'numeric' });
  return date.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
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
  var lib = drives.allLibs[Math.floor(state.data.ids[state.open] / SPAN)];
  var dl = el('dl');
  var row = function (label, value) { if (value) { dl.appendChild(el('dt', '', label)); dl.appendChild(el('dd', '', value)); } };
  row('Date', formatDate(info));
  row('Drive', lib.name);
  row('Path', info.path);
  row('Size', (info.width && info.height ? info.width + ' × ' + info.height + ' · ' : '') + formatBytes(info.size));
  row('Camera', info.camera);
  panel.appendChild(dl);
  var actions = el('div', 'actions');
  var open = el('button', 'btn quiet', 'Open in ' + lib.name);
  open.title = 'Tags, faces, moving and the rest are done in the photo\'s own drive';
  open.onclick = function () { switchLibrary(lib.id, 'folder=' + info.folder_id); };
  actions.appendChild(open);
  panel.appendChild(actions);
}

function renderPanel() {
  var info = lb.details, panel = $('lb-panel');
  panel.textContent = '';
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
  var date = row('Date', formatDate(info) + (info.taken_offset ? ' (UTC' + info.taken_offset + ')' : ''));
  if (date && info.date_source !== 'file') {
    date.appendChild(el('div', 'note', info.date_source === 'folder' ? 'No capture date; month of the event folder' : 'No capture date; file modification date'));
  }
  row('Name', info.name);
  var folder = info.path.indexOf('/') >= 0 ? info.path.slice(0, info.path.lastIndexOf('/')) : '';
  if (folder) {
    var fb = el('button', '', folder);
    fb.onclick = function () { closeLightbox(); setFilter({ folder: info.folder_id, tags: [], q: '' }); };
    row('Folder', fb);
  }
  if (info.width && info.height) row('Size', info.width + ' × ' + info.height + ' · ' + formatBytes(info.size));
  else row('Size', formatBytes(info.size));
  if (info.duration_ms) row('Length', Math.round(info.duration_ms / 1000) + ' s');
  row('Camera', info.camera);
  infoTags(row, info);
  if (info.linked && info.linked.length) {
    var versions = el('div');
    info.linked.forEach(function (l) { versions.appendChild(el('div', '', l.path + (l.missing ? ' (missing)' : ''))); });
    row('Versions', versions);
  }
  infoFaces(row, info);
  panel.appendChild(dl);
  drawFaces();

  var actions = el('div', 'actions');
  infoReveal(actions, info);
  var move = el('button', 'btn quiet', 'Move…');
  move.onclick = function () { moveDialog([info.id], function () { closeLightbox(); }); };
  var trash = el('button', 'btn danger', 'Move to trash');
  trash.onclick = function () { trashDialog([info.id], function () { closeLightbox(); }); };
  actions.appendChild(move);
  actions.appendChild(trash);
  panel.appendChild(actions);
}

// Turns the original on the drive (its EXIF orientation, two bytes, no loss),
// like the rotate button of the Finder's Quick Look. `turns` is in quarter
// turns clockwise.
function rotateOpen(turns) {
  var d = state.data, i = state.open;
  if (!d || i < 0 || isAll() || d.kinds[i] !== 'j' || lb.rotating) return;
  lb.rotating = true;
  var id = d.ids[i];
  post(fileBase(id) + '/rotate', { turns: turns }).then(function (r) {
    lb.rotating = false;
    // The address of the picture carries the version, so the new one is
    // fetched; the grid follows when the status poll sees the change.
    if (state.data === d && d.ids[i] === id) {
      d.versions = d.versions.slice(0, i * 8) + r.version + d.versions.slice(i * 8 + 8);
      if (state.open === i) showItem();
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
  else drawFaces();
};
$('lb-live').onclick = function () {
  var d = state.data, id = d.ids[state.open], video = state.live[id];
  if (!video) return;
  var stage = $('stage');
  var still = stage.querySelector('img');
  var v = el('video');
  v.autoplay = true;
  v.playsInline = true;
  v.src = fileBase(video) + '/original';
  v.onended = function () { v.remove(); if (still) still.hidden = false; };
  if (still) still.hidden = true;
  stage.appendChild(v);
};

document.addEventListener('keydown', function (ev) {
  if (!$('modal').hidden) {
    if (ev.key === 'Escape') closeModal();
    return;
  }
  if ($('lightbox').hidden) {
    if (ev.key === 'Escape' && state.selecting) endSelection();
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
  if (ev.key === 'Escape' || (ev.key === ' ' && !onVideo)) {
    ev.preventDefault();
    closeLightbox();
  }
  else if (ev.key === 'ArrowLeft') step(-1);
  else if (ev.key === 'ArrowRight') step(1);
  else if (ev.key === 'i') $('lb-info').onclick();
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

function loadInfo() {
  return api(LIBAPI + '/info').then(function (info) {
    $('title').textContent = info.name;
    state.reveal = info.reveal || null;
    document.title = info.name + ' · shoebox';
    var parts = [
      info.photos.toLocaleString() + ' photos',
      info.videos.toLocaleString() + ' videos',
    ];
    if (info.missing) parts.push(info.missing.toLocaleString() + ' missing');
    $('nav-trash').textContent = info.trash ? 'Trash (' + info.trash + ')' : 'Trash';
    if (info.thumbs_done < info.thumbs_total) {
      parts.push('thumbnails ' + Math.floor(100 * info.thumbs_done / info.thumbs_total) + '%');
    }
    var scan = info.jobs.filter(function (j) { return j.kind === 'scan'; })[0];
    if (info.busy) parts.push('scan running…');
    else if (scan) parts.push('last scan ' + new Date(scan.started_at * 1000).toLocaleDateString());
    if (!info.ffmpeg && info.videos) parts.push('no ffmpeg: video previews made by the browser');
    var f = info.faces, c = info.clusters;
    $('nav-faces').hidden = !f.faces;
    if (f.running) parts.push('finding faces ' + Math.floor(100 * f.done / Math.max(f.total, 1)) + '%');
    else if (f.done && f.done < f.total) parts.push('faces: ' + (f.total - f.done).toLocaleString() + ' photos to look at');
    if (c.embedding) parts.push('learning drawn faces…');
    if (c.running && c.total) parts.push('grouping faces ' + Math.floor(100 * c.done / Math.max(c.total, 1)) + '%');
    else if (c.running || c.stale) parts.push('grouping faces…');
    facesInfo(info);
    $('status').textContent = parts.join(' · ');

    // Reload when a scan changed the index (but not while it is still busy).
    if (state.indexVersion !== null && info.index_version !== state.indexVersion && !info.busy) {
      reloadAll();
    }
    if (!info.busy) state.indexVersion = info.index_version;
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
  // The unnamed clusters and a person's faces change their cards in place
  // after each action; loading them again would lose the place and what is
  // typed in other cards.
  if (state.filter.view === 'unnamed' || state.filter.view === 'person') return;
  if (state.filter.view) loadView(state.filter.view);
  else loadTimeline(false);
}

function loadView(view) {
  if (view === 'duplicates') loadDuplicates();
  else if (view === 'faces') loadFaces();
  else if (view === 'people') loadPeoplePage();
  else if (view === 'unnamed') loadUnnamed();
  else if (view === 'person') loadPersonPage();
  else if (view === 'drives') loadDrivesPage();
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

function plural(n, one, many) { return n.toLocaleString() + ' ' + (n === 1 ? one : many); }

// A small dialog. actions: [{label, cls, onclick(close)}]; onclick returns
// false to keep the dialog open.
function openModal(title, body, actions) {
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
  $('modal').hidden = false;
  var input = b.querySelector('input:not([type=file])');
  var preferred = actions.findIndex(function (act) { return act.focus; });
  if (input) input.focus(); else if (preferred >= 0) buttons[preferred].focus();
  return buttons;
}

function closeModal() {
  $('modal').hidden = true;
  if (closeModal.onclose) { var f = closeModal.onclose; closeModal.onclose = null; f(); }
}

$('modal').addEventListener('click', function (ev) { if (ev.target === this && !importState.busy) closeModal(); });

// Shows what did not work, if anything.
function report(title, lines) {
  if (!lines || !lines.length) return;
  var ul = el('ul', 'filelist');
  lines.forEach(function (l) { ul.appendChild(el('li', '', l)); });
  openModal(title, ul, [{ label: 'OK' }]);
}

function failed(e) { openModal('That did not work', String(e.message || e), [{ label: 'OK' }]); }

// ------------------------------------------------------------------ selection

function startSelection() {
  if (isAll()) return;
  state.selecting = true;
  state.anchor = null;
  document.body.classList.add('selecting');
  updateSelbar();
}

function endSelection() {
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
  var hint = window.matchMedia('(pointer: fine)').matches ? 'Click photos to select, Shift-click for a range' : 'Tap photos to select';
  $('sel-count').textContent = n ? plural(n, 'photo', 'photos') : hint;
  Array.prototype.forEach.call(document.querySelectorAll('.sec .pick'), function (b) {
    b.textContent = state.data && allSelected(parseInt(b.dataset.start, 10), parseInt(b.dataset.end, 10)) ? 'Deselect' : 'Select all';
  });
  $('sel-move').disabled = $('sel-trash').disabled = $('sel-tag').disabled = $('sel-untag').disabled = !n;
}

$('select').onclick = function () {
  if (state.filter.view) setFilter({ folder: null, tags: [], people: [], q: '' });
  if (state.selecting) endSelection(); else startSelection();
};
$('sel-done').onclick = endSelection;
$('sel-move').onclick = function () { moveDialog(selectedIds(), endSelection); };
$('sel-trash').onclick = function () { trashDialog(selectedIds(), endSelection); };
$('sel-tag').onclick = function () { addTagDialog(selectedIds()); };
$('sel-untag').onclick = function () { removeTagDialog(selectedIds()); };

// ------------------------------------------------------------------ move and trash

function moveDialog(ids, done) {
  var body = el('div');
  body.appendChild(el('p', '', 'Move ' + plural(ids.length, 'photo', 'photos') + ' into the folder:'));
  var input = el('input');
  input.type = 'text';
  input.className = 'wide';
  input.setAttribute('list', 'folder-list');
  input.placeholder = 'e.g. 2021-03 Ausflug or Familie/Weihnachten';
  var current = state.filter.folder && state.folderById[state.filter.folder];
  if (current && current.path) input.value = current.path;
  body.appendChild(input);
  var keepLabel = el('label', 'check');
  var keep = el('input');
  keep.type = 'checkbox';
  keep.checked = true;
  keepLabel.appendChild(keep);
  keepLabel.appendChild(document.createTextNode(' Keep tags'));
  keepLabel.title = 'Checked: the photos’ own tags move along. Unchecked: they are dropped. Folder tags always follow the new folder.';
  body.appendChild(keepLabel);
  body.appendChild(el('p', 'hint', 'Folders that do not exist yet are created. RAW, Live Photo and sidecar files of the same name move along. Nothing is ever overwritten.'));
  var error = el('p', 'error');
  body.appendChild(error);
  var go = function (btn) {
    var folder = input.value.trim();
    if (!folder) { error.textContent = 'Choose a folder.'; return false; }
    btn.disabled = true;
    post(LIBAPI + '/move', { ids: ids, folder: folder, keep_tags: keep.checked }).then(function (r) {
      closeModal();
      if (done) done();
      toast(plural(r.files.length, 'file', 'files') + ' moved to ' + r.folder);
      report('Some photos stayed where they were', r.skipped);
      changed();
    }).catch(function (e) { btn.disabled = false; error.textContent = e.message; });
    return false;
  };
  var buttons = openModal('Move', body, [{ label: 'Cancel', cls: 'quiet' }, { label: 'Move', onclick: go }]);
  input.addEventListener('keydown', function (ev) { if (ev.key === 'Enter') go(buttons[1]); });
}

function trashDialog(ids, done) {
  openModal(
    'Move to trash',
    'Move ' + plural(ids.length, 'photo', 'photos') + ' (with their RAW, Live Photo and sidecar files) to the shoebox trash? You can put them back from there until the trash is emptied.',
    [{ label: 'Cancel', cls: 'quiet' }, {
      label: 'Move to trash', cls: 'danger', focus: true, onclick: function (btn) {
        btn.disabled = true;
        post(LIBAPI + '/trash', { ids: ids }).then(function (r) {
          closeModal();
          if (done) done();
          toast(plural(r.files.length + r.sidecars, 'file', 'files') + ' moved to the trash');
          report('Some photos were not moved to the trash', r.skipped);
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
  body.appendChild(el('p', 'hint', 'Change the name, or the whole path to move the folder. Event folders are named “YYYY-MM Name”.'));
  var error = el('p', 'error');
  body.appendChild(error);
  var go = function (btn) {
    btn.disabled = true;
    post(LIBAPI + '/folders/' + folder.id + '/rename', { path: input.value }).then(function (r) {
      closeModal();
      toast('Now ' + r.path);
      changed();
    }).catch(function (e) { btn.disabled = false; error.textContent = e.message; });
    return false;
  };
  var buttons = openModal('Rename folder', body, [{ label: 'Cancel', cls: 'quiet' }, { label: 'Rename', onclick: go }]);
  input.addEventListener('keydown', function (ev) { if (ev.key === 'Enter') go(buttons[1]); });
}

// ------------------------------------------------------------------ import

var importState = { files: [], busy: false, touched: false };

function importDialog(files) {
  if (importState.busy) return;
  importState.files = [];
  importState.touched = false;
  var now = new Date();
  var body = el('div');
  var fields = el('div', 'fields');
  var year = el('input');
  year.type = 'number';
  year.min = 1800;
  year.max = 2200;
  year.value = now.getFullYear();
  year.style.width = '6em';
  var month = el('select');
  for (var m = 1; m <= 12; m++) {
    var o = el('option', '', new Date(2000, m - 1, 1).toLocaleDateString(undefined, { month: 'long' }));
    o.value = m;
    month.appendChild(o);
  }
  month.value = now.getMonth() + 1;
  var name = el('input');
  name.type = 'text';
  name.className = 'grow';
  name.placeholder = 'Event, e.g. Ausflug';
  fields.appendChild(year);
  fields.appendChild(month);
  fields.appendChild(name);
  body.appendChild(fields);
  var target = el('p', 'hint', '');
  body.appendChild(target);

  var picker = el('label', 'picker', 'Choose photos and videos, or drop them here');
  var input = el('input');
  input.type = 'file';
  input.multiple = true;
  input.accept = 'image/*,video/*,.heic,.heif,.dng,.cr2,.cr3,.nef,.arw,.orf,.rw2,.raf';
  picker.appendChild(input);
  body.appendChild(picker);
  var list = el('ul', 'filelist');
  body.appendChild(list);
  var error = el('p', 'error');
  body.appendChild(error);

  var describe = function () {
    var y = parseInt(year.value, 10), mo = parseInt(month.value, 10);
    var folder = (y || '????') + '-' + (mo < 10 ? '0' : '') + mo + ' ' + (name.value.trim() || '…');
    target.textContent = 'Into the folder “' + folder + '”. Files keep their dates; files already in the library are skipped.';
  };
  [year, month, name].forEach(function (f) {
    f.addEventListener('input', function () { if (f !== name) importState.touched = true; describe(); });
  });
  describe();

  var addFiles = function (fileList) {
    Array.prototype.forEach.call(fileList, function (f) {
      var li = el('li');
      li.appendChild(el('span', 'fname', f.name));
      var st = el('span', 'fstate', formatBytes(f.size));
      li.appendChild(st);
      list.appendChild(li);
      importState.files.push({ file: f, state: st, done: false });
    });
    // The month of the oldest file, unless chosen by hand.
    if (!importState.touched && importState.files.length) {
      var oldest = Math.min.apply(null, importState.files.map(function (x) { return x.file.lastModified || Date.now(); }));
      var d = new Date(oldest);
      year.value = d.getFullYear();
      month.value = d.getMonth() + 1;
      describe();
    }
    buttons[1].textContent = 'Import ' + plural(importState.files.length, 'file', 'files');
  };
  input.addEventListener('change', function () { addFiles(input.files); input.value = ''; });
  importState.add = addFiles;

  var go = function (btn) {
    error.textContent = '';
    var pending = importState.files.filter(function (x) { return !x.done; });
    if (!pending.length) { error.textContent = 'Choose some files first.'; return false; }
    post(LIBAPI + '/import/folder', { year: parseInt(year.value, 10), month: parseInt(month.value, 10), name: name.value })
      .then(function (r) { runImport(r.folder, pending, btn, buttons[0]); })
      .catch(function (e) { error.textContent = e.message; });
    return false;
  };
  var buttons = openModal('Import photos', body, [
    { label: 'Close', cls: 'quiet', onclick: function () { return !importState.busy; } },
    { label: 'Import', onclick: go },
  ]);
  closeModal.onclose = function () { importState.add = null; };
  if (files && files.length) addFiles(files);
  name.focus();
}

// One file after the other, as the raw request body.
function runImport(folder, pending, btn, closeBtn) {
  importState.busy = true;
  btn.disabled = closeBtn.disabled = true;
  var counts = { imported: 0, duplicate: 0, failed: 0 };
  var next = function (k) {
    if (k >= pending.length) {
      importState.busy = false;
      closeBtn.disabled = false;
      btn.textContent = 'Done';
      btn.disabled = false;
      btn.onclick = function () { closeModal(); };
      toast(plural(counts.imported, 'file', 'files') + ' imported' +
        (counts.duplicate ? ', ' + counts.duplicate + ' already there' : '') +
        (counts.failed ? ', ' + counts.failed + ' failed' : ''));
      loadInfo();
      // Show the folder the files went to.
      loadFolders().then(function () {
        var f = state.folders.filter(function (x) { return x.path === folder; })[0];
        if (f && counts.imported) setFilter({ folder: f.id, tags: [], q: '' }); else reloadAll();
      });
      return;
    }
    var item = pending[k];
    var xhr = new XMLHttpRequest();
    var url = LIBAPI + '/import' + query({ folder: folder, name: item.file.name, modified: item.file.lastModified || null });
    xhr.open('POST', url);
    xhr.setRequestHeader('X-Shoebox', '1');
    xhr.setRequestHeader('Content-Type', 'application/octet-stream');
    xhr.upload.onprogress = function (ev) {
      if (ev.lengthComputable) item.state.textContent = Math.floor(100 * ev.loaded / ev.total) + '%';
    };
    var finish = function (ok, text) {
      item.state.textContent = text;
      item.state.className = 'fstate ' + (ok ? 'ok' : 'bad');
      next(k + 1);
    };
    xhr.onload = function () {
      var body = {};
      try { body = JSON.parse(xhr.responseText); } catch (e) { /* not JSON */ }
      if (xhr.status === 200 && body.status === 'imported') {
        item.done = true;
        counts.imported++;
        finish(true, body.path.split('/').pop() === item.file.name ? 'imported' : 'imported as ' + body.path.split('/').pop());
      } else if (xhr.status === 200) {
        item.done = true;
        counts.duplicate++;
        item.state.title = body.duplicate_of;
        finish(true, 'already in ' + body.duplicate_of.split('/').slice(0, -1).join('/'));
      } else {
        counts.failed++;
        finish(false, body.error || 'HTTP ' + xhr.status);
      }
    };
    xhr.onerror = function () { counts.failed++; finish(false, 'connection lost'); };
    xhr.send(item.file);
  };
  next(0);
}

$('import').onclick = function () { importDialog(); };

// Drag and drop anywhere opens the import dialog (or adds to it).
(function () {
  var depth = 0;
  var hasFiles = function (ev) { return ev.dataTransfer && Array.prototype.indexOf.call(ev.dataTransfer.types || [], 'Files') >= 0; };
  window.addEventListener('dragenter', function (ev) { if (!hasFiles(ev) || isAll()) return; depth++; $('dropzone').hidden = false; });
  window.addEventListener('dragleave', function () { if (--depth <= 0) { depth = 0; $('dropzone').hidden = true; } });
  window.addEventListener('dragover', function (ev) { if (hasFiles(ev)) ev.preventDefault(); });
  window.addEventListener('drop', function (ev) {
    if (!hasFiles(ev)) return;
    ev.preventDefault();
    if (isAll()) { $('dropzone').hidden = true; depth = 0; toast('Pick one drive to import into'); return; }
    depth = 0;
    $('dropzone').hidden = true;
    if (importState.add && !$('modal').hidden) importState.add(ev.dataTransfer.files);
    else importDialog(ev.dataTransfer.files);
  });
})();

// ------------------------------------------------------------------ duplicates page

var DUP_KINDS_IDS = ['identical', 'resolution', 'edited', 'similar'];
var dupState = { groups: [], shown: 0, marked: {}, seen: {}, sameFolder: null, lowerQuality: null, types: dupTypes() };
var DUP_KINDS = [
  { id: 'identical', label: 'Identical photos', hint: 'The same file, copied.' },
  { id: 'resolution', label: 'Same photo, different resolution', hint: 'The same picture in another size, quality or name (a messenger copy, say).' },
  { id: 'edited', label: 'Original and edited', hint: 'An iPhone edit (the “E” in IMG_E1234, a portrait blur, say) next to its original. Keep the one you like, or both.' },
  { id: 'similar', label: 'Similar photos', hint: 'Different shots that look alike: a series, repeated clicks, a burst.' },
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
  var tabs = el('div', 'tabs');
  [['here', 'On this drive'], ['across', 'Across drives']].forEach(function (t) {
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
  page.textContent = '';
  page.appendChild(el('h2', '', 'Duplicates'));
  addDupTabs(page);
  var sub = el('p', 'sub', 'Looking…');
  page.appendChild(sub);
  return api('/api/all/duplicates?limit=200').then(function (r) {
    if (state.filter.view !== 'duplicates') return;
    sub.textContent = r.total_groups
      ? plural(r.total_groups, 'photo exists', 'photos exist') + ' on more than one drive (' + r.compared.join(', ') + '). Move the copy you do not need to the trash of its drive.'
      : 'No photo exists on more than one drive' + (r.compared.length > 1 ? ' (' + r.compared.join(', ') + ').' : '.');
    r.excluded.forEach(function (x) {
      var b = el('div', 'banner');
      b.appendChild(document.createTextNode(x.name + ' is not compared: ' + x.reason + '. '));
      var go = el('button', 'link', 'Decide in All drives →');
      go.onclick = function () { showView('drives'); };
      b.appendChild(go);
      page.appendChild(b);
    });
    r.groups.forEach(function (g) { page.appendChild(crossGroupNode(g)); });
    if (r.total_groups > r.groups.length) page.appendChild(el('p', 'sub', 'Showing the first ' + r.groups.length + ' of ' + r.total_groups + '.'));
  }).catch(failed);
}

function crossGroupNode(g) {
  var box = el('div', 'group');
  var head = el('div', 'group-head');
  head.appendChild(el('span', 'label', 'Same content on ' + plural(new Set(g.files.map(function (f) { return f.library; })).size, 'drive', 'drives')));
  box.appendChild(head);
  var cards = el('div', 'cards');
  g.files.forEach(function (f) {
    var base = '/api/lib/' + f.library + '/files/' + f.id;
    var card = el('div', 'card');
    var a = el('a', 'thumb');
    a.href = base + (f.kind === 'video' ? '/original' : '/view') + '?v=' + f.version;
    a.target = '_blank';
    a.rel = 'noopener';
    var img = el('img');
    img.alt = '';
    img.loading = 'lazy';
    img.src = base + '/thumb?v=' + f.version;
    a.appendChild(img);
    card.appendChild(a);
    card.appendChild(el('div', 'name', f.name));
    var folder = f.path.indexOf('/') >= 0 ? f.path.slice(0, f.path.lastIndexOf('/')) : '(top level)';
    var fb = el('button', 'folder', folder);
    fb.title = 'Show this photo in ' + f.name;
    fb.onclick = function () { switchLibrary(f.library, 'q=' + encodeURIComponent(f.path.split('/').pop())); };
    card.appendChild(fb);
    card.appendChild(el('div', 'meta', f.path.split('/').pop() + ' · ' + formatBytes(g.size)));
    var trash = el('button', 'btn danger', 'Move to trash');
    trash.onclick = function () {
      openModal('Move to trash', 'Move this copy on ' + f.name + ' (' + f.path + ') to the trash of that drive? The same photo stays on the other drive' + (g.files.length > 2 ? 's' : '') + '. You can put it back from the trash until it is emptied.', [
        { label: 'Cancel', cls: 'quiet' },
        { label: 'Move to trash', cls: 'danger', onclick: function (btn) {
          btn.disabled = true;
          post('/api/lib/' + f.library + '/trash', { ids: [f.id] }).then(function (r) {
            closeModal();
            toast(plural(r.files.length + r.sidecars, 'file', 'files') + ' moved to the trash of ' + f.name);
            loadDuplicatesAcross();
          }).catch(function (e) { closeModal(); failed(e); });
          return false;
        } },
      ]);
    };
    card.appendChild(trash);
    cards.appendChild(card);
  });
  box.appendChild(cards);
  return box;
}

function loadDuplicatesHere() {
  var page = $('page');
  page.textContent = '';
  page.appendChild(el('h2', '', 'Duplicates'));
  addDupTabs(page);
  page.appendChild(el('p', 'sub', 'Looking…'));
  return Promise.all([api(LIBAPI + '/duplicates'), api(LIBAPI + '/duplicates/same-folder'), api(LIBAPI + '/duplicates/lower-quality')]).then(function (res) {
    if (state.filter.view !== 'duplicates') return;
    dupState.groups = res[0].groups;
    dupState.sameFolder = res[1];
    dupState.lowerQuality = res[2];
    dupState.shown = 0;
    // Ticks survive a reload (a scan finishing meanwhile) for copies still listed.
    var still = {};
    dupState.groups.forEach(function (g) {
      g.files.forEach(function (f) {
        if (dupState.marked[f.id]) still[f.id] = true;
        // Of every photo with several files all but the best start ticked
        // (the best quality, then the one without a copy's name), once
        // (unticking one stays unticked after a reload).
        else if (!f.pick && !dupState.seen[f.id]) still[f.id] = true;
        if (!f.pick) dupState.seen[f.id] = true;
      });
    });
    dupState.marked = still;
    renderDuplicates();
    updateDupBar();
  }).catch(failed);
}

function renderDuplicates() {
  var page = $('page');
  page.textContent = '';
  page.appendChild(el('h2', '', 'Duplicates'));
  addDupTabs(page);
  page.appendChild(el('p', 'sub', dupState.groups.length
    ? 'Tick “delete this copy” on the files to remove; of every photo the best file is left unticked and one file per group always stays. Decided groups are not shown again.'
    : 'No duplicates. Photos added since the last scan are compared once they have thumbnails.'));
  if (dupState.groups.length) page.appendChild(dupToolbar());
  var box = el('div');
  page.appendChild(box);
  var more = el('button', 'btn quiet', 'Show more');
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
  if (!groups.length && dupState.groups.length) box.appendChild(el('p', 'sub', 'No duplicates of the chosen kinds.'));
}

// Ticked files in the groups that are shown.
function markedCount() {
  var n = 0;
  visibleGroups().forEach(function (g) { g.files.forEach(function (f) { if (dupState.marked[f.id]) n++; }); });
  return n;
}

// Left: which kinds to show (a drop-down with check boxes). Right: the two
// buttons that clear without review.
function dupToolbar() {
  var bar = el('div', 'dup-top');
  var filter = el('details', 'dup-filter');
  var sum = el('summary', 'btn quiet');
  var menu = el('div', 'dup-filter-menu');
  var count = {};
  dupState.groups.forEach(function (g) { count[g.kind] = (count[g.kind] || 0) + 1; });
  var refreshLabel = function () {
    var on = DUP_KINDS.filter(function (k) { return dupState.types[k.id]; });
    sum.textContent = 'Show: ' + (on.length === DUP_KINDS.length ? 'all kinds' : on.length ? on.map(function (k) { return k.label; }).join(', ') : 'nothing') + ' ▾';
  };
  DUP_KINDS.forEach(function (k) {
    var label = el('label', 'check');
    label.title = k.hint;
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
  filter.appendChild(sum);
  filter.appendChild(menu);
  bar.appendChild(filter);
  var sf = dupState.sameFolder, lq = dupState.lowerQuality;
  var buttons = el('div', 'dup-buttons');
  var same = el('button', 'btn', 'Clear Same Folder Copies');
  same.title = sf && sf.copies
    ? 'Deletes ' + plural(sf.copies, 'file', 'files') + ': identical copies in the same folder as another copy. The best one stays, no review.'
    : 'No identical copies in the same folder.';
  same.disabled = !(sf && sf.copies);
  same.onclick = removeSameFolder;
  var low = el('button', 'btn', 'Clear Lower Quality Copies');
  low.title = lq && lq.copies
    ? 'Deletes ' + plural(lq.copies, 'file', 'files') + ': the same photo in a lower resolution or without its metadata (a messenger copy, say). The better file stays, no review.'
    : 'No lower-quality versions of a photo.';
  low.disabled = !(lq && lq.copies);
  low.onclick = removeLowerQuality;
  buttons.appendChild(same);
  buttons.appendChild(low);
  bar.appendChild(buttons);
  return bar;
}

// The bar with the delete button, shown while copies are ticked.
function updateDupBar() {
  var bar = $('dup-bar');
  if (!bar) return;
  var n = markedCount();
  bar.hidden = n === 0;
  bar.textContent = '';
  if (!n) return;
  bar.appendChild(el('span', '', plural(n, 'copy', 'copies') + ' marked'));
  var clear = el('button', 'btn quiet', 'Clear');
  clear.onclick = function () {
    Array.prototype.forEach.call(document.querySelectorAll('.copy input[type=checkbox]:checked'), function (cb) {
      cb.checked = false;
      cb.dispatchEvent(new Event('change'));
    });
  };
  bar.appendChild(clear);
  var del = el('button', 'btn danger', 'Move to trash');
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

function dupThumb(f, compare) {
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
    a.title = 'Compare the photos';
    a.onclick = function (e) { e.preventDefault(); compare(f); };
  }
  return a;
}

// "IMG_E1234" is the edited version of "IMG_1234" (iPhone).
function isEdit(f) { return /^img_e\d+/i.test(f.name); }

// The group's photos large and side by side; "Keep this one" ticks all the
// others for deletion (the dialog closes), so the choice is one click.
function compareDialog(g, start, onKeep) {
  var back = el('div', 'modal');
  var dlg = el('div', 'dialog compare');
  dlg.appendChild(el('h2', '', 'Which one do you like most?'));
  var strip = el('div', 'compare-strip');
  var close = function () { back.remove(); document.removeEventListener('keydown', onKey); };
  var onKey = function (e) { if (e.key === 'Escape') close(); };
  document.addEventListener('keydown', onKey);
  g.files.forEach(function (f) {
    var col = el('div', 'compare-col' + (f.id === start.id ? ' start' : ''));
    var img = el('img');
    img.alt = f.name;
    img.src = LIBAPI + '/files/' + f.id + (f.kind === 'video' ? '/thumb' : '/view') + '?v=' + f.version;
    col.appendChild(img);
    col.appendChild(el('div', 'name', f.name + (g.kind === 'edited' ? (isEdit(f) ? ' · edited' : ' · original') : '')));
    var meta = [];
    if (f.width && f.height) meta.push(f.width + ' × ' + f.height);
    meta.push((f.size / 1e6).toFixed(1) + ' MB');
    col.appendChild(el('div', 'meta', meta.join(' · ')));
    var keep = el('button', 'btn', 'Keep this one');
    keep.onclick = function () { close(); onKeep(f); };
    col.appendChild(keep);
    strip.appendChild(col);
  });
  dlg.appendChild(strip);
  var row = el('div', 'actions');
  var done = el('button', 'btn quiet', 'Close');
  done.onclick = close;
  row.appendChild(done);
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
  title.title = kind.hint;
  head.appendChild(title);
  var ids = g.files.map(function (f) { return f.id; });
  var decide = function (decision, label) {
    var b = el('button', 'btn quiet', label);
    b.onclick = function () {
      b.disabled = true;
      post(LIBAPI + '/duplicates/decide', { ids: ids, decision: decision }).then(function () {
        ids.forEach(function (id) { delete dupState.marked[id]; });
        box.remove();
        updateDupBar();
        toast(decision === 'linked' ? 'Kept as versions of one photo' : 'Kept as different photos');
      }).catch(function (e) { b.disabled = false; failed(e); });
    };
    head.appendChild(b);
  };
  if (g.kind === 'similar') decide('distinct', 'Different photos');
  decide('linked', g.kind === 'identical' ? 'Keep all copies' : g.kind === 'resolution' ? 'Keep all versions'
    : g.kind === 'edited' ? 'Keep both' : 'Versions of one photo');
  box.appendChild(head);
  box.appendChild(el('p', 'group-hint', g.kind === 'similar' || g.kind === 'edited'
    ? kind.hint + ' Every card is one file; click a picture to compare them large.'
    : kind.hint));

  var boxes = [];
  // At least one copy of the group stays: the last unticked box is disabled.
  var refresh = function () {
    var open = boxes.filter(function (b) { return !b.checked; });
    boxes.forEach(function (b) { b.disabled = !b.checked && open.length === 1; });
  };
  // One card per file: the check box, where it is, what it is, its tags.
  // `thumb`: the card carries its own thumbnail (similar photos).
  var cards = [];
  // "Keep this one" in the compare dialog: tick every other file, untick it.
  var keepOnly = function (keep) {
    cards.forEach(function (c) {
      c.cb.checked = c.f.id !== keep.id;
      c.cb.dispatchEvent(new Event('change'));
    });
  };
  var compare = function (f) { compareDialog(g, f, keepOnly); };
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
    label.appendChild(cb);
    label.appendChild(document.createTextNode(' delete this copy'));
    card.appendChild(label);
    card.classList.toggle('marked', cb.checked);
    var folder = f.path.indexOf('/') >= 0 ? f.path.slice(0, f.path.lastIndexOf('/')) : '(top level)';
    var fb = el('button', 'folder', folder);
    fb.title = f.path;
    fb.onclick = function () { setFilter({ folder: f.folder_id, tags: [], q: '' }); };
    card.appendChild(fb);
    if (showName) card.appendChild(el('div', 'name', f.name));
    if (g.kind === 'edited') card.appendChild(el('div', 'badge', isEdit(f) ? 'Edited' : 'Original'));
    var meta = [];
    if (f.width && f.height) meta.push(f.width + ' × ' + f.height);
    meta.push((f.size / 1e6).toFixed(1) + ' MB');
    card.appendChild(el('div', 'meta', meta.join(' · ')));
    if (f.keeper) card.appendChild(el('div', 'meta worse', 'lower quality than the best version'));
    card.appendChild(el('div', 'meta', f.taken ? formatDate({ taken: f.taken, date_source: 'file' }) : 'no capture date'));
    if (f.tags && f.tags.length) {
      var tags = el('div', 'tagline');
      f.tags.forEach(function (t) {
        var chip = el('span', 'chip' + (t.own ? ' own' : ''), (t.own ? '' : '📁 ') + t.name);
        chip.title = t.own ? 'Own tag' : 'Folder tag';
        tags.appendChild(chip);
      });
      card.appendChild(tags);
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
    refresh();
    return box;
  }

  dupRows(g).forEach(function (copies) {
    var row = el('div', 'dup-row');
    // The photo shown is the best version of the row; its cards come first.
    copies = bestFirst(copies);
    var first = copies[0];
    var left = el('div', 'dup-photo');
    left.appendChild(dupThumb(first));
    var name = el('div', 'name', first.name + ' ');
    if (first.same) {
      var same = el('span', 'same', String.fromCharCode(64 + first.same));
      same.title = 'Files with the same letter are identical';
      name.appendChild(same);
    }
    left.appendChild(name);
    row.appendChild(left);
    var cards = el('div', 'copies');
    copies.forEach(function (f) { cards.appendChild(copyCard(f, copies.length > 1 || first.name !== f.name, false)); });
    row.appendChild(cards);
    box.appendChild(row);
  });
  refresh();
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
    var done = { files: 0, tags: 0, dates: 0, skipped: [] }, conflicts = [];
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
    'Move to trash',
    'Move ' + plural(total, 'copy', 'copies') + ' to the shoebox trash? In every group at least one copy stays. The tags and the capture date of a deleted copy are added to the copy that stays (as tags you can remove again). You can put the copies back from the trash.',
    [{ label: 'Cancel', cls: 'quiet' }, { label: 'Move to trash', cls: 'danger', focus: true, onclick: run }]
  );
}

function collectRemoved(done, r) {
  done.files += r.trashed.files.length;
  done.tags += r.tags_added;
  done.dates += r.dates_set;
  (r.trashed.skipped || []).forEach(function (s) { done.skipped.push(s); });
}

function finishRemoved(done) {
  var text = plural(done.files, 'copy', 'copies') + ' moved to the trash';
  if (done.tags) text += ', ' + plural(done.tags, 'tag', 'tags') + ' carried over';
  if (done.dates) text += ', ' + plural(done.dates, 'capture date', 'capture dates') + ' taken over';
  toast(text);
  report('Some copies were not moved to the trash', done.skipped);
  dupState.marked = {};
  changed();
}

// Copies with capture dates that cannot be merged: the user picks one date
// per photo that stays.
function askDates(conflicts, done) {
  return new Promise(function (resolve, reject) {
    var body = el('div');
    body.appendChild(el('p', '', 'These photos have copies with different capture dates. Which date should the photo that stays have? (Stored in the library only; the file is not changed.)'));
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
    openModal('Capture dates differ', body, [{
      label: 'Cancel', cls: 'quiet', onclick: function () { resolve(); },
    }, {
      label: 'Move to trash', cls: 'danger', onclick: function (btn) {
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

function removeLowerQuality() {
  var lq = dupState.lowerQuality;
  openModal(
    'Remove lower-quality versions',
    plural(lq.copies, 'copy', 'copies') + ' (of ' + plural(lq.groups, 'photo', 'photos') + ') are surely the same photo as a better file: the same picture with fewer pixels or without the capture date, as after sending it by messenger. They go to the shoebox trash without review; the better file stays. Folders, tags and capture dates of the removed copies are carried over to it. Photos that merely look alike (other shots of a series) are never touched.',
    [{ label: 'Cancel', cls: 'quiet' }, {
      label: 'Move to trash', cls: 'danger', focus: true, onclick: function (btn) {
        btn.disabled = true;
        post(LIBAPI + '/duplicates/lower-quality', {}).then(function (r) {
          closeModal();
          toast(plural(r.removed, 'copy', 'copies') + ' moved to the trash, ' + plural(r.tags_added, 'tag', 'tags') + ' carried over');
          report('Some copies were not moved to the trash', r.skipped);
          changed();
        }).catch(function (e) { closeModal(); failed(e); });
        return false;
      },
    }]
  );
}

function removeSameFolder() {
  var sf = dupState.sameFolder;
  openModal(
    'Remove exact duplicates',
    plural(sf.copies, 'copy', 'copies') + ' (' + plural(sf.groups, 'photo', 'photos') + ') have identical content to another file in the same folder. They go to the shoebox trash without review; in each folder one stays: the highest resolution, and the one with the original name rather than “IMG_1 (2)” or “IMG_1 - Copy”. Similar photos and copies in other folders are not touched. Tags and capture dates are carried over to the copy that stays.',
    [{ label: 'Cancel', cls: 'quiet' }, {
      label: 'Move to trash', cls: 'danger', focus: true, onclick: function (btn) {
        btn.disabled = true;
        post(LIBAPI + '/duplicates/same-folder', {}).then(function (r) {
          closeModal();
          toast(plural(r.removed, 'copy', 'copies') + ' moved to the trash');
          report('Some copies were not moved to the trash', r.skipped);
          changed();
        }).catch(function (e) { closeModal(); failed(e); });
        return false;
      },
    }]
  );
}

// ------------------------------------------------------------------ trash page

function loadTrash() {
  return api(LIBAPI + '/trash').then(function (items) {
    if (state.filter.view !== 'trash') return;
    var page = $('page');
    page.textContent = '';
    page.appendChild(el('h2', '', 'Trash'));
    var batches = [], byBatch = {};
    items.forEach(function (it) {
      if (!byBatch[it.batch]) { byBatch[it.batch] = []; batches.push(it.batch); }
      byBatch[it.batch].push(it);
    });
    page.appendChild(el('p', 'sub', batches.length
      ? plural(batches.length, 'photo', 'photos') + ' in .shoebox/trash on the drive. Put them back, or delete them for good.'
      : 'The trash is empty.'));
    if (batches.length) {
      var bar = el('div', 'toolbar');
      var empty = el('button', 'btn danger', 'Empty trash');
      empty.onclick = function () { emptyTrash(null, batches.length); };
      bar.appendChild(empty);
      page.appendChild(bar);
    }
    var cards = el('div', 'cards');
    cards.style.flexWrap = 'wrap';
    batches.forEach(function (b) {
      var files = byBatch[b];
      var first = files.filter(function (f) { return f.kind; })[0] || files[0];
      var card = el('div', 'card');
      var thumb = el('div', 'thumb');
      var img = el('img');
      img.alt = '';
      img.src = LIBAPI + '/trash/' + first.id + '/thumb';
      img.onerror = function () { this.remove(); };
      thumb.appendChild(img);
      card.appendChild(thumb);
      card.appendChild(el('div', 'name', first.path.split('/').pop()));
      var folder = first.path.indexOf('/') >= 0 ? first.path.slice(0, first.path.lastIndexOf('/')) : '(top level)';
      card.appendChild(el('div', 'meta', folder));
      var extra = files.length > 1 ? ' · with ' + files.slice(1).map(function (f) { return f.path.split('/').pop(); }).join(', ') : '';
      card.appendChild(el('div', 'meta', 'deleted ' + new Date(first.deleted_at * 1000).toLocaleDateString() + extra));
      var restore = el('button', 'btn quiet', 'Put back');
      restore.onclick = function () {
        restore.disabled = true;
        post(LIBAPI + '/trash/' + b + '/restore').then(function () { toast('Put back ' + first.path); changed(); })
          .catch(function (e) { restore.disabled = false; failed(e); });
      };
      card.appendChild(restore);
      var del = el('button', 'btn danger', 'Delete for good');
      del.onclick = function () { emptyTrash(b, 1); };
      card.appendChild(del);
      cards.appendChild(card);
    });
    page.appendChild(cards);
  }).catch(failed);
}

function emptyTrash(batch, n) {
  openModal('Delete for good', 'Delete ' + plural(n, 'photo', 'photos') + ' (with their companion files) from the drive? This cannot be undone.', [
    { label: 'Cancel', cls: 'quiet' },
    { label: 'Delete', cls: 'danger', onclick: function () {
      post(LIBAPI + '/trash/empty', { batch: batch }).then(function (r) {
        toast(plural(r.deleted, 'file', 'files') + ' deleted');
        changed();
      }).catch(failed);
    } },
  ]);
}

// ------------------------------------------------------------------ show in Finder, copy path, context menu

// Ask the computer that runs shoebox to show the file in its file manager.
// Only offered (state.reveal) to a browser on that computer; an iPad cannot
// open a Finder window over there.
function revealFile(id) {
  post(LIBAPI + '/files/' + id + '/reveal').then(function (r) {
    toast('Shown in ' + r.app);
  }).catch(failed);
}

// Info panel, 5a: the buttons that show the file on the shoebox computer
// and copy its path.
function infoReveal(actions, info) {
  if (state.reveal) {
    var reveal = el('button', 'btn quiet', state.reveal);
    reveal.onclick = function () { revealFile(info.id); };
    actions.appendChild(reveal);
  }
  var copy = el('button', 'btn quiet', 'Copy path');
  copy.title = info.path;
  copy.onclick = function () { copyPath(info.path); };
  actions.appendChild(copy);
}

// The path within the library, as in the info panel. Works on every device.
function copyPath(path) {
  copyText(path).then(function (ok) {
    if (ok) { toast('Path copied'); return; }
    // No clipboard access: show the path, selected, for copying by hand.
    var box = el('input');
    box.readOnly = true;
    box.value = path;
    box.onfocus = function () { box.select(); };
    openModal('Path', box, [{ label: 'OK' }]);
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
  items.push({ label: 'Copy path', run: function () {
    api(LIBAPI + '/files/' + id).then(function (info) { copyPath(info.path); }).catch(failed);
  } });
  // On one person's photos (from the sidebar or as the only search term):
  // their face in this photo becomes their picture.
  var only = state.filter.people && state.filter.people.length === 1 ? state.filter.people[0] : null;
  if (only != null && state.personNames[only]) {
    items.push({ label: 'Use as ' + state.personNames[only] + '’s picture', run: function () {
      post('/api/people/' + only + '/cover', { file: id }).then(function () { toast('Picture changed'); peopleChanged(); }).catch(failed);
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
    var show = function () { closeLightbox(); setFilter({ folder: null, tags: [t.id], q: '' }); };
    if (t.source === 'folder') {
      var b = el('button', '', '📁 ' + t.name);
      b.title = 'Folder tag: comes from where the photo is';
      b.onclick = show;
      tags.appendChild(b);
      return;
    }
    var chip = el('span', 'own');
    var name = el('button', '', t.name);
    name.onclick = show;
    var x = el('button', 'x', '✕');
    x.title = 'Remove this tag';
    x.setAttribute('aria-label', 'Remove tag ' + t.name);
    x.onclick = function () {
      post(LIBAPI + '/tags/remove', { ids: [info.id], name: t.name }).then(function () { tagsChanged(info.id); }).catch(failed);
    };
    chip.appendChild(name);
    chip.appendChild(x);
    tags.appendChild(chip);
  });
  var add = el('button', 'add', '+ Tag');
  add.onclick = function () {
    var input = tagInput();
    input.onkeydown = function (ev) {
      ev.stopPropagation(); // arrows and Escape belong to the field, not the viewer
      if (ev.key === 'Escape') { input.replaceWith(add); return; }
      if (ev.key !== 'Enter' || !input.value.trim()) return;
      input.disabled = true;
      post(LIBAPI + '/tags/add', { ids: [info.id], name: input.value }).then(function () { tagsChanged(info.id); })
        .catch(function (e) { input.disabled = false; failed(e); });
    };
    add.replaceWith(input);
    input.focus();
  };
  tags.appendChild(add);
  row('Tags', tags);
}

// A text field that suggests existing tags while typing.
function tagInput() {
  var input = el('input');
  input.type = 'text';
  input.placeholder = 'Tag name';
  input.setAttribute('list', 'tag-list');
  input.setAttribute('enterkeyhint', 'done');
  input.addEventListener('input', function () { suggestTags(input.value); });
  return input;
}

// After a change: refresh the open info panel and the sidebar.
function tagsChanged(id) {
  loadOwnTags();
  // A bulk change can change what a tag filter or search shows (the open
  // viewer keeps its photos until it closes).
  if (id == null && !state.filter.view && (state.filter.tags.length || state.filter.q)) loadTimeline(false);
  if (id == null || !lb.details || lb.details.id !== id) return;
  api(LIBAPI + '/files/' + id).then(function (info) {
    if (!lb.details || lb.details.id !== id) return;
    lb.details = info;
    renderPanel();
  }).catch(function () {});
}

function addTagDialog(ids) {
  var body = el('div');
  body.appendChild(el('p', '', 'Add a tag to ' + plural(ids.length, 'photo', 'photos') + ':'));
  var input = tagInput();
  input.className = 'wide';
  body.appendChild(input);
  body.appendChild(el('p', 'hint', 'Tags are kept in shoebox’s index, never written into the photos. Names are matched ignoring case.'));
  var error = el('p', 'error');
  body.appendChild(error);
  var go = function (btn) {
    if (!input.value.trim()) { error.textContent = 'Type a tag name.'; return false; }
    btn.disabled = true;
    post(LIBAPI + '/tags/add', { ids: ids, name: input.value }).then(function (r) {
      closeModal();
      toast(r.files ? plural(r.files, 'photo', 'photos') + ' tagged “' + r.tag.name + '”' : 'All of them have “' + r.tag.name + '” already');
      tagsChanged(null);
    }).catch(function (e) { btn.disabled = false; error.textContent = e.message; });
    return false;
  };
  var buttons = openModal('Add tag', body, [{ label: 'Cancel', cls: 'quiet' }, { label: 'Add', onclick: go }]);
  input.addEventListener('keydown', function (ev) { if (ev.key === 'Enter') go(buttons[1]); });
  input.focus();
}

// Lists the own tags on the selection; folder tags cannot be removed.
function removeTagDialog(ids) {
  post(LIBAPI + '/tags/selection', { ids: ids }).then(function (tags) {
    if (!tags.length) {
      openModal('Remove tag', 'None of these photos has an own tag. Folder tags come from where a photo is; move it to change them.', [{ label: 'OK' }]);
      return;
    }
    var body = el('div');
    body.appendChild(el('p', '', 'Remove which tag from ' + plural(ids.length, 'photo', 'photos') + '?'));
    var list = el('div', 'taglist');
    tags.forEach(function (t) {
      var b = el('button', '', t.name + ' (' + t.count.toLocaleString() + ')');
      b.onclick = function () {
        b.disabled = true;
        post(LIBAPI + '/tags/remove', { ids: ids, name: t.name }).then(function (r) {
          closeModal();
          toast('“' + t.name + '” removed from ' + plural(r.files, 'photo', 'photos'));
          tagsChanged(null);
        }).catch(function (e) { closeModal(); failed(e); });
      };
      list.appendChild(b);
    });
    body.appendChild(list);
    openModal('Remove tag', body, [{ label: 'Cancel', cls: 'quiet' }]);
  }).catch(failed);
}

// Sidebar: the tags the user added, most used first.
function loadOwnTags() {
  return api(LIBAPI + '/tags' + query({ own: 1, limit: 500 })).then(function (tags) {
    var list = $('own-tags');
    list.textContent = '';
    tags.forEach(function (t) {
      state.tagNames[t.id] = t.name;
      var li = el('li');
      var r = el('div', 'row');
      r.appendChild(el('span', 'toggle', '#'));
      var name = el('button', 'name', t.name);
      name.dataset.tag = t.id;
      name.onclick = function () { setFilter({ folder: null, tags: [t.id], q: '' }); };
      r.appendChild(name);
      r.appendChild(el('span', 'count', t.count.toLocaleString()));
      li.appendChild(r);
      list.appendChild(li);
    });
    $('tags-section').hidden = !tags.length;
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
var PAGE_FACES = 300;
var FACE_SORTS = [
  ['size', false, 'Smallest first'], ['size', true, 'Largest first'],
  ['score', false, 'Lowest score first'], ['score', true, 'Highest score first'],
];
var FACE_FILTERS = [
  ['all', 'All faces'], ['small', 'Too small for clustering'], ['large', 'Large enough'], ['rotated', 'Found turned'],
  ['not_face', 'Marked “not a face”'],
];

$('nav-faces').onclick = function () { showView('faces'); };

function loadFaces() {
  var page = $('page');
  page.textContent = '';
  page.appendChild(el('h2', '', 'Face check'));
  var sub = el('p', 'sub', 'Looking…');
  page.appendChild(sub);
  page.appendChild(faceToolbar());
  var grid = el('div', 'faces');
  page.appendChild(grid);
  var more = el('button', 'btn quiet', 'Show more');
  more.hidden = true;
  more.onclick = function () { moreFaces(grid, more); };
  page.appendChild(more);
  page.appendChild(facePickBar());
  faceState.faces = [];
  faceState.picked = {};
  updateFacePick();
  api(LIBAPI + '/faces/stats').then(function (s) {
    if (state.filter.view === 'faces') sub.textContent = faceSummary(s);
  }).catch(function () {});
  return moreFaces(grid, more);
}

function faceSummary(s) {
  var parts = [plural(s.faces, 'face', 'faces') + ' in ' + plural(s.looked - s.failed, 'photo', 'photos')];
  if (s.failed) parts.push(plural(s.failed, 'photo', 'photos') + ' failed');
  if (s.looked < s.photos) parts.push((s.photos - s.looked).toLocaleString() + ' still to look at');
  parts.push(s.small.toLocaleString() + ' under ' + s.min_cluster_px + ' px (too small for clustering)');
  parts.push(s.rotated_looked ? plural(s.rotated_faces, 'face', 'faces') + ' found turned' : 'not looked at turned yet');
  if (s.not_faces) parts.push(plural(s.not_faces, 'face', 'faces') + ' marked “not a face”');
  var widths = s.widths.map(function (b) {
    var label = b.from == null ? '< ' + b.to : b.to == null ? b.from + '+' : b.from + '–' + b.to;
    return label + ' px: ' + b.count.toLocaleString();
  });
  return parts.join(' · ') + '. Widths in the copy the recognizer saw: ' + widths.join(', ') + '.';
}

function faceToolbar() {
  var bar = el('div', 'toolbar');
  var sort = el('select');
  FACE_SORTS.forEach(function (o, k) {
    var opt = el('option', '', o[2]);
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
  FACE_FILTERS.forEach(function (o) {
    var opt = el('option', '', o[1]);
    opt.value = o[0];
    opt.selected = o[0] === faceState.filter;
    filter.appendChild(opt);
  });
  filter.onchange = function () { faceState.filter = filter.value; loadFaces(); };
  var pick = el('button', 'btn quiet', 'Select');
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
  var done = el('button', 'btn quiet', 'Done');
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
  $('face-pick-count').textContent = n ? plural(n, 'face', 'faces') : 'Tap faces to select';
  var mark = $('face-pick-mark');
  mark.textContent = faceState.filter === 'not_face' ? 'It is a face' : 'Not a face';
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
    toast(undo ? plural(r.faces, 'face', 'faces') + ' back' : plural(r.faces, 'face', 'faces') + ' marked “not a face”');
  }).catch(failed);
}

function moreFaces(grid, more) {
  var f = faceState.filter;
  var q = query({
    sort: faceState.sort, desc: faceState.desc ? 'true' : null, offset: faceState.faces.length, limit: PAGE_FACES,
    max_px: f === 'small' ? faceState.minPx : null, min_px: f === 'large' ? faceState.minPx : null,
    rotated: f === 'rotated' ? 'true' : null, not_face: f === 'not_face' ? 'true' : null,
  });
  more.disabled = true;
  return api(LIBAPI + '/faces' + q).then(function (r) {
    if (state.filter.view !== 'faces') return;
    faceState.total = r.total;
    faceState.minPx = r.min_cluster_px;
    r.faces.forEach(function (face) { faceState.faces.push(face); grid.appendChild(faceCard(face, null)); });
    if (!faceState.total) grid.appendChild(el('p', 'sub', 'No faces here. `shoebox recognize` finds them.'));
    more.disabled = false;
    more.hidden = faceState.faces.length >= faceState.total;
  }).catch(failed);
}

// One crop; clicking it opens its photo (or selects it, while selecting).
// `similarity` is set in the list of nearest neighbours.
function faceCard(face, similarity) {
  var card = el('div', 'face' + (face.small ? ' small' : ''));
  card.dataset.id = face.id;
  var a = el('a', 'crop');
  a.href = '#';
  a.title = 'Open the photo';
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
  if (face.small) card.appendChild(el('span', 'tag', 'small'));
  if (face.roll) card.appendChild(el('span', 'tag', 'turned'));
  if (similarity == null) {
    var near = el('button', 'near', '≈');
    near.title = 'Most similar faces';
    near.onclick = function () { similarFaces(face); };
    card.appendChild(near);
    var undo = faceState.filter === 'not_face';
    var mark = el('button', 'mark', undo ? '↺' : '✕');
    mark.title = undo ? 'It is a face (undo “not a face”)' : 'Not a face';
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
    box.appendChild(el('p', 'hint', 'Cosine similarity of the embeddings (1 = identical), most similar first. Click a face to open its photo.'));
    var grid = el('div', 'faces');
    grid.appendChild(faceCard(face, 1));
    list.forEach(function (n) { grid.appendChild(faceCard(n, n.similarity)); });
    box.appendChild(grid);
    openModal('Most similar faces', box, [{ label: 'Close' }]);
  }).catch(failed);
}

// Open one photo in the viewer from the page, and come back to it when the
// viewer closes.
function openFacePhoto(face) {
  var saved = state.data;
  var letter = { jpeg: 'j', png: 'p', heic: 'h' }[face.kind] || 'j';
  state.data = { count: 1, ids: [face.file], kinds: letter, days: [0], versions: (face.version + '00000000').slice(0, 8), live: [] };
  lb.restore = function () { state.data = saved; };
  openLightbox(0);
}

// ------------------------------------------------------------------ faces: people, groups, unnamed (5c-3)
// The sidebar's "Faces" section, the people overview, a person's faces
// (confirmed, suggested, maybe, rejected), the unnamed clusters, groups,
// and the people in the info panel. Everything said about a face is a
// decision on the server; the browser keeps nothing. Every action has a
// visible button (no hover-only actions), so it all works by touch.

var people = { list: [], byId: {}, groups: [], open: {}, info: null, watch: null };

function fold(s) { return String(s).normalize('NFC').toLowerCase(); }

function loadPeople() {
  return Promise.all([api(LIBAPI + '/people'), api(LIBAPI + '/groups')]).then(function (r) {
    people.list = r[0];
    people.groups = r[1];
    people.byId = {};
    r[0].forEach(function (p) { people.byId[p.id] = p; state.personNames[p.id] = p.name; });
    renderFacesSection();
    renderChips();
  }).catch(function () {});
}

// Groups in their order with their people, then "No group" (only when
// someone is in it).
function groupedPeople() {
  var out = people.groups.map(function (g) { return { id: g.id, name: g.name, people: [] }; });
  var byGroup = {};
  out.forEach(function (s) { byGroup[s.id] = s; });
  var none = { id: null, name: 'No group', people: [] };
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
    // Recomputed: the suggestions and counts changed.
    loadPeople().then(function () { if (state.filter.view === 'people') loadPeoplePage(); });
    if (state.filter.view === 'person') loadPersonCounts();
  }
  if (busy) watchClusters();
  renderFacesSection();
}

// Look at the status more often while the clusters are being recomputed.
function watchClusters() {
  if (people.watch) return;
  people.watch = setTimeout(function () { people.watch = null; loadInfo(); }, 1500);
}

// After a change to people or decisions.
function peopleChanged() {
  loadPeople();
  loadInfo();
}

function cropUrl(face) {
  if (face.manual != null) return LIBAPI + '/faces/manual/' + face.manual + '/crop';
  if (face.id != null) return LIBAPI + '/faces/' + face.id + '/crop';
  return null;
}

// A round face: the person's cover, or their initials.
function avatar(p, cls) {
  var a = el('span', 'avatar' + (cls ? ' ' + cls : ''));
  if (p && (p.cover != null || p.cover_manual != null)) {
    var img = el('img');
    img.alt = '';
    img.loading = 'lazy';
    img.src = p.cover != null ? LIBAPI + '/faces/' + p.cover + '/crop' : LIBAPI + '/faces/manual/' + p.cover_manual + '/crop';
    a.appendChild(img);
  } else {
    a.textContent = p ? p.name.split(/\s+/).map(function (w) { return w.charAt(0); }).join('').slice(0, 2).toUpperCase() : '?';
  }
  return a;
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
  img.src = '/api/files/' + face.file + '/thumb?v=' + face.version;
  inner.appendChild(img);
  box.appendChild(inner);
  return box;
}

function faceImg(face, cls) {
  var a = el('span', 'avatar' + (cls ? ' ' + cls : ''));
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
  return post(LIBAPI + '/people/' + p.id + '/group', { group_id: groupId }).then(function () {
    var g = people.groups.filter(function (x) { return x.id === groupId; })[0];
    toast(p.name + (g ? ' is in “' + g.name + '” now' : ' is in no group now'));
    peopleChanged();
    if (state.filter.view === 'people') loadPeople().then(loadPeoplePage);
  }).catch(failed);
}

// ---- sidebar

$('nav-people').onclick = function () { showView('people'); };

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
    name.title = s.people.length ? 'Show or hide its people' : 'Nobody in it yet';
    var flip = function () { people.open[key] = !people.open[key]; renderFacesSection(); };
    toggle.onclick = flip;
    name.onclick = flip;
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
        var b = el('button', 'name', p.name);
        b.dataset.person = p.id;
        b.onclick = function () { showPerson(p.id); };
        prow.appendChild(b);
        prow.appendChild(el('span', 'count', p.photos.toLocaleString()));
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
  var ub = el('button', 'name', 'Unnamed' + (c ? ' (' + plural(c.clusters, 'cluster', 'clusters') + ')' : ''));
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
      : b.dataset.person ? (!f.view && f.people.indexOf(parseInt(b.dataset.person, 10)) >= 0) || (f.view === 'person' && f.id === parseInt(b.dataset.person, 10))
      : false;
    b.classList.toggle('active', on);
  });
}

// ---- a field that names someone

// A text field offering people by group while typing (the autocomplete
// that makes naming quick); picking one calls done({person_id} or {name},
// label). opts: allowNew (offer a new person of the typed name), except
// (ids to leave out), placeholder.
function personField(done, opts) {
  opts = opts || {};
  var wrap = el('div', 'pfield');
  var input = el('input');
  input.type = 'text';
  input.placeholder = opts.placeholder || 'Name';
  input.autocomplete = 'off';
  input.setAttribute('autocapitalize', 'words');
  input.setAttribute('enterkeyhint', 'done');
  var list = el('div', 'plist');
  list.setAttribute('role', 'listbox');
  list.hidden = true;
  wrap.appendChild(input);
  wrap.appendChild(list);
  var items = [], at = -1;
  var pick = function (it) {
    list.hidden = true;
    input.value = it.label;
    done(it.who, it.label);
  };
  var mark = function () {
    Array.prototype.forEach.call(list.querySelectorAll('.item'), function (b, k) { b.classList.toggle('at', k === at); });
  };
  var render = function () {
    var text = input.value.trim(), low = fold(text), exact = null;
    list.textContent = '';
    items = [];
    at = -1;
    groupedPeople().forEach(function (s) {
      var ps = s.people.filter(function (p) {
        return (!low || fold(p.name).indexOf(low) >= 0) && !(opts.except && opts.except.indexOf(p.id) >= 0);
      });
      if (!ps.length) return;
      list.appendChild(el('div', 'head', s.name));
      ps.forEach(function (p) {
        var it = { who: { person_id: p.id }, label: p.name };
        if (fold(p.name) === low) exact = it;
        var b = el('button', 'item');
        b.type = 'button';
        b.appendChild(avatar(p, 'tiny'));
        b.appendChild(el('span', '', p.name));
        b.onmousedown = function (ev) { ev.preventDefault(); };
        b.onclick = function () { pick(it); };
        items.push(it);
        list.appendChild(b);
      });
    });
    if (opts.allowNew && text && !exact) {
      var it = { who: { name: text }, label: text };
      var b = el('button', 'item new', '+ New person “' + text + '”');
      b.type = 'button';
      b.onmousedown = function (ev) { ev.preventDefault(); };
      b.onclick = function () { pick(it); };
      items.push(it);
      list.appendChild(b);
    }
    wrap.exact = exact;
    list.hidden = !items.length || document.activeElement !== input;
  };
  input.addEventListener('focus', render);
  input.addEventListener('input', render);
  input.addEventListener('blur', function () { setTimeout(function () { list.hidden = true; }, 150); });
  input.addEventListener('keydown', function (ev) {
    ev.stopPropagation(); // arrows, Space and Escape belong to the field, not the viewer
    if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
      if (!items.length) return;
      ev.preventDefault();
      list.hidden = false;
      at = ev.key === 'ArrowDown' ? (at + 1) % items.length : (at + items.length - 1) % items.length;
      mark();
    } else if (ev.key === 'Enter') {
      ev.preventDefault();
      var text = input.value.trim();
      if (at >= 0) pick(items[at]);
      else if (wrap.exact) pick(wrap.exact);
      else if (text && opts.allowNew) pick({ who: { name: text }, label: text });
    } else if (ev.key === 'Escape') {
      if (!list.hidden) { list.hidden = true; return; }
      if (opts.onEscape) opts.onEscape();
    }
  });
  wrap.input = input;
  // What is typed, as {person_id} for someone listed or {name} for a new
  // person (null if empty or not allowed).
  wrap.value = function () {
    var text = input.value.trim();
    if (!text) return null;
    if (wrap.exact) return wrap.exact.who;
    var hit = people.list.filter(function (p) { return fold(p.name) === fold(text); })[0];
    if (hit) return { person_id: hit.id };
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
    if (!input.value.trim()) { error.textContent = 'Type a name.'; return false; }
    btn.disabled = true;
    save(input.value.trim()).then(function () { closeModal(); }).catch(function (e) { btn.disabled = false; error.textContent = e.message; });
    return false;
  };
  var buttons = openModal(title, body, [{ label: 'Cancel', cls: 'quiet' }, { label: label, onclick: go }]);
  input.addEventListener('keydown', function (ev) { if (ev.key === 'Enter') go(buttons[1]); });
  input.select();
}

function renamePerson(p) {
  nameDialog('Rename', p.name, 'Rename', function (name) {
    return post(LIBAPI + '/people/' + p.id + '/rename', { name: name }).then(function (r) {
      toast('Now “' + r.name + '”');
      state.personNames[p.id] = r.name;
      peopleChanged();
      if (state.filter.view) loadView(state.filter.view); else renderChips();
    });
  });
}

// Only for someone without faces (a misspelled name); others are merged.
function deletePersonDialog(p) {
  openModal('Delete person', 'Delete “' + p.name + '”? Nobody is confirmed as them, so no photo changes.', [
    { label: 'Cancel', cls: 'quiet' },
    { label: 'Delete', cls: 'danger', onclick: function () {
      post(LIBAPI + '/people/' + p.id + '/delete').then(function () {
        toast('“' + p.name + '” deleted');
        peopleChanged();
        if (state.filter.people.indexOf(p.id) >= 0) setFilter({ people: state.filter.people.filter(function (x) { return x !== p.id; }) });
        else if (state.filter.view === 'person' && state.filter.id === p.id) showView('people');
        else if (state.filter.view) loadView(state.filter.view);
      }).catch(failed);
    } },
  ]);
}

function newGroup(then) {
  nameDialog('New group', '', 'Create', function (name) {
    return post(LIBAPI + '/groups', { name: name }).then(function (g) {
      toast('Group “' + g.name + '” created');
      return loadPeople().then(function () {
        if (then) then(g);
        else if (state.filter.view === 'people') loadPeoplePage();
      });
    });
  });
}

function moveToGroupDialog(p) {
  var body = el('div');
  body.appendChild(el('p', '', 'Put ' + p.name + ' into:'));
  var list = el('div', 'taglist');
  var choice = function (label, id, current) {
    var b = el('button', current ? 'current' : '', label + (current ? ' ✓' : ''));
    b.onclick = function () { closeModal(); if (!current) setGroup(p, id); };
    list.appendChild(b);
  };
  people.groups.forEach(function (g) { choice(g.name, g.id, p.group_id === g.id); });
  choice('No group', null, p.group_id == null);
  body.appendChild(list);
  body.appendChild(el('p', 'hint', 'A person is in one group at most. On a computer you can also drag people onto a group.'));
  openModal('Move to group', body, [
    { label: 'New group…', cls: 'quiet', onclick: function () { setTimeout(function () { newGroup(function (g) { setGroup(p, g.id); }); }, 0); } },
    { label: 'Cancel', cls: 'quiet' },
  ]);
}

function mergeDialog(p) {
  var body = el('div');
  body.appendChild(el('p', '', 'Merge ' + p.name + ' into:'));
  var into = null;
  var field = personField(function (who) { into = who.person_id; }, { except: [p.id], placeholder: 'Who is it really?' });
  body.appendChild(field);
  body.appendChild(el('p', 'hint', 'All faces of ' + p.name + ' (and the photos they are on) become that person’s; ' + p.name + ' is removed. The other person keeps their name and group.'));
  var error = el('p', 'error');
  body.appendChild(error);
  openModal('Merge', body, [{ label: 'Cancel', cls: 'quiet' }, {
    label: 'Merge', onclick: function (btn) {
      var who = field.value();
      into = into || (who && who.person_id);
      if (!into) { error.textContent = 'Choose someone from the list.'; return false; }
      btn.disabled = true;
      post(LIBAPI + '/people/' + p.id + '/merge', { into: into }).then(function (r) {
        closeModal();
        toast(p.name + ' merged into ' + r.name);
        peopleChanged();
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
    if (!people.groups.length) list.appendChild(el('p', 'hint', 'No groups yet.'));
    people.groups.forEach(function (g, k) {
      var row = el('div', 'grow-row');
      row.appendChild(el('span', 'gname', g.name));
      row.appendChild(el('span', 'gcount', plural(g.people, 'person', 'people')));
      var btn = function (label, title, run, disabled) {
        var b = el('button', 'btn quiet small', label);
        b.title = title;
        b.setAttribute('aria-label', title);
        b.disabled = !!disabled;
        b.onclick = run;
        row.appendChild(b);
      };
      var move = function (d) {
        var ids = people.groups.map(function (x) { return x.id; });
        ids.splice(k, 1);
        ids.splice(k + d, 0, g.id);
        post(LIBAPI + '/groups/reorder', { ids: ids }).then(function (gs) { people.groups = gs; render(); peopleChanged(); }).catch(failed);
      };
      btn('↑', 'Move up', function () { move(-1); }, k === 0);
      btn('↓', 'Move down', function () { move(1); }, k === people.groups.length - 1);
      btn('Rename', 'Rename', function () {
        nameDialog('Rename group', g.name, 'Rename', function (name) {
          return post(LIBAPI + '/groups/' + g.id + '/rename', { name: name }).then(function () {
            return loadPeople().then(function () { groupsDialog(); if (state.filter.view === 'people') loadPeoplePage(); });
          });
        });
      });
      btn('Delete', 'Delete', function () {
        openModal('Delete group', 'Delete “' + g.name + '”? ' + (g.people ? plural(g.people, 'person', 'people') + ' in it will be in no group; nobody is deleted.' : 'Nobody is in it.'), [
          { label: 'Cancel', cls: 'quiet', onclick: function () { setTimeout(groupsDialog, 0); } },
          { label: 'Delete', cls: 'danger', onclick: function () {
            post(LIBAPI + '/groups/' + g.id + '/delete').then(function () {
              toast('Group “' + g.name + '” deleted');
              return loadPeople().then(function () { groupsDialog(); if (state.filter.view === 'people') loadPeoplePage(); });
            }).catch(failed);
          } },
        ]);
      });
      list.appendChild(row);
    });
  };
  render();
  openModal('Groups', body, [
    { label: 'New group…', cls: 'quiet', onclick: function () { setTimeout(function () { newGroup(function () { groupsDialog(); }); }, 0); } },
    { label: 'Done', onclick: function () { if (state.filter.view === 'people') loadPeoplePage(); } },
  ]);
}

function personMenuItems(p) {
  return [
    { label: 'Show photos', run: function () { showPerson(p.id); } },
    { label: 'Review faces', run: function () { showView('person', p.id, p.suggested ? 'suggested' : p.maybe ? 'maybe' : 'confirmed'); } },
    { label: 'Rename…', run: function () { renamePerson(p); } },
    { label: 'Move to group…', run: function () { moveToGroupDialog(p); } },
    { label: 'Merge into…', run: function () { mergeDialog(p); } },
  ].concat(p.faces === 0 ? [{ label: 'Delete…', run: function () { deletePersonDialog(p); } }] : []);
}

// A ⋯ button that opens a menu below itself (works by touch).
function menuButton(items, title) {
  var b = el('button', 'more-btn', '⋯');
  b.type = 'button';
  b.title = title || 'More';
  b.setAttribute('aria-label', title || 'More');
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
  text.appendChild(el('div', 'pname', p.name));
  var g = people.groups.filter(function (x) { return x.id === p.group_id; })[0];
  text.appendChild(el('div', 'pmeta', plural(p.photos, 'photo', 'photos') + ' · ' + (g ? g.name : 'no group')));
  head.appendChild(text);
  var review = p.suggested + p.maybe;
  var faces = el('button', 'btn quiet', review ? 'Check ' + plural(review, 'face', 'faces') : 'Faces');
  faces.onclick = function () { showView('person', p.id, p.suggested ? 'suggested' : p.maybe ? 'maybe' : 'confirmed'); };
  head.appendChild(faces);
  head.appendChild(menuButton(personMenuItems(p).slice(1), 'More for ' + p.name));
  box.insertBefore(head, box.firstChild);
}

// ---- people overview

function loadPeoplePage() {
  var page = $('page');
  if (state.filter.view !== 'people') return;
  page.textContent = '';
  page.appendChild(el('h2', '', 'Faces'));
  var c = people.info && people.info.clusters;
  var sub = el('p', 'sub');
  sub.appendChild(document.createTextNode(plural(people.list.length, 'person', 'people') + (c ? ' · ' + plural(c.unnamed, 'face', 'faces') + ' without a name in ' + plural(c.clusters, 'cluster', 'clusters') + ' ' : ' ')));
  if (c && c.clusters) {
    var name = el('button', 'link', 'Name them →');
    name.onclick = function () { showView('unnamed'); };
    sub.appendChild(name);
  }
  page.appendChild(sub);
  var bar = el('div', 'toolbar');
  var ng = el('button', 'btn quiet', 'New group…');
  ng.onclick = function () { newGroup(); };
  var gs = el('button', 'btn quiet', 'Groups…');
  gs.onclick = groupsDialog;
  var check = el('button', 'btn quiet', 'Face check');
  check.onclick = function () { showView('faces'); };
  bar.appendChild(ng);
  bar.appendChild(gs);
  bar.appendChild(check);
  page.appendChild(bar);
  var sections = groupedPeople();
  if (!people.list.length) page.appendChild(el('p', 'sub', 'Nobody is named yet. Name faces under “Unnamed”, or in the info panel of a photo.'));
  sections.forEach(function (s) {
    var sec = el('section', 'pgroup');
    var h = el('h3', '', s.name);
    h.appendChild(el('span', 'n', String(s.people.length)));
    sec.appendChild(h);
    var grid = el('div', 'people');
    s.people.forEach(function (p) { grid.appendChild(personTile(p)); });
    if (!s.people.length) grid.appendChild(el('p', 'hint', 'Nobody in it yet: “Move to group…” on a person, or drag one here.'));
    sec.appendChild(grid);
    dropOnGroup(sec, s.id);
    page.appendChild(sec);
  });
  // "No group" as a drop target even when nobody is in it.
  if (people.groups.length && !sections.some(function (s) { return s.id == null; })) {
    var none = el('section', 'pgroup empty-drop');
    none.appendChild(el('h3', '', 'No group'));
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
  a.appendChild(el('span', 'pname', p.name));
  a.appendChild(el('span', 'pmeta', plural(p.photos, 'photo', 'photos')));
  t.appendChild(a);
  if (p.suggested + p.maybe) {
    var badge = el('button', 'pbadge', (p.suggested + p.maybe).toLocaleString() + ' to check');
    badge.title = 'Faces suggested for ' + p.name;
    badge.onclick = function () { showView('person', p.id, p.suggested ? 'suggested' : 'maybe'); };
    t.appendChild(badge);
  }
  t.appendChild(menuButton(personMenuItems(p), 'More for ' + p.name));
  dragPerson(t, p);
  return t;
}

// ---- a person's faces

var PERSON_TABS = [['confirmed', 'Confirmed'], ['suggested', 'Suggested'], ['maybe', 'Maybe'], ['rejected', 'Not them']];
var pp = { person: null, faces: [], total: 0, picking: false, picked: {} };

function loadPersonPage() {
  var f = state.filter, page = $('page');
  var tab = PERSON_TABS.some(function (t) { return t[0] === f.tab; }) ? f.tab : 'confirmed';
  page.textContent = '';
  pp.faces = [];
  pp.picked = {};
  pp.picking = false;
  return api(LIBAPI + '/people/' + f.id).then(function (p) {
    if (state.filter.view !== 'person' || state.filter.id !== p.id) return;
    pp.person = p;
    state.personNames[p.id] = p.name;
    var head = el('div', 'person-head');
    head.appendChild(avatar(p, 'big'));
    var text = el('div', 'ptext');
    text.appendChild(el('h2', 'pname', p.name));
    text.appendChild(el('div', 'pmeta', plural(p.photos, 'photo', 'photos') + ' · ' + plural(p.faces, 'face', 'faces') + ' confirmed'));
    head.appendChild(text);
    var photos = el('button', 'btn quiet', 'Photos');
    photos.onclick = function () { showPerson(p.id); };
    head.appendChild(photos);
    head.appendChild(menuButton(personMenuItems(p).slice(2), 'More for ' + p.name));
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
    page.appendChild(el('p', 'sub', {
      confirmed: 'Faces named ' + p.name + ', largest first. Select some to take them out (“Not ' + p.name + '”) or give them another name.',
      suggested: 'Faces that look like ' + p.name + ' (from all confirmed faces), most similar first. ✓ confirms, ✗ says it is someone else.',
      maybe: 'Faces that might be ' + p.name + ', less sure than the suggestions. ✓ confirms, ✗ says it is someone else.',
      rejected: 'Faces marked “not ' + p.name + '”. They are never suggested for ' + p.name + ' again; ↺ undoes that.',
    }[tab]));
    var bar = el('div', 'toolbar');
    var pick = el('button', 'btn quiet', 'Select');
    pick.id = 'pp-pick';
    pick.onclick = function () { pp.picking = !pp.picking; if (!pp.picking) clearPersonPicks(); updatePersonPick(); };
    bar.appendChild(pick);
    if (tab === 'suggested') {
      var all = el('button', 'btn quiet', 'Confirm all shown');
      all.onclick = function () { personAction('confirm', pp.faces.map(function (x) { return x.id; }).filter(function (x) { return x != null; })); };
      bar.appendChild(all);
    }
    page.appendChild(bar);
    var grid = el('div', 'faces');
    grid.id = 'pp-grid';
    page.appendChild(grid);
    var more = el('button', 'btn quiet', 'Show more');
    more.hidden = true;
    more.onclick = function () { morePersonFaces(grid, more, tab); };
    page.appendChild(more);
    page.appendChild(personPickBar(tab));
    updatePersonPick();
    return morePersonFaces(grid, more, tab);
  }).catch(function (e) {
    if (e.status === 404) { page.appendChild(el('p', 'sub', 'This person does not exist (any more).')); return; }
    failed(e);
  });
}

function personTabCounts(p) {
  var counts = { confirmed: p.faces, suggested: p.suggested, maybe: p.maybe };
  Array.prototype.forEach.call(document.querySelectorAll('#person-tabs .tab'), function (b) {
    var t = PERSON_TABS.filter(function (x) { return x[0] === b.dataset.tab; })[0];
    b.textContent = t[1] + (counts[t[0]] != null ? ' (' + counts[t[0]].toLocaleString() + ')' : '');
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
    if (!pp.total) grid.appendChild(el('p', 'sub', 'None.'));
    more.disabled = false;
    more.hidden = pp.faces.length >= pp.total;
  }).catch(failed);
}

function faceKey(face) { return face.id != null ? 'f' + face.id : face.manual != null ? 'm' + face.manual : null; }

function personFaceCard(face, tab) {
  var p = pp.person;
  var card = el('div', 'face' + (face.small ? ' small' : ''));
  var key = faceKey(face);
  card.dataset.key = key;
  var a = el('a', 'crop');
  a.href = '#';
  a.title = 'Open the photo';
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
    if (face.file != null) openFacePhoto(face);
  };
  card.appendChild(a);
  var caption = face.similarity != null ? Math.round(face.similarity * 100) + '% alike' : Math.round(face.px) + ' px';
  if (face.lost) caption = 'no longer found';
  else if (face.manual != null) caption = 'drawn by hand';
  card.appendChild(el('div', 'meta', caption));
  if (face.small) card.appendChild(el('span', 'tag', 'small'));
  var acts = el('div', 'acts');
  var act = function (label, title, cls, run) {
    var b = el('button', cls, label);
    b.title = title;
    b.setAttribute('aria-label', title);
    b.onclick = function (ev) { ev.stopPropagation(); run(); };
    acts.appendChild(b);
  };
  if (tab === 'suggested' || tab === 'maybe') {
    act('✓', 'It is ' + p.name, 'yes', function () { personAction('confirm', [face.id]); });
    act('✗', 'Not ' + p.name, 'no', function () { personAction('reject', [face.id]); });
  } else if (tab === 'rejected') {
    act('↺', 'Undo “not ' + p.name + '”', 'undo', function () { personAction('unreject', [face.id]); });
  } else if (!face.lost) {
    card.appendChild(menuButton(function () { return confirmedFaceMenu(face); }, 'More for this face'));
  }
  card.appendChild(acts);
  return card;
}

function confirmedFaceMenu(face) {
  var p = pp.person, items = [];
  if (face.manual != null) {
    items.push({ label: 'Name someone else…', run: function () { nameFacesDialog([face], personRefresh); } });
    items.push({ label: 'Delete this drawn face', run: function () { personAction('undo', [], [face.manual]); } });
    return items;
  }
  items.push({ label: 'Not ' + p.name, run: function () { personAction('reject', [face.id]); } });
  items.push({ label: 'Name someone else…', run: function () { nameFacesDialog([face], personRefresh); } });
  items.push({ label: 'Use as ' + p.name + '’s picture', run: function () {
    post(LIBAPI + '/people/' + p.id + '/cover', { face: face.id }).then(function () { toast('Picture changed'); peopleChanged(); }).catch(failed);
  } });
  items.push({ label: 'Not a face', run: function () { personAction('not-face', [face.id]); } });
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
    pp.faces = pp.faces.filter(function (x) {
      var k = faceKey(x);
      if (!gone[k]) return true;
      var card = document.querySelector('#pp-grid .face[data-key="' + k + '"]');
      if (card) card.remove();
      delete pp.picked[k];
      return false;
    });
    updatePersonPick();
    toast({
      confirm: plural(r.faces, 'face', 'faces') + ' confirmed as ' + p.name,
      reject: plural(r.faces, 'face', 'faces') + ': not ' + p.name,
      unreject: plural(r.faces, 'face', 'faces') + ' may be ' + p.name + ' again',
      'not-face': plural(r.faces, 'face', 'faces') + ' marked “not a face”',
      undo: 'Drawn face deleted',
    }[action]);
    peopleChanged();
    loadPersonCounts();
  }).catch(failed);
}

function personPickBar(tab) {
  var bar = el('div', 'selbar');
  bar.id = 'pp-bar';
  bar.hidden = true;
  var count = el('span');
  count.id = 'pp-count';
  bar.appendChild(count);
  var p = pp.person;
  var button = function (label, cls, run) {
    var b = el('button', 'btn ' + (cls || ''), label);
    b.className += ' pp-act';
    b.onclick = function () {
      var faces = Object.keys(pp.picked).map(function (k) { return pp.picked[k]; });
      run(faces, faces.map(function (x) { return x.id; }).filter(function (x) { return x != null; }));
    };
    bar.appendChild(b);
  };
  var all = el('button', 'btn quiet', 'All');
  all.onclick = function () {
    pp.faces.forEach(function (face) {
      var k = faceKey(face);
      if (!k) return;
      pp.picked[k] = face;
      var card = document.querySelector('#pp-grid .face[data-key="' + k + '"]');
      if (card) card.classList.add('sel');
    });
    updatePersonPick();
  };
  bar.appendChild(all);
  if (tab === 'suggested' || tab === 'maybe') {
    button('Confirm', '', function (faces, ids) { personAction('confirm', ids); });
    button('Not ' + p.name, 'quiet', function (faces, ids) { personAction('reject', ids); });
  } else if (tab === 'rejected') {
    button('Undo', '', function (faces, ids) { personAction('unreject', ids); });
  } else {
    button('Not ' + p.name, 'quiet', function (faces, ids) { personAction('reject', ids); });
    button('Name…', '', function (faces) { nameFacesDialog(faces, personRefresh); });
    button('Not a face', 'quiet', function (faces, ids) { personAction('not-face', ids); });
  }
  var done = el('button', 'btn quiet', 'Done');
  done.onclick = function () { pp.picking = false; clearPersonPicks(); updatePersonPick(); };
  bar.appendChild(done);
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
  $('pp-count').textContent = n ? plural(n, 'face', 'faces') : 'Tap faces to select';
  Array.prototype.forEach.call(bar.querySelectorAll('.pp-act'), function (b) { b.disabled = !n; });
}

// Name several faces at once (detected or drawn ones): everyone listed, or
// a new person.
function nameFacesDialog(faces, done) {
  var body = el('div');
  body.appendChild(el('p', '', 'Who is on ' + (faces.length === 1 ? 'this face' : 'these ' + faces.length + ' faces') + '?'));
  var save = function (who) {
    var ids = faces.map(function (x) { return x.id; }).filter(function (x) { return x != null; });
    var drawn = faces.filter(function (x) { return x.id == null && x.manual != null; });
    var req = ids.length ? post(LIBAPI + '/faces/assign', Object.assign({ faces: ids }, who)) : Promise.resolve({ faces: 0 });
    req.then(function (r) {
      // A drawn face nothing was detected at: drawn again with the new name.
      return Promise.all(drawn.map(function (d) {
        return post(LIBAPI + '/faces/undo', { manual: [d.manual] }).then(function () {
          return post(LIBAPI + '/faces/manual', Object.assign({ file: d.file, box: [d.x, d.y, d.w, d.h] }, who));
        });
      })).then(function () { return r; });
    }).then(function (r) {
      closeModal();
      toast(plural(faces.length, 'face', 'faces') + ' named ' + (r.person ? r.person.name : who.name || state.personNames[who.person_id] || ''));
      peopleChanged();
      if (done) done();
    }).catch(failed);
  };
  var field = personField(function (who) { save(who); }, { allowNew: true, placeholder: 'Name' });
  body.appendChild(field);
  openModal('Name', body, [{ label: 'Cancel', cls: 'quiet' }, {
    label: 'Name', onclick: function () {
      var who = field.value();
      if (!who) return false;
      save(who);
      return false;
    },
  }]);
  field.input.focus();
}

// ---- unnamed clusters

var un = { shown: 0, total: 0, unnamed: 0 };
var PAGE_CLUSTERS = 30;

function loadUnnamed() {
  var page = $('page');
  page.textContent = '';
  page.appendChild(el('h2', '', 'Unnamed'));
  var sub = el('p', 'sub', 'Looking…');
  sub.id = 'un-sub';
  page.appendChild(sub);
  var box = el('div', 'clusters');
  box.id = 'un-box';
  page.appendChild(box);
  var more = el('button', 'btn quiet', 'Show more');
  more.hidden = true;
  more.onclick = function () { moreClusters(box, more); };
  page.appendChild(more);
  un.shown = 0;
  return moreClusters(box, more);
}

function unnamedSummary() {
  var sub = $('un-sub');
  if (!sub) return;
  sub.textContent = un.total
    ? plural(un.unnamed, 'face', 'faces') + ' without a name, in ' + plural(un.total, 'cluster', 'clusters') + ' of similar faces, largest first. Name a card, or say they are strangers (Ignore) or no faces at all. “Select” names only some faces of a card.'
    : 'Every face large enough has a name (or is ignored). New photos bring new ones after `shoebox recognize`.';
}

function moreClusters(box, more) {
  more.disabled = true;
  return api(LIBAPI + '/clusters' + query({ offset: un.shown, limit: PAGE_CLUSTERS, samples: 8 })).then(function (r) {
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
  var card = el('div', 'cluster');
  card.picked = {};
  card.picking = false;
  var head = el('div', 'chead');
  var count = el('span', 'csize');
  head.appendChild(count);
  // Open a card (all faces, or Select) and it takes the whole width; this
  // button stays at the top of the screen while the faces scroll by.
  var close = el('button', 'btn small cclose', 'Close ✕');
  close.title = 'Back to the short card';
  close.hidden = true;
  head.appendChild(close);
  card.appendChild(head);
  var grid = el('div', 'cfaces');
  card.appendChild(grid);
  var faces = c.faces.slice();
  var expanded = false;
  var renderFaces = function () {
    grid.textContent = '';
    faces.forEach(function (face) {
      var f = el('a', 'cface' + (card.picked[face.id] ? ' sel' : '') + (face.small ? ' small' : ''));
      f.href = '#';
      f.title = card.picking ? 'Select' : 'Open the photo';
      f.appendChild(faceImg(face, 'big'));
      f.onclick = function (ev) {
        ev.preventDefault();
        if (card.picking) {
          var span = ev.shiftKey && pickSpan(faces, function (x) { return x.id; }, card.anchor, face.id);
          if (span) {
            span.forEach(function (x) { card.picked[x.id] = true; });
            renderFaces();
          } else {
            if (card.picked[face.id]) delete card.picked[face.id]; else card.picked[face.id] = true;
            f.classList.toggle('sel', !!card.picked[face.id]);
            card.anchor = face.id;
          }
          update();
          return;
        }
        openFacePhoto(face);
      };
      grid.appendChild(f);
    });
    if (c.size > faces.length) {
      var all = el('button', 'cmore', '+' + (c.size - faces.length).toLocaleString());
      all.title = 'Show all ' + c.size + ' faces';
      all.onclick = function () {
        api(LIBAPI + '/clusters/' + c.id + '/faces' + query({ generation: c.generation })).then(function (list) {
          faces = list;
          expanded = true;
          renderFaces();
          update();
        }).catch(function (e) { stale(e); });
      };
      grid.appendChild(all);
    }
  };
  var suggestion = el('div', 'csugg');
  card.appendChild(suggestion);
  var row = el('div', 'crow');
  var field = personField(function (who) { act('name', who); }, { allowNew: true, placeholder: 'Who is it?' });
  row.appendChild(field);
  var nameBtn = el('button', 'btn', 'Name');
  nameBtn.onclick = function () {
    var who = field.value();
    if (!who) { field.input.focus(); return; }
    act('name', who);
  };
  row.appendChild(nameBtn);
  card.appendChild(row);
  var tools = el('div', 'crow tools');
  var ignore = el('button', 'btn quiet', 'Ignore');
  ignore.title = 'Strangers: nobody needs to name them';
  ignore.onclick = function () { act('ignore', {}); };
  var notFace = el('button', 'btn quiet', 'Not a face');
  notFace.title = 'False finds: no face at all';
  notFace.onclick = function () { act('not-face', {}); };
  var pick = el('button', 'btn quiet', 'Select');
  pick.title = 'Name, ignore or mark only some faces of this card';
  // Select needs all the faces to choose from.
  var loadAll = function () {
    if (faces.length >= c.size) return Promise.resolve();
    return api(LIBAPI + '/clusters/' + c.id + '/faces' + query({ generation: c.generation })).then(function (list) {
      faces = list;
    });
  };
  pick.onclick = function () {
    card.picking = !card.picking;
    if (!card.picking) { card.picked = {}; card.anchor = null; }
    else expanded = true;
    loadAll().then(function () {
      renderFaces();
      update();
    }).catch(function (e) { stale(e); });
  };
  close.onclick = function () {
    card.picking = false;
    card.picked = {};
    expanded = false;
    faces = faces.slice(0, 8);
    renderFaces();
    update();
    card.scrollIntoView({ block: 'nearest' });
  };
  tools.appendChild(ignore);
  tools.appendChild(notFace);
  tools.appendChild(pick);
  card.appendChild(tools);

  var picked = function () { return Object.keys(card.picked).map(Number); };
  var update = function () {
    var n = picked().length;
    count.textContent = plural(c.size, 'face', 'faces') + (card.picking ? ' · ' + (n ? n + ' selected' : 'tap faces to select') : '');
    pick.classList.toggle('active', card.picking);
    card.classList.toggle('picking', card.picking);
    card.classList.toggle('open', expanded || card.picking);
    close.hidden = !(expanded || card.picking);
    var some = card.picking && n;
    nameBtn.textContent = some ? 'Name ' + n : 'Name';
    ignore.textContent = some ? 'Ignore ' + n : 'Ignore';
    notFace.textContent = some ? n + ' not a face' : 'Not a face';
    nameBtn.disabled = ignore.disabled = notFace.disabled = card.picking && !n;
    suggestion.textContent = '';
    if (c.suggestion && !card.picking) {
      var s = c.suggestion;
      suggestion.appendChild(el('span', '', 'Looks like '));
      suggestion.appendChild(el('b', '', s.person.name));
      suggestion.appendChild(el('span', '', ' (' + s.faces + ' of ' + c.size + ') '));
      var yes = el('button', 'btn small', '✓ ' + s.person.name);
      yes.title = 'Name all ' + c.size + ' faces of this card ' + s.person.name;
      yes.onclick = function () { act('name', { person_id: s.person.id }); };
      suggestion.appendChild(yes);
      if (s.faces < c.size) {
        // Only the faces that really look like them; the rest stays.
        var only = el('button', 'btn small', '✓ Only the ' + s.faces.toLocaleString());
        only.title = 'Name only the ' + s.faces + ' faces recognised as ' + s.person.name + '; the other ' + (c.size - s.faces) + ' stay unnamed';
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
      toast('These faces were regrouped in the meantime; here are the cards as they are now.');
      loadUnnamed();
    } else failed(e);
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
    post(LIBAPI + '/clusters/' + c.id + '/' + action, body).then(function (r) {
      card.classList.remove('busy');
      toast(action === 'name' ? plural(r.faces, 'face', 'faces') + ' named ' + r.person.name
        : action === 'ignore' ? plural(r.faces, 'face', 'faces') + ' ignored' : plural(r.faces, 'face', 'faces') + ' marked “not a face”');
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
      peopleChanged();
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
  if (!info.faces) box.appendChild(el('div', 'note', 'Not looked at for faces yet (shoebox recognize).'));
  else if (!faces.length) box.appendChild(el('div', 'note', 'No faces found.'));
  faces.forEach(function (f, k) { box.appendChild(infoFace(info, f, k)); });
  (info.faces_lost || []).forEach(function (f) {
    var line = el('div', 'pface lost');
    line.appendChild(el('span', 'avatar small'));
    line.appendChild(el('span', 'who', (f.person ? f.person.name : '?') + ' (face no longer found)'));
    box.appendChild(line);
  });
  var tools = el('div', 'ptools');
  if (faces.length) {
    var show = el('button', '', lb.showFaces ? 'Hide boxes' : 'Show boxes');
    show.onclick = function () { lb.showFaces = !lb.showFaces; renderPanel(); };
    tools.appendChild(show);
  }
  if (lb.details && state.data && state.data.kinds[state.open] !== 'v') {
    var add = el('button', '', '+ Add face');
    add.title = 'Draw a box around a face that was missed';
    add.onclick = startDrawing;
    tools.appendChild(add);
  }
  box.appendChild(tools);
  row('People', box);
}

function infoFace(info, f, k) {
  var line = el('div', 'pface');
  // A named person shows their picture (no crop of the face is kept); the
  // box on the photo shows which face it is.
  var pic = f.state === 'confirmed' && f.person ? avatar(people.byId[f.person.id] || { name: f.person.name }, 'small') : faceImg(f, 'small');
  pic.title = 'Show where it is';
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
    name.title = 'Photos of ' + f.person.name;
    name.onclick = function () { showPerson(f.person.id); };
    who.appendChild(name);
  } else if (f.state === 'suggested' || f.state === 'maybe') {
    who.appendChild(el('span', 'guess', (f.state === 'maybe' ? 'Maybe ' : '') + f.person.name + '?'));
    var yes = el('button', 'yes', '✓');
    yes.title = 'It is ' + f.person.name;
    yes.setAttribute('aria-label', yes.title);
    yes.onclick = function () { send('confirm', { faces: ids }); };
    var no = el('button', 'no', '✗');
    no.title = 'Not ' + f.person.name;
    no.setAttribute('aria-label', no.title);
    no.onclick = function () { send('reject', { faces: ids, person_id: f.person.id }); };
    who.appendChild(yes);
    who.appendChild(no);
  } else if (f.state === 'ignored') {
    who.appendChild(el('span', 'note', 'Stranger'));
  }
  if (f.state !== 'confirmed') {
    var add = el('button', 'add', f.state === 'suggested' || f.state === 'maybe' ? 'Other…' : '+ Name');
    add.onclick = function () {
      var field = personField(function (person) {
        send('assign', Object.assign({ faces: ids }, person));
      }, { allowNew: true, placeholder: 'Who is it?', onEscape: function () { field.replaceWith(add); } });
      add.replaceWith(field);
      field.input.focus();
    };
    who.appendChild(add);
  }
  if (f.small) who.appendChild(el('span', 'note', ' small'));
  line.appendChild(menuButton(function () {
    var items = [];
    if (f.manual != null) {
      items.push({ label: 'Name someone else…', run: function () { nameFacesDialog([Object.assign({ file: info.id }, f)], changed); } });
      items.push({ label: 'Delete this drawn face', run: function () { send('undo', { manual: [f.manual] }); } });
      return items;
    }
    if (f.state === 'confirmed') {
      items.push({ label: 'Not ' + f.person.name, run: function () { send('reject', { faces: ids, person_id: f.person.id }); } });
      items.push({ label: 'Name someone else…', run: function () { nameFacesDialog([f], changed); } });
      items.push({ label: 'Use as ' + f.person.name + '’s picture', run: function () {
        post(LIBAPI + '/people/' + f.person.id + '/cover', { face: f.id }).then(function () { toast('Picture changed'); peopleChanged(); }).catch(failed);
      } });
    }
    if (f.state !== 'ignored') items.push({ label: 'Ignore (a stranger)', run: function () { send('ignore', { faces: ids }); } });
    if (f.state) items.push({ label: 'Forget what was said', run: function () { send('undo', { faces: ids }); } });
    if (f.rejected && f.rejected.length) {
      f.rejected.forEach(function (pid) {
        var n = state.personNames[pid] || 'someone';
        items.push({ label: 'May be ' + n + ' after all', run: function () { send('unreject', { faces: ids, person_id: pid }); } });
      });
    }
    items.push({ label: 'Not a face', run: function () { send('not-face', { faces: ids }); } });
    return items;
  }, 'More for this face'));
  return line;
}

// After a change in the info panel: its faces again, and the people.
function infoFacesChanged(id) {
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
// (mouse, pen or finger), then named.
function startDrawing() {
  var stage = $('stage'), img = stage.querySelector('img');
  if (!img || !lb.details) return;
  stopDrawing();
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
  hint.appendChild(el('span', '', 'Drag a box around the face'));
  var cancel = el('button', 'btn quiet small', 'Cancel');
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
  body.appendChild(el('p', '', 'Who is it?'));
  var save = function (who) {
    post(LIBAPI + '/faces/manual', Object.assign({ file: info.id, box: frac }, who)).then(function () {
      closeModal();
      stopDrawing();
      toast('Face added');
      infoFacesChanged(info.id);
    }).catch(failed);
  };
  var field = personField(function (who) { save(who); }, { allowNew: true, placeholder: 'Name' });
  body.appendChild(field);
  body.appendChild(el('p', 'hint', 'The box is kept in shoebox’s index, never written into the photo. Its face is learned for suggestions when the recognizer finds its eyes, nose and mouth in it.'));
  openModal('Add face', body, [
    { label: 'Draw again', cls: 'quiet', onclick: function () { var r = lb.drawing && lb.drawing.layer.querySelector('.draw-box'); if (r) r.hidden = true; } },
    { label: 'Cancel', cls: 'quiet', onclick: function () { stopDrawing(); } },
    { label: 'Save', onclick: function () { var who = field.value(); if (!who) return false; save(who); return false; } },
  ]);
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
    ab.appendChild(el('span', 'name', 'All drives'));
    ab.title = 'One timeline over every drive that is not a backup';
    ab.onclick = function () { if (!isAll()) switchLibrary('all', ''); };
    all.appendChild(ab);
    ul.appendChild(all);
  }
  drives.list.forEach(function (l) {
    var li = el('li', (!isAll() && l.id === drives.current.id ? 'current ' : '') + (l.online ? '' : 'offline'));
    var b = el('button');
    b.appendChild(el('span', 'dot'));
    b.appendChild(el('span', 'name', l.name));
    if (!l.online) b.appendChild(el('span', 'tag', 'offline'));
    else if (l.role === 'backup') b.appendChild(el('span', 'tag', 'backup'));
    b.title = l.online ? l.name : l.name + ' is not connected' + (l.reason ? ' (' + l.reason + ')' : '');
    b.onclick = function () { if (isAll() || l.id !== drives.current.id) switchLibrary(l.id); };
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
  page.textContent = '';
  page.appendChild(el('h2', '', drives.current.name + ' is offline'));
  page.appendChild(el('p', 'sub', 'This drive is not connected. Plug it in and it opens by itself; nothing was changed. '
    + 'The other drives in the list on the left keep working.'));
  var overview = el('button', 'btn quiet', 'Show all drives');
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

function loadDrivesPage() {
  var page = $('page');
  page.textContent = '';
  page.appendChild(el('h2', '', 'All drives'));
  page.appendChild(el('p', 'sub', 'Each drive keeps its own library. Here they are compared, without copying anything.'));
  var drivesBox = el('div'), peopleBox = el('div');
  page.appendChild(drivesBox);
  page.appendChild(el('h2', '', 'People across drives'));
  page.appendChild(peopleBox);
  var role = function (d, value, of) {
    post('/api/all/role', { library: d.library, role: value, of: of || null }).then(function () { toast(d.name + ': ' + value); loadDrivesPage(); refreshDrives(); }).catch(failed);
  };
  api('/api/all/drives').then(function (list) {
    var shown = list.filter(function (d) { return d.shown_in_all; }).length;
    var tools = el('div', 'toolbar');
    var allBtn = el('button', 'btn', 'Photos of all drives');
    allBtn.title = 'One timeline over ' + plural(shown, 'drive', 'drives');
    allBtn.onclick = function () { switchLibrary('all', ''); };
    var dups = el('button', 'btn quiet', 'Duplicates across drives');
    dups.onclick = function () { dupTab = 'across'; showView('duplicates'); };
    tools.appendChild(allBtn);
    tools.appendChild(dups);
    drivesBox.appendChild(tools);
    list.forEach(function (d) {
      var card = el('div', 'drive-card' + (d.online ? '' : ' offline'));
      card.appendChild(el('span', 'title', d.name));
      card.appendChild(el('span', 'pill' + (d.online ? ' on' : ''), d.online ? 'online' : 'offline'));
      if (d.online && !d.shown_in_all) card.appendChild(el('span', 'pill', 'not in the common views'));
      if (d.online) {
        var wrap = el('span', 'role');
        wrap.appendChild(el('span', 'sub', 'This drive is'));
        var sel = el('select');
        [['unknown', 'not decided'], ['separate', 'separate: has its own photos'], ['backup', 'a backup of another drive']].forEach(function (o) {
          var opt = el('option', '', o[1]); opt.value = o[0]; if (d.role === o[0]) opt.selected = true; sel.appendChild(opt);
        });
        sel.onchange = function () { role(d, sel.value); };
        wrap.appendChild(sel);
        card.appendChild(wrap);
      }
      drivesBox.appendChild(card);
      if (d.online && d.role === 'backup') {
        var slot = el('div', 'backup-status');
        slot.dataset.library = d.library;
        slot.appendChild(el('p', 'sub', 'Comparing with the original drive…'));
        drivesBox.appendChild(slot);
      }
      if (d.online && d.role === 'unknown' && d.suggested_backup_of) {
        var b = el('div', 'banner warn');
        b.appendChild(el('div', '', d.name + ' looks like a backup of ' + d.suggested_backup_of + ': almost everything on it is on the other drive too. '
          + 'Until you decide, it is left out of the common timeline and the duplicates across drives.'));
        var row = el('div', 'row');
        var yes = el('button', 'btn', 'Yes, it is a backup'); yes.onclick = function () { role(d, 'backup'); };
        var no = el('button', 'btn quiet', 'No, it has its own photos'); no.onclick = function () { role(d, 'separate'); };
        row.appendChild(yes); row.appendChild(no);
        b.appendChild(row);
        drivesBox.appendChild(b);
      }
    });
  }).catch(failed);
  api('/api/all/backups').then(function (list) {
    Array.prototype.forEach.call(drivesBox.querySelectorAll('.backup-status'), function (slot) { renderBackup(slot, list.filter(function (b) { return b.library === slot.dataset.library; })[0]); });
  }).catch(failed);
  api('/api/all/people').then(function (r) {
    if (r.offline.length) peopleBox.appendChild(el('p', 'sub', 'Not included, offline: ' + r.offline.join(', ')));
    if (!r.people.length) peopleBox.appendChild(el('p', 'sub', 'Nobody is named yet.'));
    var grid = el('div', 'people-grid');
    r.people.forEach(function (p) {
      var c = el('div', 'person-merged');
      c.appendChild(el('div', 'n', p.name));
      c.appendChild(el('div', 'sub', plural(p.photos, 'photo', 'photos') + (p.group ? ' · ' + p.group : '')));
      p.libraries.forEach(function (l) { c.appendChild(el('span', 'pill', l.name + ' · ' + l.faces)); });
      var show = el('button', 'btn quiet', 'Photos on all drives');
      show.onclick = function () { switchLibrary('all', 'person=' + encodeURIComponent(p.name)); };
      c.appendChild(show);
      grid.appendChild(c);
    });
    peopleBox.appendChild(grid);
  }).catch(failed);
}

// The common timeline came in: which drives its ids refer to, and what is left out.
function allTimelineLoaded(data) {
  drives.allLibs = data.libs;
  var note = $('all-note');
  if (!note) {
    note = el('div', 'note-bar');
    note.id = 'all-note';
    $('scroller').insertBefore(note, $('filters'));
  }
  var out = data.left_out.map(function (x) { return x.name + ' (' + x.reason.split(';')[0] + ')'; });
  note.textContent = 'Photos of ' + data.libs.map(function (l) { return l.name; }).join(', ') + (out.length ? ' · not shown: ' + out.join(', ') : '');
  $('status').textContent = plural(data.count, 'photo', 'photos') + ' on ' + plural(data.libs.length, 'drive', 'drives') + ' · for looking; open a photo\'s drive to change things';
}

// How long ago a Unix time was, for "last backup".
function daysAgo(t) {
  if (!t) return 'never';
  var d = Math.floor((Date.now() / 1000 - t) / 86400);
  return d <= 0 ? 'today' : d === 1 ? '1 day ago' : d + ' days ago';
}

// A backup drive against the drive it copies, from the two indexes (nothing is
// read from the photos): what is new since the last backup, what differs, what
// only the backup has. Bit rot on the backup itself is found by "Backup check"
// with re-reading in the launcher (`shoebox backup … --deep`).
function renderBackup(slot, b) {
  slot.textContent = '';
  if (!b || !b.primary) {
    slot.appendChild(el('p', 'sub', (b && b.note) || 'No comparison possible.'));
    return;
  }
  var box = el('div', 'banner' + (b.up_to_date ? '' : ' warn'));
  box.appendChild(el('div', '', (b.up_to_date ? '✓ ' : '! ') + b.backup + ' is a backup of ' + b.primary + ': '
    + (b.up_to_date ? 'it holds all ' + plural(b.compared, 'file', 'files') + '.'
      : plural(b.missing, 'file', 'files') + ' not on it yet' + (b.different ? ', ' + b.different + ' different' : '') + '.')));
  box.appendChild(el('div', 'sub', 'Last backup: ' + daysAgo(b.backup_last_new_files) + ' (new files last arrived); the backup drive was scanned ' + daysAgo(b.backup_last_scan)
    + '. Scan it after every backup to bring this up to date.'
    + (b.unhashed ? ' ' + plural(b.unhashed, 'file', 'files') + ' of ' + b.primary + ' have no full hash yet and are not compared.' : '')
    + (b.extra ? ' ' + plural(b.extra, 'file', 'files') + ' only on the backup (gone or changed on ' + b.primary + ').' : '')));
  var lists = [['Not on the backup yet', b.missing, b.missing_files], ['Different on the backup', b.different, b.different_files], ['Only on the backup', b.extra, b.extra_files]];
  lists.forEach(function (l) {
    if (!l[1]) return;
    var det = el('details');
    det.appendChild(el('summary', '', l[0] + ' (' + l[1].toLocaleString() + ')'));
    var ul = el('ul', 'pathlist');
    l[2].forEach(function (f) { ul.appendChild(el('li', '', f.path)); });
    if (l[1] > l[2].length) ul.appendChild(el('li', 'sub', '… ' + (l[1] - l[2].length).toLocaleString() + ' more'));
    det.appendChild(ul);
    box.appendChild(det);
  });
  slot.appendChild(box);
}

function refreshDrives() {
  return api('/api/libraries').then(function (libs) {
    return api('/api/all/drives').catch(function () { return []; }).then(function (roles) {
      libs.forEach(function (l) { var r = roles.filter(function (x) { return x.library === l.id; })[0]; l.role = r ? r.role : 'unknown'; });
      drives.list = libs;
      drives.current = libs.filter(function (l) { return drives.current && l.id === drives.current.id; })[0] || chosenLibrary(libs);
      renderDriveList();
    });
  });
}

api('/api/libraries').then(function (libs) {
  drives.list = libs;
  drives.current = chosenLibrary(libs);
  LIBAPI = '/api/lib/' + drives.current.id;
  return api('/api/session');
}).then(function (s) {
  if (!s.authenticated) {
    if (!s.pin_enabled) $('login-text').textContent = 'This shoebox only accepts connections from its own computer. Restart it with --lan to allow other devices.';
    showLogin();
    return;
  }
  return refreshDrives().then(function () {
    setInterval(refreshDrives, 10000);
    if (!drives.current.online) {
      // Nothing of this library can be loaded: only the overview of the drives can be shown.
      if (readHash().view === 'drives') { state.filter = readHash(); applyFilter(); } else showOffline();
      return;
    }
    state.filter = readHash();
    $('search').value = state.filter.q;
    if (isAll()) {
      // The common timeline: no folders, tags or faces of its own; the drives answer by name.
      $('title').textContent = 'All drives';
      document.title = 'All drives · shoebox';
      applyFilter();
      return;
    }
    return Promise.all([loadFolders(), loadInfo(), loadOwnTags(), loadPeople()]).then(function () {
      applyFilter();
      setInterval(loadInfo, 20000);
    });
  });
}).catch(function (e) { console.error(e); });
