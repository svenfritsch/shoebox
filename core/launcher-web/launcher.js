'use strict';
// The launcher page: choose a folder, press a button, watch the progress.
// Everything runs on this computer; the changes it asks for are POSTs with
// the X-Shoebox header, like the photo app.

var $ = function (id) { return document.getElementById(id); };
function kindName(kind) { return tr('launcher.kind.' + kind); }
var polling = null;

function api(path, body) {
  var opts = { credentials: 'same-origin' };
  if (body !== undefined) {
    opts.method = 'POST';
    opts.headers = { 'Content-Type': 'application/json', 'X-Shoebox': '1' };
    opts.body = JSON.stringify(body);
  }
  return fetch(path, opts).then(function (r) {
    return r.json().catch(function () { return {}; }).then(function (data) {
      if (!r.ok) throw new Error(data.error || path + ': HTTP ' + r.status);
      return data;
    });
  });
}

// ---------------------------------------------------------------- folders (chips)

var paths = [];          // the folders in the list, remembered in the launcher's JSON config
var off = {};            // the ones that are not ticked (everything else is)
var recentPaths = [];        // every folder ever added: offered in the drop-down again
var backups = {};        // backup folder -> { of, at }: a backup check found it complete
var drives = [];
var runningKind = '';    // the kind of the command that runs
var busy = false;        // a command is running
var serving = false;     // the photo app is running: only recognition may still run

function tickedPaths() {
  return paths.filter(function (p) { return !off[p]; });
}

function savePaths() {
  api('/api/config', { paths: paths, checked: tickedPaths() }).then(function (c) {
    recentPaths = c.history || recentPaths;
    backups = c.backups || backups;
    renderDatalist();
  }).catch(function () { /* the list still works */ });
}

function addPath(value) {
  value = value.trim();
  if (!value || paths.indexOf(value) >= 0) return;
  paths.push(value);
  delete off[value];
  renderChips();
  savePaths();
}

function removePath(value) {
  paths = paths.filter(function (p) { return p !== value; });
  delete off[value];
  renderChips();
  savePaths();
}

// "tesselina's harddrive" for /Volumes/tesselina's harddrive/Photos/Familie.
function driveName(p) {
  var m = /^\/Volumes\/([^\/]+)/.exec(p) || /^([A-Za-z]:)/.exec(p);
  return m ? m[1] : p.split(/[\\/]/).filter(Boolean).pop() || p;
}

function renderChips() {
  var ul = $('path-chips');
  ul.textContent = '';
  paths.forEach(function (p) {
    var li = document.createElement('li');
    var known = drives.filter(function (d) { return d.path === p; })[0];
    if (drives.length && !known && p.indexOf('/Volumes/') === 0) li.className = 'offline';
    if (off[p]) li.className += ' unticked';
    var cb = document.createElement('input');
    cb.type = 'checkbox'; cb.checked = !off[p];
    cb.setAttribute('aria-label', tr('launcher.tick', { path: p })); cb.title = tr('launcher.tick', { path: p });
    cb.onchange = function () {
      if (cb.checked) delete off[p]; else off[p] = true;
      renderChips();
      savePaths();
    };
    var name = document.createElement('span'); name.className = 'name'; name.textContent = p; name.title = p;
    li.appendChild(cb); li.appendChild(name);
    var mark = backups[p];
    if (mark) {
      var b = document.createElement('span'); b.className = 'badge';
      b.textContent = tr('launcher.backup_of', { name: driveName(mark.of) });
      b.title = tr('launcher.backup_of.title', { path: mark.of, date: I18n.date(new Date(mark.at * 1000)) });
      li.appendChild(b);
    }
    var x = document.createElement('button');
    x.type = 'button'; x.textContent = '×'; x.title = tr('launcher.remove'); x.setAttribute('aria-label', tr('launcher.remove_path', { path: p }));
    x.onclick = function () { removePath(p); };
    li.appendChild(x);
    ul.appendChild(li);
  });
  $('root').placeholder = tr(paths.length ? 'launcher.root.placeholder_more' : 'launcher.root.placeholder');
  updateButtons();
}

