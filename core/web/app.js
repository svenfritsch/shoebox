// shoebox web UI: virtualised timeline grid, folder tree, search, lightbox.
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
  filter: { folder: null, tag: null, q: '' },
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
};

// ------------------------------------------------------------------ api

function api(path, opts) {
  return fetch(path, Object.assign({ credentials: 'same-origin' }, opts)).then(function (r) {
    if (r.status === 401) { showLogin(); throw new Error('login required'); }
    if (!r.ok) throw new Error(path + ': HTTP ' + r.status);
    return r.json();
  });
}

function query(params) {
  var parts = [];
  Object.keys(params).forEach(function (k) {
    var v = params[k];
    if (v !== null && v !== undefined && v !== '') parts.push(k + '=' + encodeURIComponent(v));
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
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ pin: $('pin').value }),
  }).then(function (r) {
    if (r.ok) { location.reload(); return; }
    return r.json().then(function (body) { $('login-error').textContent = body.error || 'Wrong PIN'; });
  }).catch(function (e) { $('login-error').textContent = String(e); });
});

// ------------------------------------------------------------------ filters (in the URL hash)

function readHash() {
  var f = { folder: null, tag: null, q: '' };
  location.hash.replace(/^#/, '').split('&').forEach(function (kv) {
    var i = kv.indexOf('=');
    if (i < 0) return;
    var k = kv.slice(0, i), v = decodeURIComponent(kv.slice(i + 1));
    if (k === 'folder' || k === 'tag') f[k] = parseInt(v, 10) || null;
    if (k === 'q') f.q = v;
  });
  return f;
}

