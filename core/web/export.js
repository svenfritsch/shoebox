// Export (phase 13): collect photos with a tag and copy them into a new
// folder. The two settings (the collection tag and numbering) are the
// browser's, like Maps: one for every drive. Everything else is the server's
// (`/api/export/...`, only for a browser on the computer shoebox runs on).

// ------------------------------------------------------------------ settings

var COLL_KEY = 'shoebox-collection';
var NUMBER_KEY = 'shoebox-export-number';
var PARENT_KEY = 'shoebox-export-parent';

function readSetting(key, fallback) {
  try { var v = localStorage.getItem(key); return v === null ? fallback : v; } catch (e) { return fallback; }
}

function writeSetting(key, value) {
  try { localStorage.setItem(key, value); } catch (e) { /* ignore */ }
}

// The tag the bookmark button puts on a photo; empty: no button.
function collectionTag() { return readSetting(COLL_KEY, '').trim(); }

// 001_ in front of every exported name (on unless switched off).
function exportNumbered() { return readSetting(NUMBER_KEY, '1') !== '0'; }

function sameTag(a, b) { return String(a).trim().toLowerCase() === String(b).trim().toLowerCase(); }

var COLL_OFF = '<svg viewBox="0 0 24 24" width="1.1em" height="1.1em" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><path d="M6 3h12v18l-6-4.500L6 21z"/><path d="M12 8v6M9 11h6"/></svg>';
var COLL_ON = '<svg viewBox="0 0 24 24" width="1.1em" height="1.1em" aria-hidden="true" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><path d="M6 3h12v18l-6-4.500L6 21z"/><path d="M9 11.500l2.200 2.200L15.200 9" stroke="#111" fill="none"/></svg>';

function collectionSection() {
  var sec = el('section', 'settings-section');
  sec.appendChild(el('h3', '', tr('settings.coll_title')));
  var hint = el('p', 'hint');
  hint.appendChild(document.createTextNode(tr('settings.coll_hint_a') + ' '));
  var icon = el('span', 'iconbox');
  icon.innerHTML = COLL_OFF;
  hint.appendChild(icon);
  hint.appendChild(document.createTextNode(' ' + tr('settings.coll_hint_b')));
  sec.appendChild(hint);

  var row = el('div', 'coll-row');
  var icon2 = el('span', 'iconbox');
  icon2.innerHTML = COLL_OFF;
  row.appendChild(icon2);
  var input = el('input');
  input.type = 'text';
  input.value = collectionTag();
  input.placeholder = tr('settings.coll_placeholder');
  input.maxLength = 100;
  row.appendChild(input);
  var count = el('span', 'hint');
  row.appendChild(count);
  sec.appendChild(row);

  var showCount = function () {
    var name = collectionTag();
    count.textContent = '';
    if (!name || isAll()) return;
    api(LIBAPI + '/tags' + query({ own: 1, limit: 500 })).then(function (tags) {
      if (collectionTag() !== name) return;
      var t = tags.filter(function (x) { return sameTag(x.name, name); })[0];
      count.textContent = tr('settings.coll_count', { count: trn('count.photos', t ? t.count : 0) });
    }).catch(function () {});
  };
  input.onchange = function () {
    var v = input.value.trim();
    if (v && meansFav(v)) {
      input.value = collectionTag();
      toast(tr('settings.coll_fav'));
      return;
    }
    writeSetting(COLL_KEY, v);
    input.value = v;
    toast(tr(v ? 'settings.coll_saved' : 'settings.coll_off'));
    markCollectionButton();
    showCount();
  };
  showCount();
  return sec;
}

function exportSection() {
  var sec = el('section', 'settings-section');
  sec.appendChild(el('h3', '', tr('settings.export_title')));
  var label = el('label', 'check-row');
  var box = el('input');
  box.type = 'checkbox';
  box.checked = exportNumbered();
  box.onchange = function () {
    writeSetting(NUMBER_KEY, box.checked ? '1' : '0');
    toast(tr('settings.export_saved'));
  };
  label.appendChild(box);
  label.appendChild(document.createTextNode(' ' + tr('settings.export_number')));
  sec.appendChild(label);
  sec.appendChild(el('p', 'hint', tr('settings.export_number_hint')));
  return sec;
}

