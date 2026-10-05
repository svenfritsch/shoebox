'use strict';
// The launcher page: choose a folder, press a button, watch the progress.
// Everything runs on this computer; the changes it asks for are POSTs with
// the X-Shoebox header, like the photo app.

var $ = function (id) { return document.getElementById(id); };
var KINDS = { scan: 'Scan', verify: 'Verify', recognize: 'Recognize', faces_stats: 'Face stats' };
var polling = null;
var shown = { job: 0 };

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

function root() { return $('root').value.trim(); }

function loadDrives() {
  api('/api/drives').then(function (drives) {
    var list = $('drives'), chips = $('drive-chips');
    list.textContent = ''; chips.textContent = '';
    drives.forEach(function (d) {
      var o = document.createElement('option'); o.value = d.path; list.appendChild(o);
      var b = document.createElement('button');
      b.textContent = d.name; b.title = d.path;
      if (d.library) b.className = 'has-lib';
      b.onclick = function () { $('root').value = d.path; rememberRoot(); };
      chips.appendChild(b);
    });
    var saved = null;
    try { saved = localStorage.getItem('shoebox-root'); } catch (e) { /* private window */ }
    if (!root()) {
      var lib = drives.filter(function (d) { return d.library; })[0];
      $('root').value = saved || (lib && lib.path) || '';
    }
    $('root-hint').textContent = drives.length
      ? 'A check mark marks drives that already have a shoebox library.'
      : 'No drives found automatically: type the path of your drive or photo folder.';
  }).catch(function (e) { $('root-hint').textContent = String(e.message); });
}

function rememberRoot() {
  try { localStorage.setItem('shoebox-root', root()); } catch (e) { /* ignore */ }
}

function startJob(kind) {
  if (!root()) { $('root').focus(); $('root-hint').textContent = 'Enter the folder first.'; return; }
  rememberRoot();
  api('/api/job', {
    kind: kind, root: root(),
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

function pill(text, cls) {
  var s = document.createElement('span'); s.className = 'pill ' + (cls || ''); s.textContent = text; return s;
}

function render(job) {
  if (!job.id) return;
  $('progress-card').hidden = false;
  var name = KINDS[job.kind] || job.kind;
  document.querySelectorAll('.actions button').forEach(function (b) { b.disabled = job.running; });
  $('start-app').disabled = job.running;
  $('job-title').textContent = job.running ? name + ' is running…' : name + (job.ok ? ' finished' : ' finished with problems');
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
  if (job.result && !job.running) summaryOf(job).forEach(function (t) { summary.appendChild(pill(t)); });
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

function summaryOf(job) {
  var r = job.result, out = [];
  if (job.kind === 'scan') out.push(r.added + ' added', r.moved + ' moved', r.changed + ' changed', r.missing + ' missing');
  else if (job.kind === 'verify') out.push(r.checked + ' checked', r.missing.length + ' missing', r.damaged.length + ' damaged');
  else if (job.kind === 'recognize' && r.faces !== undefined) out.push(r.faces + ' faces');
  else if (job.kind === 'faces_stats') out.push(r === null ? 'no faces yet' : (r.faces + ' faces'));
  return out;
}

function appState() {
  api('/api/app').then(function (s) {
    $('stop-app').hidden = !s.running;
    $('start-app').textContent = s.running ? 'Open photo app' : 'Start photo app';
    if (s.running) $('app-state').innerHTML = '';
    if (s.running) {
      var a = document.createElement('a'); a.href = s.url; a.target = '_blank'; a.rel = 'noopener'; a.textContent = s.url;
      $('app-state').textContent = 'Running for ' + s.root + ' at ';
      $('app-state').appendChild(a);
    }
  });
}

document.querySelectorAll('.actions button').forEach(function (b) {
  b.onclick = function () { startJob(b.dataset.kind); };
});
$('only-failed').onchange = poll;
$('root').onchange = rememberRoot;
$('start-app').onclick = function () {
  if (!root()) { $('root').focus(); return; }
  rememberRoot();
  api('/api/app', { root: root() }).then(function (r) {
    appState();
    window.open(r.url, '_blank', 'noopener');
  }).catch(function (e) { $('app-state').textContent = e.message; });
};
$('stop-app').onclick = function () { api('/api/app/stop', {}).then(function () { appState(); $('app-state').textContent = 'Photo app stopped.'; }); };

loadDrives();
appState();
poll();
