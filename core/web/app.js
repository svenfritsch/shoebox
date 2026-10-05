// shoebox web UI: virtualised timeline grid, folder tree, search, lightbox,
// and the changes: move, trash, import, folder rename, duplicates.
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
  filter: { folder: null, tags: [], q: '', view: null },
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

function api(path, opts) {
  return fetch(path, Object.assign({ credentials: 'same-origin' }, opts)).then(function (r) {
    if (r.status === 401) { showLogin(); throw new Error('login required'); }
    if (!r.ok) {
      return r.json().catch(function () { return {}; }).then(function (body) {
        throw new Error(body.error || path + ': HTTP ' + r.status);
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

function readHash() {
  var f = { folder: null, tags: [], q: '', view: null };
  location.hash.replace(/^#/, '').split('&').forEach(function (kv) {
    var i = kv.indexOf('=');
    if (i < 0) return;
    var k = kv.slice(0, i), v = decodeURIComponent(kv.slice(i + 1));
    if (k === 'folder') f.folder = parseInt(v, 10) || null;
    if (k === 'tag' && parseInt(v, 10) && f.tags.indexOf(parseInt(v, 10)) < 0) f.tags.push(parseInt(v, 10));
    if (k === 'q') f.q = v;
    if (k === 'view' && (v === 'duplicates' || v === 'trash' || v === 'faces')) f.view = v;
  });
  return f;
}

// A filter without a view shows the grid. Its terms all have to match: a
// folder, any number of tags (`tag` repeated in the URL) and free text.
function setFilter(f) {
  var h = query({ view: f.view, folder: f.folder, tag: f.tags || [], q: f.q }).replace(/^\?/, '');
  if (h === location.hash.replace(/^#/, '')) { applyFilter(); return; }
  location.hash = h; // triggers hashchange -> applyFilter
}

// The current filter with some terms changed.
function withFilter(changes) {
  var f = state.filter;
  return Object.assign({ folder: f.folder, tags: f.tags.slice(), q: f.q }, changes);
}

function showView(view) { setFilter({ view: view, folder: null, tags: [], q: '' }); }

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
  if (view) {
    endSelection();
    $('empty').hidden = true;
    $('filters').hidden = true;
    $('scroller').scrollTop = 0;
    if (view === 'duplicates') loadDuplicates(); else if (view === 'faces') loadFaces(); else loadTrash();
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
  if (f.q) add('“' + f.q + '”', function () { setFilter(withFilter({ q: '' })); });
  var terms = box.children.length;
  if (terms) {
    var more = el('button', 'chip more', '+');
    more.title = 'Add a tag or folder to the search';
    more.onclick = function () { $('search').value = ''; $('search').focus(); suggest(''); };
    box.appendChild(more);
  }
  // One click for the whole search; the ✕ on a chip drops just that term.
  if (terms >= 2) {
    var clear = el('button', 'chip clear', 'Clear all');
    clear.title = 'Show all photos again';
    clear.onclick = function () { setFilter({ folder: null, tags: [], q: '' }); };
    box.appendChild(clear);
  }
  box.hidden = !box.firstChild;
}
state.tagNames = {};

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
    if (f.tags.length) setFilter(withFilter({ tags: f.tags.slice(0, -1) }));
    else if (f.folder) setFilter(withFilter({ folder: null }));
  }
});

function suggest(text) {
  var seq = ++sugg.seq, f = state.filter;
  if (f.view) { closeSuggest(); return; }
  var needle = text.trim();
  api('/api/tags' + query({ q: needle, limit: 8, tag: f.tags, folder: f.folder })).then(function (tags) {
    if (seq !== sugg.seq) return;
    var items = tags.map(function (t) {
      state.tagNames[t.id] = t.name;
      return { kind: 'tag', id: t.id, label: t.name, count: t.count, folderTag: t.kind === 'folder' };
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

function showSuggest(items) {
  var box = $('suggest');
  box.textContent = '';
  sugg.items = items;
  sugg.at = -1;
  var last = null;
  items.forEach(function (it, i) {
    if (it.kind !== last) {
      box.appendChild(el('div', 'head', it.kind === 'tag' ? (state.filter.tags.length || state.filter.folder ? 'Tags in these photos' : 'Tags') : 'Folders'));
      last = it.kind;
    }
    var b = el('button', 'item');
    b.type = 'button';
    b.setAttribute('role', 'option');
    b.appendChild(el('span', 'label', (it.kind === 'folder' ? '📁 ' : '# ') + it.label));
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
  else setFilter(withFilter({ folder: it.id, q: '' }));
}

// Suggestions for the tag fields (info panel, "Add tag…"): every tag.
var suggestSeq = 0;
function suggestTags(text) {
  var seq = ++suggestSeq;
  if (!text.trim()) return;
  api('/api/tags' + query({ q: text.trim(), limit: 12 })).then(function (tags) {
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
  return api('/api/folders').then(function (folders) {
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

function folderNode(f, depth) {
  var li = el('li');
  var row = el('div', 'row');
  var kids = f.children.filter(function (c) { return c.count > 0; });
  var toggle = el('button', 'toggle', kids.length ? '▸' : '');
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
    toggle.textContent = open ? '▾' : '▸';
  };
  li._expand = expand;
  toggle.onclick = function () { expand(!ul || ul.hidden); };
  name.onclick = function () {
    expand(true);
    setFilter({ folder: f.id, tags: [], q: state.filter.q });
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

$('all').onclick = function () { setFilter({ folder: null, tags: [], q: '' }); };
$('nav-dups').onclick = function () { showView('duplicates'); };
$('nav-trash').onclick = function () { showView('trash'); };

$('menu').onclick = function () {
  if (window.matchMedia('(max-width: 760px)').matches) document.body.classList.toggle('side-open');
  else { document.body.classList.toggle('side-closed'); relayout(); }
};

function closeSidebarOnPhone() { document.body.classList.remove('side-open'); }

// ------------------------------------------------------------------ timeline

function loadTimeline(resetScroll) {
  var seq = ++state.loadSeq;
  var f = state.filter;
  return api('/api/timeline' + query({ folder: f.folder, tag: f.tags, q: f.q })).then(function (data) {
    if (seq !== state.loadSeq) return;
    state.anchor = null; // indices change with the new list
    var unnamed = f.tags.some(function (id) { return !state.tagNames[id]; });
    data.tags.forEach(function (t) { state.tagNames[t.id] = t.name; });
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
  return '/api/files/' + d.ids[i] + '/thumb?v=' + d.versions.substr(i * 8, 8);
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
  v.src = '/api/files/' + job.id + '/original';
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

var lb = { video: null, details: null, showFaces: false };

function openLightbox(i) {
  state.open = i;
  $('lightbox').hidden = false;
  showItem();
}

function closeLightbox() {
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
  $('lb-download').href = '/api/files/' + id + '/original?download=1';
  $('lb-live').hidden = !state.live[id];
  $('lb-title').textContent = '';

  if (kind === 'v') {
    var v = el('video');
    v.controls = true;
    v.autoplay = true;
    v.playsInline = true;
    v.preload = 'metadata';
    v.poster = frames.cache[id] || thumbUrl(i);
    v.src = '/api/files/' + id + '/original';
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

  api('/api/files/' + id).then(function (info) {
    if (state.open !== i) return;
    lb.details = info;
    $('lb-title').textContent = formatDate(info) + ' · ' + info.name;
    if (!$('lb-panel').hidden) renderPanel();
    drawFaces();
  }).catch(function () {});
}

// Boxes around the faces `shoebox recognize` found, while the info panel is
// open and "Show" is on.
function drawFaces() {
  var stage = $('stage');
  stage.querySelectorAll('.face-box').forEach(function (b) { b.remove(); });
  var info = lb.details, img = stage.querySelector('img');
  if (!lb.showFaces || $('lb-panel').hidden || !info || !info.faces || !img || img.hidden) return;
  if (info.id !== state.data.ids[state.open]) return;
  var r = img.getBoundingClientRect(), s = stage.getBoundingClientRect();
  info.faces.forEach(function (f) {
    var b = el('div', 'face-box');
    b.style.left = (r.left - s.left + f.x * r.width) + 'px';
    b.style.top = (r.top - s.top + f.y * r.height) + 'px';
    b.style.width = (f.w * r.width) + 'px';
    b.style.height = (f.h * r.height) + 'px';
    b.title = 'score ' + f.score.toFixed(2);
    stage.appendChild(b);
  });
}
window.addEventListener('resize', drawFaces);

function viewUrl(i) {
  var d = state.data;
  return '/api/files/' + d.ids[i] + '/view?v=' + d.versions.substr(i * 8, 8);
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

function renderPanel() {
  var info = lb.details, panel = $('lb-panel');
  panel.textContent = '';
  if (!info) return;
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
  if (info.faces) {
    var faces = el('div', '', info.faces.length ? info.faces.length + (info.faces.length === 1 ? ' face ' : ' faces ') : 'none found');
    if (info.faces.length) {
      var show = el('button', '', lb.showFaces ? 'Hide' : 'Show');
      show.onclick = function () { lb.showFaces = !lb.showFaces; renderPanel(); };
      faces.appendChild(show);
    }
    row('Faces', faces);
  }
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
  v.src = '/api/files/' + video + '/original';
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
  // Space closes the viewer, except on a focused video (play/pause).
  var onVideo = document.activeElement && document.activeElement.tagName === 'VIDEO';
  if (ev.key === 'Escape' || (ev.key === ' ' && !onVideo)) {
    ev.preventDefault();
    closeLightbox();
  }
  else if (ev.key === 'ArrowLeft') step(-1);
  else if (ev.key === 'ArrowRight') step(1);
  else if (ev.key === 'i') $('lb-info').onclick();
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
    if (x0 === null || ev.target.closest('.lb-panel, video')) return;
    var dx = ev.changedTouches[0].clientX - x0, dy = ev.changedTouches[0].clientY - y0;
    x0 = null;
    if (Math.abs(dx) > 50 && Math.abs(dx) > 1.5 * Math.abs(dy)) step(dx < 0 ? 1 : -1);
    else if (dy > 90 && Math.abs(dy) > 2 * Math.abs(dx)) closeLightbox();
  });
})();

// ------------------------------------------------------------------ status

function loadInfo() {
  return api('/api/info').then(function (info) {
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
    var f = info.faces;
    $('nav-faces').hidden = !f.faces;
    if (f.running) parts.push('finding faces ' + Math.floor(100 * f.done / Math.max(f.total, 1)) + '%');
    else if (f.done && f.done < f.total) parts.push('faces: ' + (f.total - f.done).toLocaleString() + ' photos to look at');
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
  loadFolders();
  loadOwnTags();
  if (state.filter.view === 'duplicates') loadDuplicates();
  else if (state.filter.view === 'trash') loadTrash();
  else if (state.filter.view === 'faces') loadFaces();
  else loadTimeline(false);
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
  if (input) input.focus();
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
  if (state.filter.view) setFilter({ folder: null, tags: [], q: '' });
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
  body.appendChild(el('p', 'hint', 'Folders that do not exist yet are created. RAW, Live Photo and sidecar files of the same name move along. Nothing is ever overwritten.'));
  var error = el('p', 'error');
  body.appendChild(error);
  var go = function (btn) {
    var folder = input.value.trim();
    if (!folder) { error.textContent = 'Choose a folder.'; return false; }
    btn.disabled = true;
    post('/api/move', { ids: ids, folder: folder }).then(function (r) {
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
      label: 'Move to trash', cls: 'danger', onclick: function (btn) {
        btn.disabled = true;
        post('/api/trash', { ids: ids }).then(function (r) {
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
    post('/api/folders/' + folder.id + '/rename', { path: input.value }).then(function (r) {
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
    post('/api/import/folder', { year: parseInt(year.value, 10), month: parseInt(month.value, 10), name: name.value })
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
    var url = '/api/import' + query({ folder: folder, name: item.file.name, modified: item.file.lastModified || null });
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
  window.addEventListener('dragenter', function (ev) { if (!hasFiles(ev)) return; depth++; $('dropzone').hidden = false; });
  window.addEventListener('dragleave', function () { if (--depth <= 0) { depth = 0; $('dropzone').hidden = true; } });
  window.addEventListener('dragover', function (ev) { if (hasFiles(ev)) ev.preventDefault(); });
  window.addEventListener('drop', function (ev) {
    if (!hasFiles(ev)) return;
    ev.preventDefault();
    depth = 0;
    $('dropzone').hidden = true;
    if (importState.add && !$('modal').hidden) importState.add(ev.dataTransfer.files);
    else importDialog(ev.dataTransfer.files);
  });
})();

// ------------------------------------------------------------------ duplicates page

var dupState = { groups: [], shown: 0 };
var PAGE_GROUPS = 30;

function loadDuplicates() {
  var page = $('page');
  page.textContent = '';
  page.appendChild(el('h2', '', 'Duplicates'));
  page.appendChild(el('p', 'sub', 'Looking…'));
  return api('/api/duplicates').then(function (r) {
    if (state.filter.view !== 'duplicates') return;
    dupState.groups = r.groups;
    dupState.shown = 0;
    renderDuplicates();
  }).catch(failed);
}

function renderDuplicates() {
  var page = $('page');
  page.textContent = '';
  page.appendChild(el('h2', '', 'Duplicates'));
  var exact = dupState.groups.filter(function (g) { return g.exact; }).length;
  var near = dupState.groups.length - exact;
  page.appendChild(el('p', 'sub', dupState.groups.length
    ? plural(exact, 'group', 'groups') + ' of identical copies, ' + plural(near, 'group', 'groups') + ' of similar photos. Decide per group; decided groups are not shown again.'
    : 'No duplicates. Photos added since the last scan are compared once they have thumbnails.'));
  var box = el('div');
  page.appendChild(box);
  var more = el('button', 'btn quiet', 'Show more');
  more.onclick = function () { showGroups(box, more); };
  page.appendChild(more);
  showGroups(box, more);
}

function showGroups(box, more) {
  var end = Math.min(dupState.groups.length, dupState.shown + PAGE_GROUPS);
  for (var k = dupState.shown; k < end; k++) box.appendChild(groupNode(dupState.groups[k]));
  dupState.shown = end;
  more.hidden = end >= dupState.groups.length;
}

function groupNode(g) {
  var box = el('div', 'group');
  var head = el('div', 'group-head');
  head.appendChild(el('span', 'label', g.exact ? 'Identical copies' : 'Similar photos'));
  var ids = g.files.map(function (f) { return f.id; });
  var decide = function (decision, label) {
    var b = el('button', 'btn quiet', label);
    b.onclick = function () {
      b.disabled = true;
      post('/api/duplicates/decide', { ids: ids, decision: decision }).then(function () {
        box.remove();
        toast(decision === 'linked' ? 'Kept as versions of one photo' : 'Kept as different photos');
      }).catch(function (e) { b.disabled = false; failed(e); });
    };
    head.appendChild(b);
  };
  if (!g.exact) decide('distinct', 'Different photos');
  decide('linked', g.exact ? 'Keep all copies' : 'Versions of one photo');
  box.appendChild(head);

  var cards = el('div', 'cards');
  g.files.forEach(function (f) {
    var card = el('div', 'card');
    var a = el('a', 'thumb');
    a.href = '/api/files/' + f.id + (f.kind === 'video' ? '/original' : '/view') + '?v=' + f.version;
    a.target = '_blank';
    a.rel = 'noopener';
    var img = el('img');
    img.alt = '';
    img.loading = 'lazy';
    img.src = '/api/files/' + f.id + '/thumb?v=' + f.version;
    a.appendChild(img);
    card.appendChild(a);
    var name = el('div', 'name', f.name + ' ');
    if (f.same) {
      var same = el('span', 'same', String.fromCharCode(64 + f.same));
      same.title = 'Files with the same letter are identical';
      name.appendChild(same);
    }
    card.appendChild(name);
    var folder = f.path.indexOf('/') >= 0 ? f.path.slice(0, f.path.lastIndexOf('/')) : '(top level)';
    var fb = el('button', 'folder', folder);
    fb.onclick = function () { setFilter({ folder: f.folder_id, tags: [], q: '' }); };
    card.appendChild(fb);
    var meta = [];
    if (f.taken) meta.push(formatDate({ taken: f.taken, date_source: 'file' }));
    if (f.width && f.height) meta.push(f.width + ' × ' + f.height);
    meta.push(formatBytes(f.size));
    card.appendChild(el('div', 'meta', meta.join(' · ')));
    var del = el('button', 'btn danger', 'Move to trash');
    del.onclick = function () {
      trashDialog([f.id], function () {
        card.remove();
        if (cards.children.length < 2) box.remove();
      });
    };
    card.appendChild(del);
    cards.appendChild(card);
  });
  box.appendChild(cards);
  return box;
}

// ------------------------------------------------------------------ trash page

function loadTrash() {
  return api('/api/trash').then(function (items) {
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
      img.src = '/api/trash/' + first.id + '/thumb';
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
        post('/api/trash/' + b + '/restore').then(function () { toast('Put back ' + first.path); changed(); })
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
      post('/api/trash/empty', { batch: batch }).then(function (r) {
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
  post('/api/files/' + id + '/reveal').then(function (r) {
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
    api('/api/files/' + id).then(function (info) { copyPath(info.path); }).catch(failed);
  } });
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
      post('/api/tags/remove', { ids: [info.id], name: t.name }).then(function () { tagsChanged(info.id); }).catch(failed);
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
      post('/api/tags/add', { ids: [info.id], name: input.value }).then(function () { tagsChanged(info.id); })
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
  api('/api/files/' + id).then(function (info) {
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
    post('/api/tags/add', { ids: ids, name: input.value }).then(function (r) {
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
  post('/api/tags/selection', { ids: ids }).then(function (tags) {
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
        post('/api/tags/remove', { ids: ids, name: t.name }).then(function (r) {
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
  return api('/api/tags' + query({ own: 1, limit: 500 })).then(function (tags) {
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
  api('/api/faces/stats').then(function (s) {
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
  post(undo ? '/api/faces/undo' : '/api/faces/not-face', { faces: ids }).then(function (r) {
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
  return api('/api/faces' + q).then(function (r) {
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
  img.src = '/api/faces/' + face.id + '/crop';
  img.onerror = function () { card.classList.add('broken'); };
  a.appendChild(img);
  a.onclick = function (ev) {
    ev.preventDefault();
    if (faceState.picking && similarity == null) {
      if (faceState.picked[face.id]) delete faceState.picked[face.id]; else faceState.picked[face.id] = true;
      card.classList.toggle('sel', !!faceState.picked[face.id]);
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
  api('/api/faces/' + face.id + '/similar?limit=24').then(function (list) {
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

// ------------------------------------------------------------------ start

api('/api/session').then(function (s) {
  if (!s.authenticated) {
    if (!s.pin_enabled) $('login-text').textContent = 'This shoebox only accepts connections from its own computer. Restart it with --lan to allow other devices.';
    showLogin();
    return;
  }
  state.filter = readHash();
  $('search').value = state.filter.q;
  return Promise.all([loadFolders(), loadInfo(), loadOwnTags()]).then(function () {
    applyFilter();
    setInterval(loadInfo, 20000);
  });
}).catch(function (e) { console.error(e); });