// ------------------------------------------------------------------ the bookmark in the photo view

var coll = { busy: false };

function openCollectionDetails() {
  var d = state.data, i = state.open;
  if (!d || i < 0 || !lb.details || lb.details.id !== d.ids[i]) return null;
  return lb.details;
}

function hasCollectionTag(info, name) {
  return (info.tags || []).some(function (t) { return t.source === 'user' && sameTag(t.name, name); });
}

// Shown while a collection is named in the settings; it tags the open photo
// with that tag and takes the tag off again.
function markCollectionButton() {
  var b = $('lb-coll'), name = collectionTag();
  b.hidden = !name || isAll();
  if (b.hidden) return;
  var info = openCollectionDetails();
  var on = !!info && hasCollectionTag(info, name);
  b.innerHTML = on ? COLL_ON : COLL_OFF;
  b.classList.toggle('on', on);
  b.disabled = !info || coll.busy;
  setTip(b, tr(on ? 'coll.remove_hint' : 'coll.add_hint', { name: name }));
  b.setAttribute('aria-label', tr(on ? 'coll.remove' : 'coll.add', { name: name }));
  b.setAttribute('aria-pressed', on ? 'true' : 'false');
}

function toggleCollection() {
  var name = collectionTag(), info = openCollectionDetails();
  if (!name || !info || coll.busy || isAll()) return;
  var on = hasCollectionTag(info, name);
  coll.busy = true;
  markCollectionButton();
  post(LIBAPI + '/tags/' + (on ? 'remove' : 'add'), { ids: [info.id], name: name }).then(function () {
    toast(tr(on ? 'coll.removed' : 'coll.added', { name: name }));
    tagsChanged(info.id);
  }).catch(function (e) {
    coll.busy = false;
    markCollectionButton();
    failed(e);
  });
}

$('lb-coll').onclick = toggleCollection;

// ------------------------------------------------------------------ the Export button next to a tag chip

// With a tag in the search: the same dialog as the selection's, for
// everything shown, with the tag's name as the folder name.
function exportChip(box, f) {
  if (!state.exportOk || isAll() || !f.tags.length || f.view) return;
  var b = el('button', 'btn small export-chip', tr('sel.export'));
  setTip(b, tr('export.tag_hint'));
  b.onclick = function () {
    var ids = state.data && state.data.ids ? Array.prototype.slice.call(state.data.ids) : [];
    var name = f.tags.map(function (id) { return state.tagNames[id] || ''; }).filter(Boolean).join('-');
    // Hidden photos (the Hidden chip is on) are not part of a collection: only a selection exports them.
    var left = ids.filter(function (id) { return state.hids && state.hids[id]; }).length;
    if (left) ids = ids.filter(function (id) { return !state.hids[id]; });
    if (!ids.length) { toast(tr('export.nothing')); return; }
    if (left) toast(trn('export.hidden_left_out', left));
    openExport(ids, name);
  };
  box.appendChild(b);
}

var exportOkShown = false;
function applyExportOk() {
  $('sel-export').hidden = !state.exportOk || isAll();
  // The chips may have been drawn before the server said this computer may export.
  if (exportOkShown !== !!state.exportOk) {
    exportOkShown = !!state.exportOk;
    if (state.filter && state.filter.tags && state.filter.tags.length) renderChips();
  }
}

// ------------------------------------------------------------------ the dialog

var ex = null;

var PLACE_NAMES = {
  downloads: 'export.place_downloads',
  desktop: 'export.place_desktop',
  documents: 'export.place_documents',
  pictures: 'export.place_pictures',
  home: 'export.place_home',
};

function pathJoin(parent, name) {
  return parent + (parent.indexOf('\\') >= 0 ? (/[\\]$/.test(parent) ? '' : '\\') : (/\/$/.test(parent) ? '' : '/')) + name;
}

