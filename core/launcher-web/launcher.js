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
var drivesLoaded = false;
var runningKind = '';    // the kind of the command that runs
var busy = false;        // a command is running
var starting = false;    // the photo app was asked to start and is not up yet
var serving = false;    // the photo app is running: only recognition may still run

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

// A folder as typed, without a trailing separator ("C:" stays "C:\\", "/" stays "/").
function cleanPath(value) {
  value = value.trim();
  var cut = value.replace(/[\\\/]+$/, '');
  if (!cut) return value;
  return /^[A-Za-z]:$/.test(cut) ? cut + '\\' : cut;
}

function addPath(value) {
  value = cleanPath(value);
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

// Windows paths with a drive letter: `E:\`, `e:\`, `E:` and `E:/` are one
// place. The key has the letter in capitals, `\` as separator and no trailing
// one ("E:" for the top of the drive); null for any other path (macOS and
// Linux compare as written, a volume can be case-sensitive there).
function winKey(p) {
  if (!/^[A-Za-z]:([\\\/]|$)/.test(p)) return null;
  var rest = p.slice(2).replace(/\//g, '\\').replace(/\\+$/, '');
  return p.charAt(0).toUpperCase() + ':' + rest;
}

function samePath(a, b) {
  var ka = winKey(a), kb = winKey(b);
  return ka !== null && kb !== null ? ka === kb : a === b;
}

// A drive that the list of connected drives does not have: under /Volumes
// (macOS), or with a letter that no listed drive has (Windows). A photo folder
// on a drive (/Volumes/X/Photos, E:\Photos) is there when the drive
// (/Volumes/X, E:\) is in the list.
function isOffline(p) {
  if (!drivesLoaded) return false;
  var key = winKey(p);
  if (key !== null) {
    return !drives.some(function (d) { var k = winKey(d.path); return k !== null && k.charAt(0) === key.charAt(0); });
  }
  return p.indexOf('/Volumes/') === 0 && !drives.some(function (d) {
    return d.path === p || p.indexOf(d.path.replace(/\/+$/, '') + '/') === 0;
  });
}

// The photo app cannot start on a drive that is not there: the button stays
// off, with the reason in red under it, until it is unticked or plugged in.
function refreshStart() {
  var off = tickedPaths().filter(isOffline);
  var block = off.length > 0 && !serving;
  $('start-app').disabled = starting || block || (busy && BESIDE_APP.indexOf(runningKind) < 0);
  $('app-offline').hidden = !block;
  $('app-offline').textContent = block ? tr('launcher.app.offline', { names: off.map(driveName).join(', ') }) : '';
}

function renderChips() {
  var ul = $('path-chips');
  ul.textContent = '';
  paths.forEach(function (p) {
    var li = document.createElement('li');
    var known = drives.filter(function (d) { return samePath(d.path, p); })[0];
    if (isOffline(p)) li.className = 'offline';
    if (off[p]) li.className += ' unticked';
    var cb = document.createElement('input');
    cb.type = 'checkbox'; cb.checked = !off[p];
    cb.setAttribute('aria-label', tr('launcher.tick', { path: p })); cb.title = tr('launcher.tick', { path: p });
    cb.onchange = function () {
      if (cb.checked) delete off[p]; else off[p] = true;
      renderChips();
      savePaths();
    };
    var name = document.createElement('span'); name.className = 'name'; name.textContent = p; name.title = p + ' · ' + tr('launcher.chip.edit');
    name.ondblclick = function () { editChip(li, p); };
    li.appendChild(cb); li.appendChild(name);
    var mark = backups[p];
    if (mark) {
      var b = document.createElement('span'); b.className = 'badge';
      b.textContent = tr('launcher.backup_of', { name: driveName(mark.of) });
      b.title = tr('launcher.backup_of.title', { path: mark.of, date: I18n.date(new Date(mark.at * 1000)) });
      li.appendChild(b);
    }
    var lib = libOfPath(p);
    if (lib) li.appendChild(sepBadge(lib.name));
    var x = document.createElement('button');
    x.type = 'button'; x.textContent = '×'; x.title = tr('launcher.remove'); x.setAttribute('aria-label', tr('launcher.remove_path', { path: p }));
    x.onclick = function () { removePath(p); };
    li.appendChild(x);
    ul.appendChild(li);
  });
  $('root').placeholder = tr(paths.length ? 'launcher.root.placeholder_more' : 'launcher.root.placeholder');
  updateButtons();
}

// Double-click a chip to change its path, for instance to add a folder to a drive.
function editChip(li, p) {
  var nameEl = li.querySelector('.name');
  if (!nameEl) return;
  var inp = document.createElement('input');
  inp.value = p; inp.className = 'chip-edit'; inp.size = Math.max(24, p.length + 2);
  inp.setAttribute('aria-label', tr('launcher.chip.edit'));
  nameEl.replaceWith(inp);
  inp.focus(); inp.select();
  var done = false;
  var finish = function (save) {
    if (done) return;
    done = true;
    var v = cleanPath(inp.value);
    if (save && v && v !== p && paths.indexOf(v) < 0) {
      paths[paths.indexOf(p)] = v;
      if (off[p]) { off[v] = true; delete off[p]; }
      savePaths();
    }
    renderChips();
  };
  inp.onkeydown = function (ev) { if (ev.key === 'Enter') finish(true); else if (ev.key === 'Escape') finish(false); };
  inp.onblur = function () { finish(true); };
}

function updateButtons() {
  var n = tickedPaths().length;
  refreshActions();
  refreshStart();
  $('backup-hint').textContent = n === 0 ? tr('launcher.need_ticked') : n !== 2 ? tr('launcher.backup_need_two') : '';
  loadAddons();
}

// ---------------------------------------------------------------- add-ons

// What the recognition buttons need: faces (Recognize, lying down), pets or text.
var ADDON_OF = { recognize: 'faces', recognize_rotated: 'faces', recognize_pets: 'pets', recognize_text: 'text' };
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
  // Ready, or the models are on the drive but this kind of computer lacks its Python, or missing.
  var addonState = function (id, ready, models) {
    var s = $(id);
    s.textContent = tr(ready ? 'launcher.addon.installed' : models ? 'launcher.addon.needs_runtime' : 'launcher.addon.missing');
    s.className = 'pill' + (ready ? ' ok' : '');
  };
  addonState('addon-faces-state', addons.faces, addons.faces_models);
  addonState('addon-pets-state', addons.pets, addons.pets_models);
  addonState('addon-text-state', addons.text, addons.text_models);
  // Each add-on stands alone; an installed one is ticked and cannot be ticked off.
  ['faces', 'pets', 'text'].forEach(function (k) {
    var box = $('addon-' + k);
    box.checked = box.checked || addons[k];
    box.disabled = busy || serving || addons[k];
  });
  $('addons-install').disabled = busy || serving || !addons.installable || wantedAddons().length === 0;
  $('addons-hint').textContent = !addons.installable ? tr('launcher.addons.not_installable')
    : addons.models && !addons.runtime ? tr('launcher.addons.runtime_missing', { dir: addons.dir })
    : addons.faces && addons.pets && addons.text ? tr('launcher.addons.all_installed', { dir: addons.dir })
    : tr('launcher.addons.where', { dir: addons.dir });
  // The folded card still says where things stand.
  var word = function (ready, models) { return tr(ready ? 'launcher.addon.installed' : models ? 'launcher.addon.needs_runtime' : 'launcher.addon.missing'); };
  $('addons-summary').textContent = [
    tr('launcher.addon.faces') + ': ' + word(addons.faces, addons.faces_models),
    tr('launcher.addon.pets') + ': ' + word(addons.pets, addons.pets_models),
    tr('launcher.addon.text') + ': ' + word(addons.text, addons.text_models),
  ].join(' · ');
  // Why a recognition button is grey.
  var why = [];
  if (addonMissing('recognize').length) why.push(tr('launcher.addons.need_faces'));
  if (addonMissing('recognize_pets').length) why.push(tr('launcher.addons.need_pets'));
  if (addonMissing('recognize_text').length) why.push(tr('launcher.addons.need_text'));
  $('addon-need').textContent = why.join(' ');
  // Both ways: a button whose add-on was just installed can be pressed again.
  refreshActions();
}

// The folders are looked at again whenever the page comes back to the front
// (add-ons installed or deleted in the file manager meanwhile).
document.addEventListener('visibilitychange', function () { if (!document.hidden) loadAddons(); });
window.addEventListener('focus', loadAddons);

// The add-ons ticked that are not installed yet.
function wantedAddons() {
  return ['faces', 'pets', 'text'].filter(function (k) { return $('addon-' + k).checked && !(addons && addons[k]); });
}
$('addon-faces').onchange = $('addon-pets').onchange = $('addon-text').onchange = renderAddons;
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
  $('addons-details').open = !(addons.runtime && addons.faces && addons.pets && addons.text);
  setTimeout(function () { settingDefault = false; }, 0);
}

