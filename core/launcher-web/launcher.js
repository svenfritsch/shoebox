'use strict';
// The launcher page: choose a folder, press a button, watch the progress.
// Everything runs on this computer; the changes it asks for are POSTs with
// the X-Shoebox header, like the photo app.

var $ = function (id) { return document.getElementById(id); };
var KINDS = { scan: 'Scan', verify: 'Verify', recognize: 'Recognize', faces_stats: 'Face stats' };
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
    x.type = 'button'; x.textContent = '×'; x.title = 'Remove'; x.setAttribute('aria-label', 'Remove ' + p);
    x.onclick = function () { removePath(p); };
    li.appendChild(name); li.appendChild(x);
    ul.appendChild(li);
  });
  $('root').placeholder = paths.length ? 'Add another folder…' : '/Volumes/MyDrive';
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
      ? 'A check mark marks drives that already have a shoebox library.'
      : 'No drives found automatically: type the path of your drive or photo folder.';
    renderChips();
  }).catch(function (e) { $('root-hint').textContent = String(e.message); });
}

function loadConfig() {
  return api('/api/config').then(function (c) { paths = c.paths || []; renderChips(); }).catch(function () {});
}

// ---------------------------------------------------------------- commands

function startJob(kind) {
  if (!paths.length) { $('root').focus(); $('root-hint').textContent = 'Add at least one folder first.'; return; }
  api('/api/job', {
    kind: kind, roots: paths,
    quick: $('opt-quick').checked, rotated: $('opt-rotated').checked,
  }).then(function () {
    $('progress-card').hidden = false;
    $('progress-card').scrollIntoView({ behavior: 'smooth' });
    poll();
  }).catch(function (e) {
    $('progress-card').hidden = false;
    $('job-title').textContent = KINDS[kind] + ' did not start';
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
  var name = KINDS[job.kind] || job.kind;
  busy = job.running;
  applyLocks();
  $('cancel').hidden = !job.running;
  $('cancel').disabled = false;
  var where = job.current_root && job.roots.length > 1 ? ' (' + job.current_root + ')' : '';
  $('job-title').textContent = job.running ? name + ' is running…' + where
    : job.cancelled ? name + ' was cancelled (what was done is kept)'
    : name + (job.ok ? ' finished' : ' finished with problems');
  var bar = document.querySelector('.bar');
  var p = job.progress;
  var pct = p && p.total ? Math.min(100, Math.round(100 * p.done / p.total)) : null;
  bar.classList.toggle('busy', job.running && pct === null);
  $('bar-fill').style.width = job.running ? (pct === null ? '30%' : pct + '%') : '100%';
  bar.setAttribute('aria-valuenow', job.running ? (pct === null ? '' : pct) : 100);
  $('progress-label').textContent = job.running ? (p ? p.label : 'Starting…') : '';

  var summary = $('summary');
  summary.textContent = '';
  summary.hidden = false;
  summary.appendChild(pill(job.ok_count.toLocaleString() + ' worked', 'ok'));
  summary.appendChild(pill(job.fail_count.toLocaleString() + ' failed', job.fail_count ? 'bad' : ''));
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
    var li = document.createElement('li'); li.textContent = job.running ? 'Nothing to show yet.' : (only ? 'No problems.' : 'No files.');
    ul.appendChild(li);
  }
  $('lines').textContent = job.lines.join('\n');
}

function summaryOf(kind, r) {
  var out = [];
  var job = { kind: kind };
  if (job.kind === 'scan') out.push(r.added + ' added', r.moved + ' moved', r.changed + ' changed', r.missing + ' missing');
  else if (job.kind === 'verify') out.push(r.checked + ' checked', r.missing.length + ' missing', r.damaged.length + ' damaged');
  else if (job.kind === 'recognize' && r.faces !== undefined) out.push(r.faces + ' faces');
  else if (job.kind === 'faces_stats') out.push(r === null ? 'no faces yet' : (r.faces + ' faces'));
  return out;
}

function appState() {
  return api('/api/app').then(function (s) {
    serving = !!s.running;
    applyLocks();
    $('stop-app').hidden = !s.running;
    $('start-app').textContent = s.running ? 'Open photo app' : 'Start photo app';
    if (s.running) {
      var a = document.createElement('a'); a.href = s.url; a.target = '_blank'; a.rel = 'noopener'; a.textContent = s.url;
      $('app-state').textContent = 'Running for ' + s.roots.join(', ') + ' at ';
      $('app-state').appendChild(a);
      $('app-state').appendChild(document.createTextNode('. Stop it to scan, verify or change folders.'));
    }
  });
}

document.querySelectorAll('.actions button').forEach(function (b) {
  b.onclick = function () { startJob(b.dataset.kind); };
});
$('only-failed').onchange = poll;
$('start-app').onclick = function () {
  if (serving) { window.open($('app-state').querySelector('a').href, '_blank', 'noopener'); return; }
  if (!paths.length) { $('root').focus(); $('root-hint').textContent = 'Add at least one folder first.'; return; }
  api('/api/app', { roots: paths }).then(function (r) {
    return appState().then(function () { window.open(r.url, '_blank', 'noopener'); });
  }).catch(function (e) { $('app-state').textContent = e.message; });
};
$('stop-app').onclick = function () {
  api('/api/app/stop', {}).then(appState).then(function () { $('app-state').textContent = 'Photo app stopped.'; });
};
$('cancel').onclick = function () {
  $('cancel').disabled = true;
  api('/api/job/cancel', {}).then(poll);
};

loadConfig().then(loadDrives);
appState();
poll();