function openExport(ids, name) {
  $('toast').hidden = true; // it would sit on the dialog's title
  api(LIBAPI + '/export/status').then(function (s) {
    if (s.state === 'running') { exportProgress(); return null; }
    return Promise.all([post(LIBAPI + '/export/summary', { ids: ids }), api(LIBAPI + '/export/places')]);
  }).then(function (r) {
    if (!r) return;
    var summary = r[0], places = r[1];
    if (!summary.photos && !summary.videos) { toast(tr('export.nothing')); return; }
    var saved = readSetting(PARENT_KEY, '');
    var downloads = places.places.filter(function (p) { return p.kind === 'downloads'; })[0] || places.places[0];
    ex = {
      ids: ids, name: name || '', summary: summary, places: places,
      parent: saved || (downloads ? downloads.path : ''),
      heic: true, raw: false, live: false,
      check: null, error: '', seq: 0, timer: null,
    };
    renderExport();
  }).catch(failed);
}

function exportRequest() {
  return {
    ids: ex.ids, parent: ex.parent, name: ex.name.trim(),
    options: {
      number: exportNumbered(),
      raw: ex.raw && ex.summary.raw > 0,
      live: ex.live && ex.summary.live > 0,
      heic_to_jpeg: ex.heic && ex.summary.heic > 0,
    },
  };
}

function exportCheckbox(text, checked, hint, onchange) {
  var wrap = el('div');
  var label = el('label', 'check');
  var box = el('input');
  box.type = 'checkbox';
  box.checked = checked;
  box.onchange = function () { onchange(box.checked); scheduleExportCheck(); };
  label.appendChild(box);
  label.appendChild(document.createTextNode(' ' + text));
  wrap.appendChild(label);
  wrap.appendChild(el('p', 'hint x-opt', hint));
  return wrap;
}

function renderExport() {
  var s = ex.summary;
  var body = el('div');
  var sum = el('div', 'x-sum');
  body.appendChild(sum);

  var where = el('div', 'x-row');
  where.appendChild(el('label', 'x-lbl', tr('export.save_in')));
  var path = el('div', 'x-path', ex.parent || '…');
  path.title = ex.parent;
  where.appendChild(path);
  var choose = el('button', 'btn quiet small', tr('export.choose'));
  choose.onclick = function () { exportPicker(); };
  where.appendChild(choose);
  body.appendChild(where);

  var named = el('div', 'x-row');
  named.appendChild(el('label', 'x-lbl', tr('export.folder_name')));
  var input = el('input');
  input.type = 'text';
  input.value = ex.name;
  input.placeholder = tr('export.name_placeholder');
  input.maxLength = 200;
  input.oninput = function () { ex.name = input.value; scheduleExportCheck(); };
  named.appendChild(input);
  body.appendChild(named);

  var result = el('div', 'x-result');
  body.appendChild(result);
  var note = el('div', 'x-note');
  note.hidden = true;
  body.appendChild(note);

  if (s.heic) {
    body.appendChild(exportCheckbox(tr('export.heic', { count: trn('count.photos', s.heic) }), ex.heic, tr('export.heic_hint'), function (v) { ex.heic = v; }));
  }
  if (s.raw) {
    body.appendChild(exportCheckbox(tr('export.raw', { count: trn('count.photos', s.raw) }), ex.raw, tr('export.raw_hint'), function (v) { ex.raw = v; }));
  }
  if (s.live) {
    body.appendChild(exportCheckbox(tr('export.live', { count: trn('count.photos', s.live) }), ex.live, tr('export.live_hint'), function (v) { ex.live = v; }));
  }
  if (s.videos) body.appendChild(el('p', 'hint', tr('export.videos_note', { count: trn('count.videos', s.videos) })));
  var error = el('p', 'error');
  body.appendChild(error);

  var title = tr('export.title', { what: [s.photos ? trn('count.photos', s.photos) : '', s.videos ? trn('count.videos', s.videos) : ''].filter(Boolean).join(' · ') });
  var buttons = openModal(title, body, [
    { label: tr('app.cancel'), cls: 'quiet' },
    { label: tr('export.go'), onclick: function () { exportGo(); return false; } },
  ], 'wide');
  closeModal.onclose = function () { if (ex) { clearTimeout(ex.timer); ex.seq++; } };
  ex.ui = { sum: sum, result: result, note: note, error: error, go: buttons[1], input: input };
  updateExport();
  scheduleExportCheck();
  if (!ex.name) input.focus();
}