// Every action works on the ticked folders; the backup check on exactly two.
function updateButtons() {
  var n = tickedPaths().length;
  document.querySelectorAll('.actions button').forEach(function (b) {
    b.disabled = n === 0 || (b.dataset.kind === 'backup' && n !== 2) || addonMissing(b.dataset.kind).length > 0;
  });
  $('backup-hint').textContent = n === 0 ? tr('launcher.need_ticked') : n !== 2 ? tr('launcher.backup_need_two') : '';
  loadAddons();
}

// ---------------------------------------------------------------- add-ons

// What the recognition buttons need: faces (Recognize, lying down) or pets.
var ADDON_OF = { recognize: 'faces', recognize_rotated: 'faces', recognize_pets: 'pets' };
var addons = null;       // /api/addons: what is installed, for this computer and each ticked folder

// The ticked folders where the add-on the button needs is missing.
function addonMissing(kind) {
  var need = ADDON_OF[kind];
  if (!need || !addons) return [];
  return addons.roots.filter(function (r) { return !r[need]; }).map(function (r) { return driveName(r.root); });
}

function loadAddons() {
  return api('/api/addons', { roots: tickedPaths() }).then(function (a) {
    addons = a;
    renderAddons();
    foldWhenComplete();
  }).catch(function () {});
}

function renderAddons() {
  if (!addons) return;
  var state = function (id, on) {
    var s = $(id); s.textContent = tr(on ? 'launcher.addon.installed' : 'launcher.addon.missing'); s.className = 'pill' + (on ? ' ok' : '');
  };
  // Never optional: until it is installed the pill says it is required.
  var rt = $('addon-runtime-state');
  rt.textContent = tr(addons.runtime ? 'launcher.addon.installed' : 'launcher.addon.required');
  rt.className = 'pill' + (addons.runtime ? ' ok' : ' req');
  state('addon-faces-state', addons.faces);
  state('addon-pets-state', addons.pets);
  // Each add-on stands alone; an installed one is ticked and cannot be ticked off.
  ['faces', 'pets'].forEach(function (k) {
    var box = $('addon-' + k);
    box.checked = box.checked || addons[k];
    box.disabled = busy || serving || addons[k];
  });
  $('addons-install').disabled = busy || serving || !addons.installable || wantedAddons().length === 0;
  $('addons-hint').textContent = !addons.installable ? tr('launcher.addons.not_installable')
    : addons.models && !addons.runtime ? tr('launcher.addons.runtime_missing', { dir: addons.dir })
    : addons.faces && addons.pets ? tr('launcher.addons.all_installed', { dir: addons.dir })
    : tr('launcher.addons.where', { dir: addons.dir });
  // The folded card still says where things stand.
  $('addons-summary').textContent = [
    tr('launcher.addon.faces') + ': ' + tr(addons.faces ? 'launcher.addon.installed' : 'launcher.addon.missing'),
    tr('launcher.addon.pets') + ': ' + tr(addons.pets ? 'launcher.addon.installed' : 'launcher.addon.missing'),
  ].join(' · ');
  // Why a recognition button is grey.
  var why = [];
  var noFaces = addonMissing('recognize'), noPets = addonMissing('recognize_pets');
  if (noFaces.length) why.push(tr('launcher.addons.need_faces', { drives: noFaces.join(', ') }));
  if (noPets.length) why.push(tr('launcher.addons.need_pets', { drives: noPets.join(', ') }));
  $('addon-need').textContent = why.join(' ');
  document.querySelectorAll('.actions button').forEach(function (b) {
    if (addonMissing(b.dataset.kind).length) b.disabled = true;
  });
}

