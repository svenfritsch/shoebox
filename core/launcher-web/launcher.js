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

var paths = [];          // the chosen folders, remembered in the launcher's JSON config
var drives = [];
var busy = false;        // a command is running
var serving = false;     // the photo app is running: everything else is locked

function savePaths() {
  api('/api/config', { paths: paths }).catch(function () { /* the list still works */ });
}

function addPath(value) {
  value = value.trim();
  if (!value || paths.indexOf(value) >= 0) return;
  paths.push(value);
  renderChips();
  savePaths();
}

function removePath(value) {
  paths = paths.filter(function (p) { return p !== value; });
  renderChips();
  savePaths();
}

function renderChips() {
  var ul = $('path-chips');
  ul.textContent = '';
  paths.forEach(function (p) {
    var li = document.createElement('li');
    var known = drives.filter(function (d) { return d.path === p; })[0];
    if (drives.length && !known && p.indexOf('/Volumes/') === 0) li.className = 'offline';
    var name = document.createElement('span'); name.className = 'name'; name.textContent = p; name.title = p;
    var x = document.createElement('button');
    x.type = 'button'; x.textContent = '×'; x.title = tr('launcher.remove'); x.setAttribute('aria-label', tr('launcher.remove_path', { path: p }));
    x.onclick = function () { removePath(p); };
    li.appendChild(name); li.appendChild(x);
    ul.appendChild(li);
  });
  $('root').placeholder = tr(paths.length ? 'launcher.root.placeholder_more' : 'launcher.root.placeholder');
  renderBackupPick();
}

// The backup check compares exactly two of the chosen folders; the person says which is which.
var bkOrig = '', bkCopy = '';
function renderBackupPick() {
  var box = $('backup-pick');
  box.hidden = paths.length < 2;
  if (paths.length < 2) { bkOrig = bkCopy = ''; return; }
  if (paths.indexOf(bkOrig) < 0) bkOrig = paths[0];
  if (paths.indexOf(bkCopy) < 0 || bkCopy === bkOrig) bkCopy = paths.filter(function (p) { return p !== bkOrig; })[0];
  [['bk-orig', bkOrig], ['bk-copy', bkCopy]].forEach(function (pair) {
    var sel = $(pair[0]);
    sel.textContent = '';
    paths.forEach(function (p) {
      var o = document.createElement('option'); o.value = p; o.textContent = p;
      if (p === pair[1]) o.selected = true;
      sel.appendChild(o);
    });
  });
  document.querySelectorAll('#path-chips li').forEach(function (li, i) {
    var role = paths[i] === bkOrig ? 'launcher.badge.original' : paths[i] === bkCopy ? 'launcher.badge.backup' : '';
    if (!role) return;
    var b = document.createElement('span'); b.className = 'badge'; b.textContent = tr(role);
    li.insertBefore(b, li.querySelector('button'));
  });
}
$('bk-orig').addEventListener('change', function () { bkOrig = this.value; renderBackupPick(); });
$('bk-copy').addEventListener('change', function () { bkCopy = this.value; renderBackupPick(); });

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
  if (v && drives.some(function (d) { return d.path === v; })) { addPath(v); $('root').value = ''; }
});
$('chipbox').addEventListener('click', function (ev) { if (ev.target === this) $('root').focus(); });

function loadDrives() {
  return api('/api/drives').then(function (list) {
    drives = list;
    var dl = $('drives'), chips = $('drive-chips');
    dl.textContent = ''; chips.textContent = '';
    drives.forEach(function (d) {
      var o = document.createElement('option'); o.value = d.path; dl.appendChild(o);
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
  return api('/api/config').then(function (c) { paths = c.paths || []; renderChips(); }).catch(function () {});
}

// ---------------------------------------------------------------- commands

function startJob(kind) {
  if (!paths.length) { $('root').focus(); $('root-hint').textContent = tr('launcher.need_folder'); return; }
  if (kind === 'backup' && (paths.length < 2 || !bkOrig || !bkCopy || bkOrig === bkCopy)) {
    $('root-hint').textContent = tr('launcher.backup_two');
    return;
  }
  api('/api/job', {
    kind: kind, roots: kind === 'backup' ? [bkOrig, bkCopy] : paths,
    quick: $('opt-quick').checked, rotated: $('opt-rotated').checked, deep: $('opt-deep').checked,
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
// the photo app runs; only Cancel (command) or Stop (app) stay usable.
function applyLocks() {
  $('controls').disabled = busy || serving;
  $('start-app').disabled = busy;
}

function pill(text, cls) {
  var s = document.createElement('span'); s.className = 'pill ' + (cls || ''); s.textContent = text; return s;
}

function render(job) {
  if (!job.id) return;
  $('progress-card').hidden = false;
  var name = kindName(job.kind);
  busy = job.running;
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
  if (job.kind === 'backup' && job.roots.length === 2) {
    summary.appendChild(pill(tr('launcher.backup.pair', { orig: job.roots[0], copy: job.roots[1] })));
  }
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

  var only = $('only-failed').checked;
  var items = job.failures.concat(only ? [] : job.recent_ok.slice().reverse());
  var ul = $('results');
  ul.textContent = '';
  items.slice(0, 600).forEach(function (f) {
    var li = document.createElement('li'); li.className = f.ok ? 'ok' : 'bad';
    var m = document.createElement('span'); m.className = 'mark'; m.textContent = f.ok ? '✓' : '✗';
    var pa = document.createElement('span'); pa.className = 'path'; pa.textContent = f.path;
    var n = document.createElement('span'); n.className = 'note'; n.textContent = f.note;
    li.appendChild(m); li.appendChild(pa); li.appendChild(n);
    ul.appendChild(li);
  });
  if (!items.length) {
    var li = document.createElement('li'); li.textContent = tr(job.running ? 'launcher.nothing_yet' : only ? 'launcher.no_problems' : 'launcher.no_files');
    ul.appendChild(li);
  }
  $('lines').textContent = job.lines.join('\n');
}

function summaryOf(kind, r) {
  var out = [];
  if (kind === 'scan') out.push(tr('launcher.sum.added', { n: r.added }), tr('launcher.sum.moved', { n: r.moved }), tr('launcher.sum.changed', { n: r.changed }), tr('launcher.sum.missing', { n: r.missing }));
  else if (kind === 'backup') out.push(tr('launcher.sum.backup_covered', { covered: r.report.covered, compared: r.report.compared }), tr('launcher.sum.backup_missing', { n: r.report.missing }), tr('launcher.sum.backup_different', { n: r.report.different }), tr('launcher.sum.backup_extra', { n: r.report.extra }));
  else if (kind === 'verify') out.push(tr('launcher.sum.checked', { n: r.checked }), tr('launcher.sum.missing', { n: r.missing.length }), tr('launcher.sum.damaged', { n: r.damaged.length }));
  else if (kind === 'recognize' && r.faces !== undefined) out.push(tr('launcher.sum.faces', { n: r.faces }));
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
  if (!paths.length) { $('root').focus(); $('root-hint').textContent = tr('launcher.need_folder'); return; }
  api('/api/app', { roots: paths }).then(function (r) {
    return appState().then(function () { window.open(r.url, '_blank', 'noopener'); });
  }).catch(function (e) { $('app-state').textContent = e.message; });
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