function scheduleExportCheck() {
  if (!ex || !ex.ui) return;
  clearTimeout(ex.timer);
  ex.check = null;
  updateExport();
  var seq = ++ex.seq;
  if (!ex.name.trim() || !ex.parent) return;
  ex.timer = setTimeout(function () {
    post(LIBAPI + '/export/check', exportRequest()).then(function (c) {
      if (!ex || seq !== ex.seq) return;
      ex.check = c;
      ex.error = '';
      updateExport();
    }).catch(function (e) {
      if (!ex || seq !== ex.seq) return;
      ex.error = e.message;
      updateExport();
    });
  }, 250);
}

function updateExport() {
  var u = ex.ui, s = ex.summary, o = exportRequest().options;
  var bytes = s.bytes + (o.raw ? s.raw_bytes : 0) + (o.live ? s.live_bytes : 0);
  u.sum.textContent = '';
  if (s.photos) u.sum.appendChild(el('span', '', trn('count.photos', s.photos)));
  if (s.videos) u.sum.appendChild(el('span', '', trn('count.videos', s.videos)));
  u.sum.appendChild(el('span', '', formatBytes(bytes)));

  var name = ex.name.trim();
  u.result.textContent = '';
  u.result.appendChild(document.createTextNode(name && ex.parent ? tr('export.creates', { path: pathJoin(ex.parent, name) }) : tr('export.creates_none')));
  // Only said when numbering is on; with it off there is nothing to point out.
  if (exportNumbered()) {
    u.result.appendChild(el('br'));
    u.result.appendChild(document.createTextNode(tr('export.numbered', { example: '001_IMG_3457.jpg' })));
  }

  var c = ex.check;
  u.note.hidden = !(c && c.exists);
  var label = tr('export.go'), enabled = !!c;
  if (c && c.exists) {
    var n = c.counts;
    u.note.textContent = '';
    u.note.appendChild(el('b', '', tr('export.exists_title')));
    u.note.appendChild(el('br'));
    var text = tr('export.exists_text', { existing: trn('count.files', n.existing), already: trn('count.files', n.already), added: trn('count.files', n.new) });
    if (n.renamed) text += ' ' + tr('export.exists_renumber');
    u.note.appendChild(document.createTextNode(text));
    if (n.new) label = tr('export.go_add', { count: trn('count.files', n.new) });
    else if (n.renamed) label = tr('export.go_update');
    else enabled = false;
  }
  u.go.textContent = label;
  u.go.disabled = !enabled;
  u.error.textContent = ex.error || '';
}

function exportGo() {
  var c = ex.check;
  if (!c || ex.ui.go.disabled) return;
  if (c.shoebox) { exportWarn(c); return; }
  exportStart();
}

// The copies would land on a drive that has a shoebox library: its next scan
// finds them. Not forbidden (two people may be handing photos to each other),
// but asked.
function exportWarn(c) {
  var body = el('div');
  var same = c.shoebox.same_drive;
  body.appendChild(el('p', '', tr('export.other_text', { drive: c.shoebox.name })));
  body.appendChild(el('p', 'hint', tr(same ? 'export.same_hint' : 'export.other_hint', { count: trn('count.files', c.counts.new) })));
  openModal(tr(same ? 'export.same_title' : 'export.other_title'), body, [
    { label: tr('app.cancel'), cls: 'quiet', onclick: function () { renderExport(); return false; } },
    { label: tr('export.anyway'), onclick: function () { exportStart(); return false; } },
  ]);
}

function exportStart() {
  var req = exportRequest();
  // A drive with a shoebox library of its own (not the one the photos came from):
  // its next scan finds the copies, and Carry over data brings their tags and so on.
  var landed = ex.check && ex.check.shoebox && !ex.check.shoebox.same_drive ? ex.check.shoebox.name : null;
  post(LIBAPI + '/export/start', req).then(function () {
    exportLanded = landed;
    writeSetting(PARENT_KEY, ex.parent);
    clearTimeout(ex.timer);
    closeModal.onclose = null;
    ex = null;
    exportProgress();
  }).catch(function (e) {
    ex.error = e.message;
    renderExport();
  });
}

// ------------------------------------------------------------------ progress and result