// The add-ons ticked that are not installed yet.
function wantedAddons() {
  return ['faces', 'pets'].filter(function (k) { return $('addon-' + k).checked && !(addons && addons[k]); });
}
$('addon-faces').onchange = $('addon-pets').onchange = renderAddons;
// The runtime is installed with the first add-on: its box is ticked and cannot be changed.
$('addon-runtime').checked = true;
$('addon-runtime').onclick = function (ev) { ev.preventDefault(); };

// Fold the card: only the heading stays. The choice is remembered; until then
// it is open while something is missing and folded once everything is there.
var detailsChosen = false;
try { var saved = localStorage.getItem('shoebox.addons.open'); if (saved !== null) { detailsChosen = true; $('addons-details').open = saved === '1'; } } catch (e) {}
$('addons-details').addEventListener('toggle', function () {
  if (settingDefault) return;
  detailsChosen = true;
  try { localStorage.setItem('shoebox.addons.open', $('addons-details').open ? '1' : '0'); } catch (e) {}
});
var settingDefault = false;
function foldWhenComplete() {
  if (detailsChosen || !addons) return;
  settingDefault = true;
  $('addons-details').open = !(addons.runtime && addons.faces && addons.pets);
  setTimeout(function () { settingDefault = false; }, 0);
}

$('addons-install').onclick = function () {
  var want = wantedAddons();
  api('/api/job', { kind: 'install_addons', faces: want.indexOf('faces') >= 0, pets: want.indexOf('pets') >= 0 }).then(function () {
    $('progress-card').hidden = false;
    $('progress-card').scrollIntoView({ behavior: 'smooth' });
    poll();
  }).catch(function (e) {
    $('progress-card').hidden = false;
    $('job-title').textContent = tr('launcher.not_started', { name: kindName('install_addons') });
    $('job-error').hidden = false; $('job-error').textContent = e.message;
  });
};

$('root').addEventListener('keydown', function (ev) {
  if (ev.key === 'Enter') {
    ev.preventDefault();
    addPath($('root').value);
    $('root').value = '';
  } else if (ev.key === 'Backspace' && !$('root').value && paths.length) {
    removePath(paths[paths.length - 1]);
  }
});
// A suggestion picked from the list is a finished path: add it right away.
$('root').addEventListener('change', function () {
  var v = $('root').value.trim();
  if (v && (drives.some(function (d) { return d.path === v; }) || recentPaths.indexOf(v) >= 0)) { addPath(v); $('root').value = ''; }
});
$('chipbox').addEventListener('click', function (ev) { if (ev.target === this) $('root').focus(); });

// The drop-down: detected drives and every folder added before, except those already in the list.
function renderDatalist() {
  var dl = $('drives');
  dl.textContent = '';
  var seen = {};
  drives.map(function (d) { return d.path; }).concat(recentPaths).forEach(function (p) {
    if (seen[p] || paths.indexOf(p) >= 0) return;
    seen[p] = true;
    var o = document.createElement('option'); o.value = p; dl.appendChild(o);
  });
}

function loadDrives() {
  return api('/api/drives').then(function (list) {
    drives = list;
    var chips = $('drive-chips');
    chips.textContent = '';
    renderDatalist();
    drives.forEach(function (d) {
      var b = document.createElement('button');
      b.type = 'button'; b.textContent = d.name; b.title = d.path;
      if (d.library) b.className = 'has-lib';
      b.onclick = function () { addPath(d.path); };
      chips.appendChild(b);
    });
    $('root-hint').textContent = drives.length
      ? tr('launcher.drives.has_lib')
      : tr('launcher.drives.none');
    renderChips();
  }).catch(function (e) { $('root-hint').textContent = String(e.message); });
}

function loadConfig() {
  return api('/api/config').then(function (c) {
    paths = c.paths || [];
    recentPaths = c.history || [];
    backups = c.backups || {};
    off = {};
    if (c.checked) paths.forEach(function (p) { if (c.checked.indexOf(p) < 0) off[p] = true; });
    renderChips();
    renderDatalist();
  }).catch(function () {});
}