function setFilter(f) {
  var h = query({ folder: f.folder, tag: f.tag, q: f.q }).replace(/^\?/, '');
  if (h === location.hash.replace(/^#/, '')) { applyFilter(); return; }
  location.hash = h; // triggers hashchange -> applyFilter
}

function applyFilter() {
  state.filter = readHash();
  $('search').value = state.filter.q;
  renderChips();
  markActiveFolder();
  closeSidebarOnPhone();
  loadTimeline(true);
}

window.addEventListener('hashchange', applyFilter);

function renderChips() {
  var box = $('filters');
  box.textContent = '';
  var f = state.filter;
  var add = function (label, clear) {
    var c = el('span', 'chip', label);
    var x = el('button', '', '✕');
    x.title = 'Remove filter';
    x.onclick = clear;
    c.appendChild(x);
    box.appendChild(c);
  };
  if (f.folder && state.folderById[f.folder]) {
    add('📁 ' + (state.folderById[f.folder].path || state.folderById[f.folder].name), function () {
      setFilter({ folder: null, tag: f.tag, q: f.q });
    });
  }
  if (f.tag) {
    add('# ' + (state.tagNames[f.tag] || f.tag), function () { setFilter({ folder: f.folder, tag: null, q: f.q }); });
  }
  if (f.q) add('“' + f.q + '”', function () { setFilter({ folder: f.folder, tag: f.tag, q: '' }); });
  box.hidden = !box.firstChild;
}
state.tagNames = {};

var searchTimer = null;
$('search').addEventListener('input', function () {
  clearTimeout(searchTimer);
  var v = this.value;
  searchTimer = setTimeout(function () {
    setFilter({ folder: state.filter.folder, tag: state.filter.tag, q: v.trim() });
  }, 300);
  suggestTags(v);
});

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
    setFilter({ folder: f.id, tag: null, q: state.filter.q });
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
}

$('all').onclick = function () { setFilter({ folder: null, tag: null, q: '' }); };

$('menu').onclick = function () {
  if (window.matchMedia('(max-width: 760px)').matches) document.body.classList.toggle('side-open');
  else { document.body.classList.toggle('side-closed'); relayout(); }
};

function closeSidebarOnPhone() { document.body.classList.remove('side-open'); }

// ------------------------------------------------------------------ timeline

function loadTimeline(resetScroll) {
  var seq = ++state.loadSeq;
  var f = state.filter;
  return api('/api/timeline' + query({ folder: f.folder, tag: f.tag, q: f.q })).then(function (data) {
    if (seq !== state.loadSeq) return;
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
    state.rows.push({ type: 'h', top: y, h: HEADER_H, label: monthLabel(g.month), n: g.end - g.start, month: g.month });
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
  if (!state.rows.length) return;
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
    h.style.top = row.top + 'px';
    h.style.height = row.h + 'px';
    return h;
  }
  var d = state.data;
  var e = el('div', 'row-cells');
  e.style.top = row.top + 'px';
  e.style.height = row.h + 'px';
  for (var i = row.start; i < row.end; i++) {
    var a = el('a', 'cell');
    a.style.width = state.cell + 'px';
    a.style.height = state.cell + 'px';
    a.href = '#';
    a.dataset.index = i;
    var ph = el('div', 'ph');
    var img = el('img');
    img.alt = '';
    img.decoding = 'async';
    img.onload = function () { this.classList.add('ok'); };
    img.onerror = function () { this.parentNode.parentNode.classList.add('broken'); };
    img.src = thumbUrl(i);
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

$('sizer').addEventListener('click', function (ev) {
  var a = ev.target.closest('.cell');
  if (!a) return;
  ev.preventDefault();
  openLightbox(parseInt(a.dataset.index, 10));
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

var lb = { video: null, details: null };

function openLightbox(i) {
  state.open = i;
  $('lightbox').hidden = false;
  showItem();
}

function closeLightbox() {
  stopMedia();
  $('stage').textContent = '';
  $('lightbox').hidden = true;
  // Keep the grid where the viewer left off.
  var d = state.data, i = state.open;
  state.open = -1;
  if (!d || i < 0) return;
  for (var r = 0; r < state.rows.length; r++) {
    var row = state.rows[r];
    if (row.type === 'r' && row.start <= i && i < row.end) {
      var scroller = $('scroller');
      var y = row.top + $('sizer').offsetTop;
      if (y < scroller.scrollTop || y + row.h > scroller.scrollTop + scroller.clientHeight) {
        scroller.scrollTop = y - scroller.clientHeight / 3;
      }
      break;
    }
  }
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
    v.poster = thumbUrl(i);
    v.src = '/api/files/' + id + '/original';
    stage.appendChild(v);
  } else {
    var img = el('img');
    img.alt = '';
    img.src = thumbUrl(i); // instant, sharpened when the full image arrives
    stage.appendChild(img);
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
  }).catch(function () {});
}

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
    fb.onclick = function () { closeLightbox(); setFilter({ folder: info.folder_id, tag: null, q: '' }); };
    row('Folder', fb);
  }
  if (info.width && info.height) row('Size', info.width + ' × ' + info.height + ' · ' + formatBytes(info.size));
  else row('Size', formatBytes(info.size));
  if (info.duration_ms) row('Length', Math.round(info.duration_ms / 1000) + ' s');
  row('Camera', info.camera);
  if (info.tags.length) {
    var tags = el('div', 'tags');
    info.tags.forEach(function (t) {
      state.tagNames[t.id] = t.name;
      var b = el('button', '', t.name);
      b.onclick = function () { closeLightbox(); setFilter({ folder: null, tag: t.id, q: '' }); };
      tags.appendChild(b);
    });
    row('Tags', tags);
  }
  panel.appendChild(dl);
}

$('lb-close').onclick = closeLightbox;
$('lb-prev').onclick = function () { step(-1); };
$('lb-next').onclick = function () { step(1); };
$('lb-info').onclick = function () {
  var p = $('lb-panel');
  p.hidden = !p.hidden;
  if (!p.hidden) renderPanel();
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
  if ($('lightbox').hidden) return;
  if (ev.key === 'Escape') closeLightbox();
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
    document.title = info.name + ' · shoebox';
    var parts = [
      info.photos.toLocaleString() + ' photos',
      info.videos.toLocaleString() + ' videos',
    ];
    if (info.missing) parts.push(info.missing.toLocaleString() + ' missing');
    if (info.thumbs_done < info.thumbs_total) {
      parts.push('thumbnails ' + Math.floor(100 * info.thumbs_done / info.thumbs_total) + '%');
    }
    var scan = info.jobs.filter(function (j) { return j.kind === 'scan'; })[0];
    if (info.busy) parts.push('scan running…');
    else if (scan) parts.push('last scan ' + new Date(scan.started_at * 1000).toLocaleDateString());
    if (!info.ffmpeg && info.videos) parts.push('no ffmpeg: videos without preview');
    $('status').textContent = parts.join(' · ');

    // Reload when a scan changed the index (but not while it is still busy).
    if (state.indexVersion !== null && info.index_version !== state.indexVersion && !info.busy) {
      loadFolders();
      loadTimeline(false);
    }
    if (!info.busy) state.indexVersion = info.index_version;
  }).catch(function () {});
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
  return Promise.all([loadFolders(), loadInfo()]).then(function () {
    applyFilter();
    setInterval(loadInfo, 20000);
  });
}).catch(function (e) { console.error(e); });