function exportProgress() {
  var body = el('div');
  var line = el('p', '', '…');
  body.appendChild(line);
  var bar = el('div', 'x-bar');
  var fill = el('i');
  bar.appendChild(fill);
  body.appendChild(bar);
  body.appendChild(el('p', 'hint', tr('export.progress_hint')));
  var shown = true;
  openModal(tr('export.progress_title'), body, [
    { label: tr('export.stop'), cls: 'quiet', onclick: function (btn) { btn.disabled = true; post(LIBAPI + '/export/stop').catch(failed); return false; } },
  ], 'wide');
  closeModal.onclose = function () { shown = false; };
  var poll = function () {
    api(LIBAPI + '/export/status').then(function (s) {
      if (s.state === 'running') {
        line.textContent = tr('export.progress', { done: s.done + 1 > s.total ? s.total : s.done + 1, total: s.total, name: s.current });
        fill.style.width = (s.total ? Math.round(100 * s.done / s.total) : 0) + '%';
        setTimeout(poll, 400);
        return;
      }
      if (shown) { closeModal.onclose = null; exportResult(s); } else toast(tr('export.finished_toast', { count: trn('count.files', s.copied + s.already) }));
      changed();
    }).catch(function (e) { if (shown) { closeModal(); failed(e); } });
  };
  poll();
}

// The drive with a library the last export wrote to, until its result is shown.
var exportLanded = null;

function exportResult(s) {
  var body = el('div');
  var landed = exportLanded;
  exportLanded = null;
  if (s.state === 'failed') {
    body.appendChild(el('div', 'x-note', s.error || ''));
  } else {
    var good = el('div', 'x-note good');
    good.appendChild(el('b', '', tr('export.done_text', { count: trn('count.files', s.copied + s.already), path: s.dest })));
    if (s.copied && !s.failed.length) {
      good.appendChild(el('br'));
      good.appendChild(document.createTextNode(tr('export.done_check')));
    }
    body.appendChild(good);
    var parts = [tr('export.done_new', { count: trn('count.files', s.copied) }), tr('export.done_already', { count: trn('count.files', s.already) })];
    if (s.converted) parts.unshift(tr('export.done_converted', { count: trn('count.photos', s.converted) }));
    if (s.renamed) parts.push(tr('export.done_renumbered', { count: trn('count.files', s.renamed) }));
    body.appendChild(el('p', 'hint', parts.join(' · ')));
    if (s.no_exif) body.appendChild(el('p', 'hint', tr('export.no_exif', { count: trn('count.photos', s.no_exif) })));
    if (landed && (s.copied || s.already)) body.appendChild(el('p', 'hint', tr('export.transfer_hint', { drive: landed })));
  }
  if (s.failed && s.failed.length) {
    body.appendChild(el('p', '', tr('export.some_failed', { count: trn('count.files', s.failed.length) })));
    var ul = el('ul', 'filelist');
    s.failed.forEach(function (l) { ul.appendChild(el('li', '', l)); });
    body.appendChild(ul);
  }
  var actions = [{ label: tr('app.close'), cls: 'quiet' }];
  if (s.copied || s.already) {
    actions.push({
      label: state.reveal || tr('reveal.file_manager'),
      onclick: function () { post(LIBAPI + '/export/reveal').catch(failed); },
    });
  }
  openModal(tr(s.state === 'failed' ? 'export.failed_title' : s.state === 'stopped' ? 'export.stopped_title' : 'export.done_title'), body, actions, 'wide');
}

// ------------------------------------------------------------------ the folder picker

// The export's picker: where the new folder goes.
function exportPicker() {
  pathPicker({
    title: tr('export.pick_title'),
    hint: tr('export.pick_hint'),
    base: LIBAPI,
    places: ex.places,
    start: ex.parent,
    use: tr('export.use_folder'),
    onPick: function (path) { ex.parent = path; renderExport(); },
    onCancel: renderExport,
  });
}