// ---------------------------------------------------------------- commands

function startJob(kind) {
  var roots = tickedPaths();
  if (!roots.length) { $('root').focus(); $('root-hint').textContent = tr('launcher.need_ticked'); return; }
  if (kind === 'backup' && roots.length !== 2) {
    $('root-hint').textContent = tr('launcher.backup_need_two');
    return;
  }
  api('/api/job', {
    kind: kind, roots: roots,
    quick: $('opt-quick').checked, deep: $('opt-deep').checked,
  }).then(function () {
    $('progress-card').hidden = false;
    $('progress-card').scrollIntoView({ behavior: 'smooth' });
    poll();
  }).catch(function (e) {
    $('progress-card').hidden = false;
    $('job-title').textContent = tr('launcher.not_started', { name: kindName(kind) });
    $('job-error').hidden = false; $('job-error').textContent = e.message;
  });
}

function poll() {
  clearTimeout(polling);
  api('/api/job').then(function (job) {
    render(job);
    if (job.running) polling = setTimeout(poll, 500);
  }).catch(function () { polling = setTimeout(poll, 2000); });
}

// Everything that starts or changes something is locked while a command or
// the photo app runs; only Cancel (command) or Stop (app) stay usable. The
// recognition buttons are the exception while the photo app runs, and the
// photo app can be started while recognition runs.
var BESIDE_APP = ['recognize', 'recognize_pets', 'recognize_rotated', 'faces_stats'];

function applyLocks() {
  $('controls').disabled = busy || serving;
  document.querySelectorAll('.actions button').forEach(function (b) {
    b.disabled = busy || (serving && BESIDE_APP.indexOf(b.dataset.kind) < 0) || addonMissing(b.dataset.kind).length > 0;
  });
  renderAddons();
  $('opt-quick').disabled = $('opt-deep').disabled = busy || serving;
  $('start-app').disabled = busy && BESIDE_APP.indexOf(runningKind) < 0;
}

function pill(text, cls) {
  var s = document.createElement('span'); s.className = 'pill ' + (cls || ''); s.textContent = text; return s;
}

var reloadedFor = 0;
var addonsFor = 0;
var lastJob = null;
function render(job) {
  if (!job.id) return;
  lastJob = job;
  showCleanup(job);
  showArrivals(job);
  showForget(job);
  if (job.kind === 'backup' && !job.running && reloadedFor !== job.id) { reloadedFor = job.id; loadConfig(); }
  if (job.kind === 'install_addons' && !job.running && addonsFor !== job.id) { addonsFor = job.id; loadAddons(); }
  $('progress-card').hidden = false;
  var name = kindName(job.kind);
  busy = job.running;
  runningKind = job.running ? job.kind : '';
  applyLocks();
  $('cancel').hidden = !job.running;
  $('cancel').disabled = false;
  var where = job.current_root && job.roots.length > 1 ? ' (' + job.current_root + ')' : '';
  $('job-title').textContent = job.running ? tr('launcher.running', { name: name, where: where })
    : job.cancelled ? tr('launcher.cancelled', { name: name })
    : tr(job.ok ? 'launcher.finished' : 'launcher.finished_problems', { name: name });
  var bar = document.querySelector('.bar');
  var p = job.progress;
  var pct = p && p.total ? Math.min(100, Math.round(100 * p.done / p.total)) : null;
  bar.classList.toggle('busy', job.running && pct === null);
  $('bar-fill').style.width = job.running ? (pct === null ? '30%' : pct + '%') : '100%';
  bar.setAttribute('aria-valuenow', job.running ? (pct === null ? '' : pct) : 100);
  $('progress-label').textContent = job.running ? (p ? p.label : tr('launcher.starting')) : '';

  var summary = $('summary');
  summary.textContent = '';
  summary.hidden = false;
  summary.appendChild(pill(tr('launcher.worked', { n: job.ok_count }), 'ok'));
  summary.appendChild(pill(tr('launcher.failed', { n: job.fail_count }), job.fail_count ? 'bad' : ''));
  if (!job.running) {
    // One pill per folder, so several drives stay readable.
    job.results.forEach(function (r) {
      if (!r.result) return;
      var label = job.results.length > 1 ? r.root.split(/[\\/]/).filter(Boolean).pop() + ': ' : '';
      summary.appendChild(pill(label + summaryOf(job.kind, r.result).join(' · ')));
    });
  }
  $('job-error').hidden = !job.error;
  $('job-error').textContent = job.error || '';

  var tabs = scanTabs(job);
  if (!tabs.some(function (t) { return t.id === activeTab; })) activeTab = 'results';
  renderTabs(tabs);
  var tab = tabs.filter(function (t) { return t.id === activeTab; })[0];
  var ul = $('results');
  ul.textContent = '';
  $('tab-hint').hidden = !(tab && tab.hint);
  $('tab-hint').textContent = tab && tab.hint ? tr(tab.hint) : '';
  $('only-failed-label').hidden = activeTab !== 'results';
  if (tab && tab.items) renderItems(ul, tab);
  else renderResults(job, ul);
  $('lines').textContent = job.lines.join('\n');
}

