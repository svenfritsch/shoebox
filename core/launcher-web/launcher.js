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
    b.disabled = n === 0 || (b.dataset.kind === 'backup' && n !== 2);
  });
  $('backup-hint').textContent = n === 0 ? tr('launcher.need_ticked') : n !== 2 ? tr('launcher.backup_need_two') : '';
}

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
// the photo app runs; only Cancel (command) or Stop (app) stay usable. The
// recognition buttons are the exception while the photo app runs, and the
// photo app can be started while recognition runs.
var BESIDE_APP = ['recognize', 'recognize_pets', 'faces_stats'];

function applyLocks() {
  $('controls').disabled = busy || serving;
  document.querySelectorAll('.actions button').forEach(function (b) {
    b.disabled = busy || (serving && BESIDE_APP.indexOf(b.dataset.kind) < 0);
  });
  $('opt-quick').disabled = $('opt-deep').disabled = busy || serving;
  $('opt-rotated').disabled = busy;
  $('start-app').disabled = busy && BESIDE_APP.indexOf(runningKind) < 0;
}

function pill(text, cls) {
  var s = document.createElement('span'); s.className = 'pill ' + (cls || ''); s.textContent = text; return s;
}

var reloadedFor = 0;
function render(job) {
  if (!job.id) return;
  showCleanup(job);
  if (job.kind === 'backup' && !job.running && reloadedFor !== job.id) { reloadedFor = job.id; loadConfig(); }
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

  var only = $('only-failed').checked;
  var items = job.failures.concat(only ? [] : job.recent_ok.slice().reverse());
  var ul = $('results');
  ul.textContent = '';
  items.slice(0, 600).forEach(function (f) {
    var li = document.createElement('li'); li.className = f.ok ? 'ok' : 'bad';
    var m = document.createElement('span'); m.className = 'mark'; m.textContent = f.ok ? '✓' : '✗';
    var pa = document.createElement('span'); pa.className = 'path'; pa.textContent = f.path;
    var n = document.createElement('span'); n.className = 'note'; n.textContent = f.note;
    li.appendChild(m); li.appendChild(pa);
    if (job.kind === 'backup' && job.roots.length === 2) {
      var r = document.createElement('button');
      r.type = 'button'; r.className = 'reveal'; r.textContent = '⌕';
      r.title = tr('launcher.reveal_both'); r.setAttribute('aria-label', tr('launcher.reveal_both'));
      r.onclick = function () {
        api('/api/reveal', { items: [{ root: job.roots[0], path: f.path }, { root: job.roots[1], path: f.path }] })
          .then(function (res) { if (!res.opened) n.textContent = tr(res.found ? 'launcher.reveal_failed' : 'launcher.reveal_none'); })
          .catch(function (e) { n.textContent = e.message; });
      };
      li.appendChild(r);
    }
    li.appendChild(n);
    ul.appendChild(li);
  });
  if (!items.length) {
    var li = document.createElement('li'); li.textContent = tr(job.running ? 'launcher.nothing_yet' : only ? 'launcher.no_problems' : 'launcher.no_files');
    ul.appendChild(li);
  }
  $('lines').textContent = job.lines.join('\n');
}

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
  if (kind === 'scan') out.push(tr('launcher.sum.added', { n: r.added }), tr('launcher.sum.moved', { n: r.moved }), tr('launcher.sum.changed', { n: r.changed }), tr('launcher.sum.missing', { n: r.missing }));
  else if (kind === 'backup') out.push(tr('launcher.sum.backup_covered', { covered: r.report.covered, compared: r.report.compared }), tr('launcher.sum.backup_missing', { n: r.report.missing }), tr('launcher.sum.backup_different', { n: r.report.different }), tr('launcher.sum.backup_extra', { n: r.report.extra }));
  if (kind === 'backup' && r.report.removed) out.push(tr('launcher.sum.backup_removed', { n: r.report.removed }));
  else if (kind === 'backup_cleanup') out.push(tr('launcher.sum.cleanup', { n: r.removed.length }), tr('launcher.sum.cleanup_skipped', { n: r.skipped.length }));
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
  var roots = tickedPaths();
  if (!roots.length) { $('root').focus(); $('root-hint').textContent = tr('launcher.need_ticked'); return; }
  api('/api/app', { roots: roots }).then(function (r) {
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