// The computer's own folders, listed by the server (a browser cannot give a
// web page a real folder or file). Drives with a shoebox library are marked.
// `opts`: title, hint, base (the API of the drive that lists them), places
// (its /export/places), start (a folder), use (the button's label), onPick
// (with the folder's, or the file's, path) and onCancel. With `files` the
// `.json` files are listed too, and one of them is what is picked (Carry over
// data from a userdata.json).
function pathPicker(opts) {
  var current = null, chosen = null, useBtn = null;
  var body = el('div');
  body.appendChild(el('p', 'hint', opts.hint));
  var pk = el('div', 'x-pk');
  var nav = el('nav');
  var main = el('div', 'x-main');
  var crumbs = el('div', 'x-crumbs');
  var list = el('ul');
  main.appendChild(crumbs);
  main.appendChild(list);
  pk.appendChild(nav);
  pk.appendChild(main);
  body.appendChild(pk);
  var selected = el('p', 'hint x-selected');
  body.appendChild(selected);
  var error = el('p', 'error');
  body.appendChild(error);

  var navItems = [];
  var addNav = function (label, path, extra) {
    var d = el('div', '', label);
    if (extra) d.appendChild(el('small', '', extra));
    d.onclick = function () { show(path); };
    d.dataset.path = path;
    nav.appendChild(d);
    navItems.push(d);
  };
  nav.appendChild(el('h4', '', tr('export.places')));
  opts.places.places.forEach(function (p) { addNav(tr(PLACE_NAMES[p.kind] || 'export.place_home'), p.path); });
  if (opts.places.drives.length) {
    nav.appendChild(el('h4', '', tr('export.drives')));
    opts.places.drives.forEach(function (d) {
      var extra = d.library ? tr('export.shoebox_tag') : d.free != null ? tr('export.free', { size: formatBytes(d.free) }) : '';
      addNav(d.name, d.path, extra);
    });
  }

  var choose = function (f, li) {
    chosen = f;
    Array.prototype.forEach.call(list.children, function (x) { x.classList.toggle('on', x === li); });
    selected.textContent = f ? tr('export.selected', { name: f.name }) : '';
    if (useBtn) useBtn.disabled = !f;
  };
  var show = function (path) {
    error.textContent = '';
    api(opts.base + '/export/folders' + query({ path: path, files: opts.files ? 'json' : null })).then(function (l) {
      current = l;
      navItems.forEach(function (d) { d.classList.toggle('on', d.dataset.path === l.path); });
      crumbs.textContent = '';
      l.crumbs.forEach(function (c, i) {
        if (i) crumbs.appendChild(document.createTextNode(' › '));
        var a = el('button', 'link', c.name);
        a.onclick = function () { show(c.path); };
        crumbs.appendChild(a);
      });
      if (l.shoebox) crumbs.appendChild(el('span', 'x-tag', tr('export.shoebox_tag')));
      list.textContent = '';
      var files = opts.files ? l.files || [] : [];
      if (!l.folders.length && !files.length) list.appendChild(el('li', 'x-none', tr(opts.files ? 'transfer.pick_empty' : 'export.empty_folder')));
      l.folders.forEach(function (f) {
        var li = el('li');
        var nm = el('span', '');
        nm.appendChild(folderIcon());
        nm.appendChild(document.createTextNode(f.name));
        li.appendChild(nm);
        if (f.library) li.appendChild(el('small', '', tr('export.shoebox_tag')));
        li.onclick = function () { show(f.path); };
        list.appendChild(li);
      });
      files.forEach(function (f) {
        var li = el('li');
        var nm = el('span', '');
        nm.appendChild(fileIcon());
        nm.appendChild(document.createTextNode(f.name));
        li.appendChild(nm);
        li.appendChild(el('small', '', formatBytes(f.size)));
        li.onclick = function () { choose(f, li); };
        li.ondblclick = function () { choose(f, li); opts.onPick(f.path, l.path); };
        list.appendChild(li);
      });
      if (opts.files) choose(null);
      else selected.textContent = tr('export.selected', { name: l.crumbs[l.crumbs.length - 1].name });
    }).catch(function (e) { error.textContent = e.message; });
  };

  var buttons = openModal(opts.title, body, [
    { label: tr('app.cancel'), cls: 'quiet', onclick: function () { opts.onCancel(); return false; } },
    {
      label: opts.use,
      onclick: function () {
        if (opts.files ? !chosen : !current) return false;
        if (opts.files) opts.onPick(chosen.path, current.path); else opts.onPick(current.path);
        return false;
      },
    },
  ], 'picker');
  useBtn = buttons[1];
  if (opts.files) useBtn.disabled = true;
  show(opts.start && opts.start.length ? opts.start : (opts.places.places[0] ? opts.places.places[0].path : '/'));
}