function renderResults(job, ul) {
  var dupNote = arrivalNotes(job);
  var only = $('only-failed').checked;
  var items = job.failures.concat(only ? [] : job.recent_ok.slice().reverse());
  items.slice(0, 600).forEach(function (f) {
    var again = dupNote[f.path.normalize('NFC')];
    var li = document.createElement('li'); li.className = f.ok ? 'ok' + (again ? ' dup' : '') : 'bad';
    var m = document.createElement('span'); m.className = 'mark'; m.textContent = f.ok ? '✓' : '✗';
    var pa = document.createElement('span'); pa.className = 'path'; pa.textContent = f.path;
    var n = document.createElement('span'); n.className = 'note'; n.textContent = again || f.note;
    li.appendChild(m); li.appendChild(pa);
    if (job.kind === 'backup' && job.roots.length === 2) {
      li.appendChild(revealButton(n, [{ root: job.roots[0], path: f.path }, { root: job.roots[1], path: f.path }], 'launcher.reveal_both'));
    }
    li.appendChild(n);
    ul.appendChild(li);
  });
  if (!items.length) {
    var li = document.createElement('li'); li.textContent = tr(job.running ? 'launcher.nothing_yet' : only ? 'launcher.no_problems' : 'launcher.no_files');
    ul.appendChild(li);
  }
}

function revealButton(status, items, label) {
  var r = document.createElement('button');
  r.type = 'button'; r.className = 'reveal'; r.textContent = '⌕';
  r.title = tr(label); r.setAttribute('aria-label', tr(label));
  r.onclick = function () {
    api('/api/reveal', { items: items })
      .then(function (res) { if (!res.opened) status.textContent = tr(res.found ? 'launcher.reveal_failed' : 'launcher.reveal_none'); })
      .catch(function (e) { status.textContent = e.message; });
  };
  return r;
}