$('addons-install').onclick = function () {
  var want = wantedAddons();
  api('/api/job', { kind: 'install_addons', faces: want.indexOf('faces') >= 0, pets: want.indexOf('pets') >= 0, text: want.indexOf('text') >= 0 }).then(function () {
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
  if (v && (drives.some(function (d) { return samePath(d.path, v); }) || recentPaths.indexOf(v) >= 0)) { addPath(v); $('root').value = ''; }
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

// The list of connected drives is read again while the page is open (every 15
// seconds and when the window is used again): a drive that is plugged in or
// mounted after the page opened, or while a command runs, must not keep the
// photo app blocked as "not connected". Reading it looks at every volume (and
// its .shoebox folder), so only one read is under way at a time.
var drivesReading = false;
function loadDrives(quiet) {
  if (drivesReading) return Promise.resolve();
  drivesReading = true;
  return api('/api/drives').then(function (list) {
    drivesReading = false;
    if (quiet === true && JSON.stringify(list) === JSON.stringify(drives)) return;
    drives = list;
    drivesLoaded = true;
    var chips = $('drive-chips');
    chips.textContent = '';
    renderDatalist();
    drives.forEach(function (d) {
      var b = document.createElement('button');
      b.type = 'button'; b.textContent = d.name; b.title = d.path;
      if (d.library) b.className = 'has-lib';
      b.onclick = function () { startPath(d.path); };
      chips.appendChild(b);
    });
    $('root-hint').textContent = drives.length
      ? tr('launcher.drives.has_lib')
      : tr('launcher.drives.none');
    renderChips();
  }).catch(function (e) { drivesReading = false; if (quiet !== true) $('root-hint').textContent = String(e.message); });
}

// A drive is rarely the photo folder itself: put its path in the box to finish
// it (Enter alone adds the drive).
function startPath(base) {
  var sep = /\\|^[A-Za-z]:/.test(base) ? '\\' : '/';
  var inp = $('root');
  inp.value = base.replace(/[\\\/]+$/, '') + sep;
  inp.focus();
  inp.setSelectionRange(inp.value.length, inp.value.length);
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

// ---------------------------------------------------------------- libraries next to the program (phase 16)
// A library can keep its index next to the program instead of in the folder:
// nothing is written into the folder, so it is only read (docs/phase16.md).

var libs = [];                           // /api/libraries: the registered libraries
var libsInfo = { dir: null, writable: false };
var libEdit = null;                      // { name, mode: 'rename' | 'root' | 'remove' | 'delete', check, error }
var libsShown = '';

// The photo app's own tab icon (core/web/index.html): a box with a lid.
var SHOEBOX_ICON = '<svg class="sbx" viewBox="0 0 16 16" aria-hidden="true"><rect x="1" y="5" width="14" height="10" rx="1" fill="#b07d4f"/><rect x="0" y="2" width="16" height="4" rx="1" fill="#d9a066"/></svg>';

function sameFolder(a, b) {
  var n = function (p) { return p.trim().replace(/[\\\/]+$/, ''); };
  return n(a) === n(b);
}

function libOfPath(p) {
  return libs.filter(function (l) { return sameFolder(l.root, p); })[0];
}

function sepBadge(name) {
  var b = document.createElement('span');
  b.className = 'badge sep';
  b.title = tr('launcher.libs.badge.title', { name: name });
  b.innerHTML = SHOEBOX_ICON;
  b.appendChild(document.createTextNode(tr('launcher.libs.badge')));
  return b;
}

function libsDir() {
  return libsInfo.dir ? libsInfo.dir.replace(/[\\\/]+$/, '') + (libsInfo.dir.indexOf('\\') >= 0 ? '\\libraries' : '/libraries') : '';
}

function el(tag, cls, text) {
  var e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function button(text, cls, onclick) {
  var b = el('button', cls, text);
  b.type = 'button';
  b.onclick = onclick;
  return b;
}

function loadLibs(quiet) {
  return api('/api/libraries').then(function (d) {
    var shown = JSON.stringify(d);
    if (quiet === true && shown === libsShown) return;
    libsShown = shown;
    libs = d.libraries || [];
    libsInfo = { dir: d.dir, writable: !!d.writable };
    $('libs-error').hidden = true;
    renderLibs();
    renderChips();
  }).catch(function (e) {
    // The list of drives still works; a damaged list of libraries says so (the server's message names the file).
    $('libs-error').hidden = false;
    $('libs-error').textContent = e.message;
  });
}

function renderLibs() {
  var box = $('lib-chips');
  box.textContent = '';
  libs.forEach(function (l) {
    var b = button('', 'lib' + (l.online ? '' : ' off'), function () { addPath(l.root); });
    b.title = l.root + (l.online ? '' : ' · ' + tr('launcher.libs.offline'));
    b.appendChild(el('span', '', l.name));
    b.appendChild(sepBadge(l.name));
    box.appendChild(b);
  });
  $('libs-head').hidden = !libs.length;
  $('libs-none').hidden = libs.length > 0;
  renderLibTabs();
  renderManage();
  syncWritable();
}

// ---- tabs: Photo folders, Add a folder or server, and (once there is a library) Manage libraries

var libTab = 'folders';
var libsReloadedFor = -1;   // the job whose end already refreshed the list

function showLibTab(name) {
  if (name === 'manage' && !libs.length) name = 'folders';
  libTab = name;
  ['folders', 'add', 'manage'].forEach(function (t) {
    $('panel-' + t).hidden = t !== name;
    $('tab-' + t).setAttribute('aria-selected', t === name ? 'true' : 'false');
  });
}

function renderLibTabs() {
  var m = $('tab-manage');
  m.hidden = !libs.length;
  m.textContent = tr('launcher.tab.manage', { n: libs.length });
  if (libTab === 'manage' && !libs.length) showLibTab('folders');
}

document.querySelectorAll('#libs-tabs button').forEach(function (b) {
  b.onclick = function () {
    showLibTab(b.dataset.tab);
    if (b.dataset.tab === 'add') $('add-root').focus();
    if (b.dataset.tab === 'manage') loadLibs(true);
  };
});

// ---- dialogs: everything else is greyed out behind them

// actions: [{ label, cls, onclick(ctl) }]; ctl.close() and ctl.fail(message) are
// for work that finishes later (return false to keep the dialog open meanwhile).
function openDialog(title, body, actions, cls) {
  var dlg = document.createElement('dialog');
  dlg.className = 'dlg' + (cls ? ' ' + cls : '');
  dlg.appendChild(el('h3', '', title));
  dlg.appendChild(body);
  var err = el('p', 'error'); err.hidden = true;
  dlg.appendChild(err);
  var ctl = { close: function () { dlg.close(); }, fail: function (m) { err.hidden = !m; err.textContent = m || ''; } };
  var row = el('div', 'btnrow');
  actions.forEach(function (a) {
    row.appendChild(button(a.label, a.cls || '', function () { if (a.onclick(ctl) !== false) dlg.close(); }));
  });
  dlg.appendChild(row);
  dlg.addEventListener('close', function () { dlg.remove(); });
  document.body.appendChild(dlg);
  dlg.showModal();
  return dlg;
}

function folderIcon() {
  var s = document.createElement('span');
  s.innerHTML = '<svg class="fld" viewBox="0 0 16 16" aria-hidden="true"><path d="M1.5 4.5a1 1 0 0 1 1-1h3.2l1.3 1.5h6.5a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1z"/></svg>';
  return s.firstChild;
}

var PLACE_NAMES = {
  downloads: 'export.place_downloads',
  desktop: 'export.place_desktop',
  documents: 'export.place_documents',
  pictures: 'export.place_pictures',
  home: 'export.place_home',
};

// The computer's own folders, listed by the launcher (a browser cannot give a
// page a real folder path). Same lists as the photo app's export dialog.
function pickFolder(start, onChoose) {
  var current = null;
  var body = el('div');
  var pk = el('div', 'x-pk'), nav = el('nav'), main = el('div', 'x-main'), crumbs = el('div', 'x-crumbs'), list = el('ul');
  main.appendChild(crumbs); main.appendChild(list); pk.appendChild(nav); pk.appendChild(main);
  body.appendChild(pk);
  var selected = el('p', 'hint x-selected');
  body.appendChild(selected);
  var error = el('p', 'error'); error.hidden = true;
  body.appendChild(error);
  var navItems = [];
  var addNav = function (label, path, extra) {
    var d = el('div', '', label);
    if (extra) d.appendChild(el('small', '', extra));
    d.dataset.path = path;
    d.onclick = function () { show(path); };
    nav.appendChild(d);
    navItems.push(d);
  };
  var show = function (path) {
    return api('/api/folders?path=' + encodeURIComponent(path)).then(function (l) {
      error.hidden = true;
      current = l;
      navItems.forEach(function (d) { d.classList.toggle('on', d.dataset.path === l.path); });
      crumbs.textContent = '';
      l.crumbs.forEach(function (c, i) {
        if (i) crumbs.appendChild(document.createTextNode(' › '));
        var a = el('button', 'link', c.name);
        a.type = 'button';
        a.onclick = function () { show(c.path); };
        crumbs.appendChild(a);
      });
      list.textContent = '';
      if (!l.folders.length) list.appendChild(el('li', 'x-none', tr('export.empty_folder')));
      l.folders.forEach(function (f) {
        var li = el('li');
        var nm = el('span');
        nm.appendChild(folderIcon());
        nm.appendChild(document.createTextNode(f.name));
        li.appendChild(nm);
        if (f.library) li.appendChild(el('small', '', tr('export.shoebox_tag')));
        li.onclick = function () { show(f.path); };
        list.appendChild(li);
      });
      selected.textContent = tr('export.selected', { name: l.crumbs[l.crumbs.length - 1].name });
    });
  };
  var cancel = { label: tr('launcher.libs.cancel'), onclick: function () {} };
  openDialog(tr('launcher.pick.title'), body, [
    cancel,
    { label: tr('export.use_folder'), cls: 'primary', onclick: function () { if (!current) return false; onChoose(current.path); } },
  ], 'wide');
  api('/api/places').then(function (pl) {
    nav.appendChild(el('h4', '', tr('export.places')));
    pl.places.forEach(function (p) { addNav(tr(PLACE_NAMES[p.kind] || 'export.place_home'), p.path); });
    if (pl.drives.length) {
      nav.appendChild(el('h4', '', tr('export.drives')));
      pl.drives.forEach(function (d) { addNav(d.name, d.path, d.library ? tr('export.shoebox_tag') : ''); });
    }
    var first = pl.places[0] ? pl.places[0].path : '/';
    return (start && start.trim() ? show(start.trim()).catch(function () { return show(first); }) : show(first));
  }).catch(function (e) { error.hidden = false; error.textContent = e.message; });
}

// ---- add a folder or server

function baseName(p) {
  return p.trim().replace(/[\\\/]+$/, '').split(/[\\\/]/).pop() || '';
}

var addNameTouched = false;

// "Separate" needs a program folder that can be written to; otherwise it is
// greyed out with the reason.
function syncWritable() {
  var ok = libsInfo.writable;
  document.querySelector('input[name=where][value=separate]').disabled = !ok;
  $('choice-separate').classList.toggle('disabled', !ok);
  $('add-separate-desc').textContent = tr('launcher.libs.add.separate.desc', { dir: libsDir() });
  $('add-unwritable').hidden = ok;
  $('add-unwritable').textContent = ok ? '' : tr('launcher.libs.add.unwritable', { dir: libsInfo.dir || '' });
  if (!ok && chosenWhere() === 'separate') { document.querySelector('input[name=where][value=drive]').checked = true; syncAdd(); }
}

function resetAdd() {
  $('add-root').value = '';
  $('add-name').value = '';
  addNameTouched = false;
  $('add-error').hidden = true;
  document.querySelector('input[name=where][value=drive]').checked = true;
  syncAdd();
}

function chosenWhere() {
  return document.querySelector('input[name=where]:checked').value;
}

function syncAdd() {
  var sep = chosenWhere() === 'separate';
  $('choice-drive').classList.toggle('on', !sep);
  $('choice-separate').classList.toggle('on', sep);
  $('add-name-field').hidden = !sep;
  $('add-note').hidden = !sep;
  if (sep && !addNameTouched) $('add-name').value = baseName($('add-root').value);
}

document.querySelectorAll('input[name=where]').forEach(function (r) { r.onchange = syncAdd; });
$('add-root').oninput = syncAdd;
$('add-name').oninput = function () { addNameTouched = true; };
$('add-pick').onclick = function () { pickFolder($('add-root').value, function (p) { $('add-root').value = p; syncAdd(); }); };
$('root-pick').onclick = function () { pickFolder($('root').value, function (p) { addPath(p); }); };
$('add-cancel').onclick = function () { resetAdd(); showLibTab('folders'); };
$('add-go').onclick = function () {
  var root = $('add-root').value.trim();
  if (!root) { $('add-root').focus(); return; }
  if (chosenWhere() === 'drive') { addPath(root); resetAdd(); showLibTab('folders'); return; }
  api('/api/libraries', { root: root, name: $('add-name').value.trim() }).then(function (d) {
    addPath(d.root);
    resetAdd();
    return loadLibs().then(function () { showLibTab('folders'); });
  }).catch(function (e) { $('add-error').hidden = false; $('add-error').textContent = e.message; });
};

// ---- manage

function stateText(l) {
  if (!l.online) return tr('launcher.libs.offline');
  return tr('launcher.libs.online') + ' · ' + (l.photos == null ? tr('launcher.libs.unscanned') : trn('launcher.libs.photos', l.photos));
}

function editLib(name, mode) { libEdit = { name: name, mode: mode, check: null, error: '' }; renderManage(); }

function libCall(path, body, done) {
  api(path, body).then(function (d) { libEdit = null; done(d); return loadLibs().then(renderManage); })
    .catch(function (e) { if (libEdit) libEdit.error = e.message; renderManage(); });
}

function renderManage() {
  var box = $('panel-manage');
  box.textContent = '';
  if (!libs.length) return;
  box.appendChild(el('p', 'hint first', tr('launcher.libs.where', { dir: libsDir() })));
  var ul = el('ul', 'libs');
  libs.forEach(function (l) {
    var li = el('li');
    var top = el('div', 'top');
    var who = el('div');
    who.appendChild(el('div', 'lname', l.name));
    who.appendChild(el('div', 'lpath', l.root));
    top.appendChild(who);
    top.appendChild(el('span', 'state' + (l.online ? '' : ' off'), stateText(l)));
    li.appendChild(top);
    var ed = libEdit && libEdit.name === l.name ? libEdit : null;
    if (!ed) {
      var acts = el('div', 'acts');
      acts.appendChild(button(tr('launcher.libs.rename'), '', function () { editLib(l.name, 'rename'); }));
      acts.appendChild(button(tr('launcher.libs.change'), '', function () { editLib(l.name, 'root'); }));
      acts.appendChild(button(tr('launcher.libs.remove'), '', function () { confirmRemove(l); }));
      acts.appendChild(button(tr('launcher.libs.delete'), 'danger', function () { confirmDelete(l); }));
      li.appendChild(acts);
      if (!l.online) li.appendChild(el('p', 'note warn', tr('launcher.libs.offline.note', { root: l.root })));
    } else {
      renderEdit(li, l, ed);
    }
    ul.appendChild(li);
  });
  box.appendChild(ul);
}

// The root of a library that goes away leaves the list of folders to scan too:
// a scan of it would make a .shoebox folder in it.
function forgetRoot(root) {
  var p = paths.filter(function (x) { return sameFolder(x, root); })[0];
  if (p) removePath(p);
}

function confirmRemove(l) {
  openDialog(tr('launcher.libs.remove'), el('p', '', tr('launcher.libs.remove.confirm', { name: l.name })), [
    { label: tr('launcher.libs.cancel'), onclick: function () {} },
    { label: tr('launcher.libs.remove.go'), cls: 'primary', onclick: function (ctl) {
      api('/api/libraries/remove', { name: l.name }).then(function () { forgetRoot(l.root); return loadLibs(); }).then(ctl.close).catch(function (e) { ctl.fail(e.message); });
      return false;
    } },
  ]);
}

function confirmDelete(l) {
  openDialog(tr('launcher.libs.delete.title', { name: l.name }), el('p', '', tr('launcher.libs.delete.text')), [
    { label: tr('launcher.libs.cancel'), onclick: function () {} },
    { label: tr('launcher.libs.delete.go'), cls: 'danger', onclick: function (ctl) {
      api('/api/libraries/delete', { name: l.name }).then(function () { forgetRoot(l.root); return loadLibs(); }).then(ctl.close).catch(function (e) { ctl.fail(e.message); });
      return false;
    } },
  ]);
}

function renderEdit(li, l, ed) {
  var cancel = button(tr('launcher.libs.cancel'), '', function () { libEdit = null; renderManage(); });
  var row = el('div', 'btnrow');
  if (ed.mode === 'rename') {
    var inp = el('input'); inp.value = l.name; inp.setAttribute('aria-label', tr('launcher.libs.name_label'));
    var wrap = el('div', 'edit'); wrap.appendChild(inp); li.appendChild(wrap);
    row.appendChild(cancel);
    row.appendChild(button(tr('launcher.libs.save'), 'primary', function () {
      libCall('/api/libraries/rename', { name: l.name, new_name: inp.value }, function () {});
    }));
    setTimeout(function () { inp.focus(); inp.select(); }, 0);
  } else if (ed.mode === 'root') {
    var root = el('input'); root.value = ed.value !== undefined ? ed.value : l.root; root.setAttribute('aria-label', tr('launcher.libs.root_label'));
    var wrap2 = el('div', 'edit'); wrap2.appendChild(root);
    var check = function () {
      ed.value = root.value;
      api('/api/libraries/root', { name: l.name, root: root.value, check: true }).then(function (r) { ed.check = r; ed.error = ''; renderManage(); })
        .catch(function (e) { ed.check = null; ed.error = e.message; renderManage(); });
    };
    wrap2.appendChild(button(tr('launcher.pick.button'), '', function () { pickFolder(root.value, function (p) { root.value = p; check(); }); }));
    wrap2.appendChild(button(tr('launcher.libs.check'), '', check));
    li.appendChild(wrap2);
    var r = ed.check;
    if (r) {
      if (r.checked === 0) li.appendChild(el('p', 'note', tr('launcher.libs.check.new')));
      else if (r.found === 0) li.appendChild(el('p', 'note warn', tr('launcher.libs.check.none', { checked: r.checked })));
      else li.appendChild(el('p', 'note ok', tr('launcher.libs.check.ok', { found: r.found, checked: r.checked, total: r.total })));
    }
    var save = function (force) {
      var old = l.root;
      libCall('/api/libraries/root', { name: l.name, root: root.value, force: force }, function () {
        paths = paths.map(function (p) { return sameFolder(p, old) ? root.value.trim() : p; });
        renderChips(); savePaths();
      });
    };
    row.appendChild(cancel);
    row.appendChild(button(tr(r && r.checked > 0 && r.found === 0 ? 'launcher.libs.use_anyway' : 'launcher.libs.use'), 'primary', function () { save(!!(r && r.checked > 0 && r.found === 0)); }));
  }
  if (ed.error) li.appendChild(el('p', 'error', ed.error));
  li.appendChild(row);
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
var BESIDE_APP = ['recognize', 'recognize_pets', 'recognize_rotated', 'recognize_text', 'recognition_stats'];

// Every action works on the ticked folders (the backup check on exactly two)
// and needs the add-on it runs (`addons`). One rule for every caller, so a
// change to any of these (a tick, a job ending, the photo app, an install)
// turns a button on as well as off.
function refreshActions() {
  var n = tickedPaths().length;
  document.querySelectorAll('.actions button').forEach(function (b) {
    var kind = b.dataset.kind;
    b.disabled = n === 0 || (kind === 'backup' && n !== 2) || busy
      || (serving && BESIDE_APP.indexOf(kind) < 0) || addonMissing(kind).length > 0;
  });
}

function applyLocks() {
  $('controls').disabled = busy || serving;
  refreshActions();
  renderAddons();
  $('opt-quick').disabled = $('opt-deep').disabled = busy || serving;
  refreshStart();
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
  // A finished scan or recognition changes the photo counts of the libraries.
  if (!job.running && libsReloadedFor !== job.id) { libsReloadedFor = job.id; loadLibs(true); }
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
  else if (kind === 'recognize_text' && r.text) out.push(tr('launcher.sum.text', { n: r.text.faces }), tr('launcher.failed', { n: r.text.failed }));
  else if (kind === 'recognition_stats') {
    if (!r || !r.started) out.push(tr('launcher.sum.nothing_yet'));
    else {
      if (r.faces.looked) out.push(tr('launcher.sum.faces', { n: r.faces.finds }));
      if (r.pets.looked) out.push(tr('launcher.sum.pets', { n: r.pets.finds }));
      if (r.text.looked) out.push(tr('launcher.sum.text', { n: r.text.finds }));
    }
  }
  return out;
}

function setStarting(on) {
  starting = on;
  $('app-spinner').hidden = !on;
  if (on) $('app-state').textContent = tr('launcher.app.starting');
  else if (!serving && !$('app-state').classList.contains('error')) $('app-state').textContent = tr('launcher.app.idle');
  refreshStart();
}

function appState() {
  return api('/api/app').then(function (s) {
    serving = !!s.running;
    $('app-state').classList.remove('error');
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
  // The server opens the browser itself when it starts the app. Until the
  // stop button shows, the start button stays off and a spinner is shown.
  setStarting(true);
  api('/api/app', { roots: roots }).then(appState).catch(function (e) { $('app-state').textContent = e.message; $('app-state').classList.add('error'); })
    .then(function () { setStarting(false); });
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
  loadConfig().then(function () { return Promise.all([loadDrives(), loadLibs()]); });
  setInterval(function () { if (!document.hidden) { loadDrives(true); loadLibs(true); } }, 15000);
  window.addEventListener('focus', function () { loadDrives(true); loadLibs(true); });
  appState();
  poll();
});