// After a scan: one tab per kind of file that needs a look, next to the results.
var activeTab = 'results';
function scanTabs(job) {
  var tabs = [{ id: 'results', label: tr('launcher.results') }];
  if (job.kind !== 'scan' || job.running) return tabs;
  var kinds = [
    { id: 'moved', key: 'moved_files', total: 'moved', hint: 'launcher.tab.moved.hint', label: 'launcher.tab.moved' },
    { id: 'changed', key: 'changed_files', total: 'changed', hint: 'launcher.tab.changed.hint', label: 'launcher.tab.changed', warn: true },
    { id: 'missing', key: 'missing_files', total: 'missing', hint: 'launcher.tab.missing.hint', label: 'launcher.tab.missing', warn: true, gone: true },
    { id: 'arrivals', key: 'duplicates', total: 'duplicates_total', label: 'launcher.tab.arrivals' },
  ];
  kinds.forEach(function (k) {
    var items = [], total = 0;
    job.results.forEach(function (r) {
      if (!r.result) return;
      var pre = rootPrefix(job, r);
      total += r.result[k.total] || 0;
      (r.result[k.key] || []).forEach(function (x) {
        var o = typeof x === 'string' ? { path: x } : x;
        var item = { root: r.root, path: o.path, text: pre + o.path, gone: k.gone };
        if (k.id === 'moved') item.note = tr('launcher.tab.moved.was', { from: o.from });
        if (k.id === 'arrivals') { item.note = tr('launcher.arrivals.note', { of: o.of }); item.also = o.of; }
        items.push(item);
      });
    });
    if (total > 0) tabs.push({ id: k.id, label: tr(k.label, { n: total.toLocaleString() }), items: items, total: total, hint: k.hint, warn: k.warn });
  });
  return tabs;
}
function renderTabs(tabs) {
  var bar = $('tabs');
  bar.hidden = tabs.length < 2;
  $('results-title').hidden = tabs.length > 1;
  bar.textContent = '';
  tabs.forEach(function (t) {
    var b = document.createElement('button');
    b.type = 'button'; b.setAttribute('role', 'tab'); b.textContent = t.label;
    b.setAttribute('aria-selected', t.id === activeTab ? 'true' : 'false');
    if (t.warn) b.className = 'warn';
    b.onclick = function () { activeTab = t.id; if (lastJob) render(lastJob); };
    bar.appendChild(b);
  });
}
function renderItems(ul, tab) {
  tab.items.slice(0, 600).forEach(function (it) {
    var li = document.createElement('li'); li.className = 'item';
    var pa = document.createElement('span'); pa.className = 'path'; pa.textContent = it.text;
    var n = document.createElement('span'); n.className = 'note'; n.textContent = it.note || '';
    li.appendChild(pa);
    if (!it.gone) {
      var shown = [{ root: it.root, path: it.path }];
      if (it.also) shown.push({ root: it.root, path: it.also });
      li.appendChild(revealButton(n, shown, it.also ? 'launcher.reveal_copies' : 'launcher.reveal_one'));
    }
    if (it.note) li.appendChild(n);
    ul.appendChild(li);
  });
  if (tab.total > 600) {
    var li = document.createElement('li'); li.className = 'item';
    li.textContent = tr('launcher.tab.more', { n: (tab.total - 600).toLocaleString() });
    ul.appendChild(li);
  }
}

// After a scan: new files whose content the drive had already (`arrivals.rs`).
function rootPrefix(job, r) {
  return job.results.length > 1 ? r.root.split(/[\\/]/).filter(Boolean).pop() + ': ' : '';
}
function arrivalNotes(job) {
  var notes = {};
  if (job.kind !== 'scan') return notes;
  job.results.forEach(function (r) {
    ((r.result && r.result.duplicates) || []).forEach(function (d) {
      notes[(rootPrefix(job, r) + d.path).normalize('NFC')] = tr('launcher.arrivals.note', { of: d.of });
    });
  });
  return notes;
}
var arrivalsJob = null;
function showArrivals(job) {
  var hit = job.kind === 'scan' && !job.running ? job.results.filter(function (r) { return r.result && r.result.duplicates_total > 0; }) : [];
  var box = $('arrivals-box');
  box.hidden = !hit.length;
  if (box.hidden) return;
  arrivalsJob = { id: job.id, roots: hit.map(function (r) { return r.root; }) };
  arrivalsJob.n = hit.reduce(function (n, r) { return n + r.result.duplicates_total; }, 0);
  $('arrivals-text').textContent = hit.length === 1
    ? tr('launcher.arrivals.found', { n: arrivalsJob.n, drive: driveName(hit[0].root) })
    : tr('launcher.arrivals.found_many', { n: arrivalsJob.n });
}
$('arrivals-forever').checked = (function () { try { return localStorage.getItem('shoebox.arrivals.forever') === '1'; } catch (e) { return false; } })();
$('arrivals-go').onclick = function () {
  if (!arrivalsJob) return;
  var forever = $('arrivals-forever').checked;
  if (!window.confirm(tr(forever ? 'launcher.arrivals.confirm_forever' : 'launcher.arrivals.confirm', { n: arrivalsJob.n }))) return;
  try { localStorage.setItem('shoebox.arrivals.forever', forever ? '1' : '0'); } catch (e) { /* only a convenience */ }
  api('/api/job', { kind: 'scan_cleanup', roots: arrivalsJob.roots, forever: forever }).then(poll).catch(function (e) {
    $('job-error').hidden = false; $('job-error').textContent = e.message;
  });
};

// After a scan or verify that found files gone: forget their records (`scan::forget_missing_records`).
var forgetJob = null;
function goneCount(kind, res) {
  if (!res) return 0;
  if (kind === 'scan') return res.missing || 0;
  return kind === 'verify' ? res.missing.length + (res.relocated || 0) : 0;
}
function showForget(job) {
  var hit = !job.running ? job.results.filter(function (r) { return goneCount(job.kind, r.result) > 0; }) : [];
  var box = $('forget-box');
  box.hidden = !hit.length;
  if (box.hidden) return;
  forgetJob = { roots: hit.map(function (r) { return r.root; }) };
  forgetJob.n = hit.reduce(function (n, r) { return n + goneCount(job.kind, r.result); }, 0);
  $('forget-text').textContent = tr('launcher.forget.found', { n: forgetJob.n });
}
$('forget-go').onclick = function () {
  if (!forgetJob) return;
  if (!window.confirm(tr('launcher.forget.confirm', { n: forgetJob.n }))) return;
  api('/api/job', { kind: 'forget_missing', roots: forgetJob.roots }).then(poll).catch(function (e) {
    $('job-error').hidden = false; $('job-error').textContent = e.message;
  });
};

// After a backup check: copies removed on the duplicates screen that the backup still holds.
var cleanupJob = null;
function showCleanup(job) {
  var rep = job.kind === 'backup' && !job.running && job.result && job.result.report;
  var box = $('cleanup-box');
  box.hidden = !(rep && rep.removed > 0);
  if (box.hidden) return;
  cleanupJob = job;
  $('cleanup-text').textContent = tr('launcher.cleanup.found', { n: rep.removed, backup: driveName(job.roots[1]) });
}
$('cleanup-forever').checked = (function () { try { return localStorage.getItem('shoebox.cleanup.forever') === '1'; } catch (e) { return false; } })();
$('cleanup-go').onclick = function () {
  if (!cleanupJob) return;
  var forever = $('cleanup-forever').checked;
  var n = cleanupJob.result.report.removed;
  var name = driveName(cleanupJob.roots[1]);
  if (!window.confirm(tr(forever ? 'launcher.cleanup.confirm_forever' : 'launcher.cleanup.confirm', { n: n, backup: name }))) return;
  try { localStorage.setItem('shoebox.cleanup.forever', forever ? '1' : '0'); } catch (e) { /* only a convenience */ }
  api('/api/job', { kind: 'backup_cleanup', roots: cleanupJob.roots, forever: forever }).then(poll).catch(function (e) {
    $('job-error').hidden = false; $('job-error').textContent = e.message;
  });
};

function summaryOf(kind, r) {
  var out = [];
  if (kind === 'scan') {
    out.push(tr('launcher.sum.added', { n: r.added }), tr('launcher.sum.moved', { n: r.moved }), tr('launcher.sum.changed', { n: r.changed }), tr('launcher.sum.missing', { n: r.missing }));
    if (r.duplicates_total) out.push(tr('launcher.sum.arrivals', { n: r.duplicates_total }));
  } else if (kind === 'scan_cleanup') out.push(tr('launcher.sum.arrivals_removed', { n: r.removed.length }), tr('launcher.sum.cleanup_skipped', { n: r.skipped.length }));
  else if (kind === 'backup') out.push(tr('launcher.sum.backup_covered', { covered: r.report.covered, compared: r.report.compared }), tr('launcher.sum.backup_missing', { n: r.report.missing }), tr('launcher.sum.backup_different', { n: r.report.different }), tr('launcher.sum.backup_extra', { n: r.report.extra }));
  if (kind === 'backup' && r.report.removed) out.push(tr('launcher.sum.backup_removed', { n: r.report.removed }));
  else if (kind === 'backup_cleanup') out.push(tr('launcher.sum.cleanup', { n: r.removed.length }), tr('launcher.sum.cleanup_skipped', { n: r.skipped.length }));
  else if (kind === 'verify') {
    out.push(tr('launcher.sum.checked', { n: r.checked }), tr('launcher.sum.missing', { n: r.missing.length }), tr('launcher.sum.damaged', { n: r.damaged.length }));
    if (r.relocated) out.push(tr('launcher.sum.relocated', { n: r.relocated }));
  } else if ((kind === 'recognize' || kind === 'recognize_rotated') && r.faces !== undefined) out.push(tr('launcher.sum.faces', { n: r.faces }));
  else if (kind === 'forget_missing') out.push(tr('launcher.sum.forgotten', { n: r.forgotten }));
  else if (kind === 'recognize_pets' && r.pets) out.push(tr('launcher.sum.pets', { n: r.pets.faces }), tr('launcher.failed', { n: r.pets.failed }));
  else if (kind === 'faces_stats') out.push(r === null ? tr('launcher.sum.no_faces') : tr('launcher.sum.faces', { n: r.faces }));
  return out;
}

function appState() {
  return api('/api/app').then(function (s) {
    serving = !!s.running;
    applyLocks();
    $('stop-app').hidden = !s.running;
    $('start-app').textContent = tr(s.running ? 'launcher.app.open' : 'launcher.app.start');
    if (s.running) {
      var a = document.createElement('a'); a.href = s.url; a.target = '_blank'; a.rel = 'noopener'; a.textContent = s.url;
      $('app-state').textContent = tr('launcher.app.running', { roots: s.roots.join(', ') });
      $('app-state').appendChild(a);
      $('app-state').appendChild(document.createTextNode(tr('launcher.app.running_end')));
    }
  });
}

document.querySelectorAll('.actions button').forEach(function (b) {
  b.onclick = function () { startJob(b.dataset.kind); };
});
$('only-failed').onchange = poll;
$('start-app').onclick = function () {
  if (serving) { window.open($('app-state').querySelector('a').href, '_blank', 'noopener'); return; }
  var roots = tickedPaths();
  if (!roots.length) { $('root').focus(); $('root-hint').textContent = tr('launcher.need_ticked'); return; }
  // The server opens the browser itself when it starts the app.
  api('/api/app', { roots: roots }).then(appState).catch(function (e) { $('app-state').textContent = e.message; });
};
$('stop-app').onclick = function () {
  api('/api/app/stop', {}).then(appState).then(function () { $('app-state').textContent = tr('launcher.app.stopped'); });
};
$('cancel').onclick = function () {
  $('cancel').disabled = true;
  api('/api/job/cancel', {}).then(poll);
};

// The messages first: everything below builds text from them.
I18n.ready.then(function () {
  var sel = $('lang');
  Object.keys(I18n.langs).forEach(function (code) {
    var o = document.createElement('option'); o.value = code; o.textContent = I18n.langs[code];
    if (code === I18n.lang) o.selected = true;
    sel.appendChild(o);
  });
  sel.onchange = function () { I18n.setLang(sel.value); };
  loadConfig().then(loadDrives);
  appState();
  poll();
});
