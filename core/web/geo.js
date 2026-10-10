// shoebox web UI, phase 10: locations. The map in a photo's info, setting a
// location by hand, the Locations page (clustered pins, named places drawn as
// rectangles) and the Maps setting. Loaded after app.js and uses its helpers.
// Maps are Leaflet with OpenStreetMap tiles, loaded only when the Maps setting
// is on; the browser fetches the tiles, the server stores none.
'use strict';

var geo = {
  on: false,        // the Maps setting
  places: [],       // [{id, name, south, west, north, east, count}]
  mini: null,       // the map in the info panel
  map: null,        // the map of the Locations page
  page: null,       // its root element
  sel: null,        // {kind: 'place', id} | {kind: 'area', area, count} | null
  data: null,       // timeline columns of the selection
  shown: 0,
  rects: {},        // place id -> rectangle layer
  selRect: null,
  drawing: null,
  pendingDraw: false,
  side: true,
};
try { geo.side = localStorage.getItem('geoSide') !== '0'; } catch (e) { /* ignore */ }
geo.on = readMapsSetting(); // also on the screens that load no library (All drives, Settings)

var TILES = 'https://tile.openstreetmap.org/{z}/{x}/{y}.png';
var OSM_ATTRIBUTION = '© <a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noopener">OpenStreetMap</a>';

// ------------------------------------------------------------------ leaflet

var leafletLoading = null;

// Leaflet and its cluster plug-in come with shoebox (vendor/), and are only
// loaded when a map is first needed.
function leafletReady() {
  if (window.L && L.markerClusterGroup) return Promise.resolve();
  if (leafletLoading) return leafletLoading;
  var css = function (href) {
    var l = document.createElement('link');
    l.rel = 'stylesheet';
    l.href = href;
    document.head.appendChild(l);
  };
  var js = function (src) {
    return new Promise(function (ok, fail) {
      var s = document.createElement('script');
      s.src = src;
      s.onload = ok;
      s.onerror = function () { fail(new Error(src)); };
      document.head.appendChild(s);
    });
  };
  css('vendor/leaflet/leaflet.css');
  css('vendor/markercluster/MarkerCluster.css');
  css('vendor/markercluster/MarkerCluster.Default.css');
  leafletLoading = js('vendor/leaflet/leaflet.js').then(function () {
    L.Icon.Default.imagePath = 'vendor/leaflet/images/';
    return js('vendor/markercluster/leaflet.markercluster.js');
  }).catch(function (e) { leafletLoading = null; throw e; });
  return leafletLoading;
}

// A map with the OpenStreetMap layer and an "offline" label that shows when no
// tile of the view could be loaded (no internet), and goes when one can.
function newMap(box, opts) {
  var map = L.map(box, Object.assign({ attributionControl: false, keyboard: false }, opts));
  L.control.attribution({ prefix: false }).addTo(map);
  var label = el('div', 'geo-offline', tr('geo.offline'));
  label.hidden = true;
  box.appendChild(label);
  var ok = 0, bad = 0;
  var layer = L.tileLayer(TILES, {
    maxZoom: 19,
    attribution: OSM_ATTRIBUTION,
    // The OSM tile servers answer a request without a Referer with an "Access
    // blocked" tile (403, still a PNG, so no error shows). The pages are served
    // with Referrer-Policy: no-referrer; for tiles only send the origin.
    referrerPolicy: 'origin',
  });
  layer.on('loading', function () { ok = 0; bad = 0; });
  layer.on('tileload', function () { ok++; label.hidden = true; });
  layer.on('tileerror', function () { bad++; });
  layer.on('load', function () { label.hidden = !(bad > 0 && ok === 0) && navigator.onLine !== false; });
  layer.addTo(map);
  if (navigator.onLine === false) label.hidden = false;
  var back = function () { layer.redraw(); };
  var gone = function () { label.hidden = false; };
  window.addEventListener('online', back);
  window.addEventListener('offline', gone);
  map.on('unload', function () {
    window.removeEventListener('online', back);
    window.removeEventListener('offline', gone);
  });
  return map;
}

// ------------------------------------------------------------------ numbers

function fmtCoord(v, pos, neg) {
  return Math.abs(v).toFixed(5) + '° ' + (v >= 0 ? pos : neg);
}

// Latitude and longitude as typed: a decimal number (a comma works as the
// point), within range; null when it is not.
function parseCoord(text, limit) {
  var t = String(text).trim().replace('−', '-').replace(',', '.');
  if (!/^[-+]?\d+(\.\d+)?$|^[-+]?\.\d+$/.test(t)) return null;
  var v = Number(t);
  return isFinite(v) && Math.abs(v) <= limit ? v : null;
}

// ------------------------------------------------------------------ setting

// The Maps setting belongs to the application, not to a drive: the browser
// keeps it, so it is the same whichever drive is shown.
function readMapsSetting() {
  try { return localStorage.getItem('shoebox-maps') === '1'; } catch (e) { return false; }
}

function writeMapsSetting(on) {
  try { localStorage.setItem('shoebox-maps', on ? '1' : '0'); } catch (e) { /* ignore */ }
}

function loadGeo() {
  geo.on = readMapsSetting();
  return loadPlaces().catch(function () {}); // also with maps off: a place in the search keeps its name
}

function mapsSection() {
  var sec = el('section', 'settings-section');
  sec.appendChild(el('h3', '', tr('settings.maps')));
  var label = el('label', 'check-row');
  var box = el('input');
  box.type = 'checkbox';
  box.checked = geo.on;
  box.onchange = function () {
    geo.on = box.checked;
    writeMapsSetting(geo.on);
    if (geo.on) loadPlaces(); else renderLocationsSection();
    toast(tr('settings.maps_saved'));
  };
  label.appendChild(box);
  label.appendChild(document.createTextNode(' ' + tr('settings.maps_label')));
  sec.appendChild(label);
  sec.appendChild(el('p', 'hint', tr('settings.maps_hint')));
  return sec;
}

// ------------------------------------------------------------------ info panel

function geoCleanup() {
  if (geo.mini) { geo.mini.remove(); geo.mini = null; }
}

// "GPS data" in a photo's info: the position (or, when there is none, a button
// to give one) and, where there is one and maps are on, a small map with a pin,
// fixed to the bottom of the panel.
function infoGeo(info) {
  geoCleanup();
  var pos = info.position;
  var box = el('div', 'info-geo');
  var head = el('div', 'geo-head');
  head.appendChild(el('h3', '', tr('geo.title')));
  var right = el('div', 'geo-right');
  if (pos) {
    var c = el('span', 'coords', fmtCoord(pos.lat, 'N', 'S') + ' · ' + fmtCoord(pos.lon, 'E', 'W'));
    if (pos.source === 'user') c.title = tr('geo.from_user');
    right.appendChild(c);
  }
  // Only a position that is not in the file can be given or changed.
  if (!pos || pos.source === 'user') {
    var b = el('button', 'btn quiet small', tr(pos ? 'geo.change' : 'geo.set'));
    b.type = 'button';
    b.onclick = function () { locationDialog(info); };
    right.appendChild(b);
  }
  head.appendChild(right);
  box.appendChild(head);
  if (pos && geo.on) {
    var square = el('div', 'geo-square');
    var mapBox = el('div', 'geo-mini');
    square.appendChild(mapBox);
    box.appendChild(square);
    leafletReady().then(function () {
      if (!mapBox.isConnected) return;
      var map = newMap(mapBox, { scrollWheelZoom: false }).setView([pos.lat, pos.lon], 13);
      L.marker([pos.lat, pos.lon], { interactive: false }).addTo(map);
      geo.mini = map;
      setTimeout(function () { if (geo.mini === map) map.invalidateSize(); }, 0);
    }).catch(function () { mapBox.appendChild(el('div', 'geo-offline', tr('geo.offline'))); });
  }
  return box;
}

// ------------------------------------------------------------------ set a location

// Give a photo without a position one: a pin on the map or latitude and
// longitude typed, both always there and kept in step. Saved in shoebox only.
function locationDialog(info) {
  var pos = info.position;
  var body = el('div', 'geo-dialog');
  var pick = el('div', 'geo-pick');
  var mapBox = el('div', 'geo-pickmap');
  pick.appendChild(mapBox);
  body.appendChild(pick);
  var hint = el('p', 'hint', tr(geo.on ? 'geo.dialog_map' : 'geo.dialog_maps_off'));
  body.appendChild(hint);
  var fields = el('div', 'fields');
  var field = function (key, limit) {
    var wrap = el('label', 'grow');
    wrap.appendChild(el('span', 'hint', tr('geo.' + key)));
    var input = el('input');
    input.type = 'text';
    input.inputMode = 'decimal';
    input.autocomplete = 'off';
    input.setAttribute('aria-label', tr('geo.' + key));
    wrap.appendChild(input);
    fields.appendChild(wrap);
    return { input: input, limit: limit };
  };
  var lat = field('lat', 90), lon = field('lon', 180);
  body.appendChild(fields);
  var error = el('p', 'error');
  body.appendChild(error);
  body.appendChild(el('p', 'hint', tr('geo.dialog_hint')));

  var map = null, marker = null;
  var values = function () { return [parseCoord(lat.input.value, 90), parseCoord(lon.input.value, 180)]; };
  var save;
  var check = function () {
    var v = values();
    var latBad = lat.input.value.trim() !== '' && v[0] === null;
    var lonBad = lon.input.value.trim() !== '' && v[1] === null;
    lat.input.classList.toggle('bad', latBad);
    lon.input.classList.toggle('bad', lonBad);
    error.textContent = latBad ? tr('geo.bad_lat') : lonBad ? tr('geo.bad_lon') : '';
    if (save) save.disabled = v[0] === null || v[1] === null;
    return v;
  };
  var place = function (la, lo, fromMap) {
    if (!map) return;
    if (!marker) marker = L.marker([la, lo], { draggable: true }).addTo(map).on('dragend', function () {
      var p = map.wrapLatLng(marker.getLatLng());
      setFields(p.lat, p.lng);
    });
    marker.setLatLng([la, lo]);
    if (!fromMap) map.panTo([la, lo]);
  };
  var setFields = function (la, lo) {
    la = Math.max(-90, Math.min(90, la));
    lat.input.value = la.toFixed(6);
    lon.input.value = lo.toFixed(6);
    check();
  };
  [lat, lon].forEach(function (f) {
    f.input.addEventListener('input', function () {
      var v = check();
      if (v[0] !== null && v[1] !== null) place(v[0], v[1], false);
    });
  });
  if (pos) { lat.input.value = String(pos.lat); lon.input.value = String(pos.lon); }

  var go = function (btn) {
    var v = check();
    if (v[0] === null || v[1] === null) return false;
    btn.disabled = true;
    post(fileBase(info.id) + '/position', { lat: v[0], lon: v[1] }).then(function () {
      closeModal();
      toast(tr('geo.saved'));
      geoPositionChanged(info);
    }).catch(function (e) { btn.disabled = false; error.textContent = e.message; });
    return false;
  };
  var actions = [{ label: tr('app.cancel'), cls: 'quiet' }];
  if (pos && pos.source === 'user') {
    actions.push({ label: tr('geo.remove'), cls: 'danger', onclick: function (btn) {
      btn.disabled = true;
      post(fileBase(info.id) + '/position', { clear: true }).then(function () {
        closeModal();
        toast(tr('geo.removed'));
        geoPositionChanged(info);
      }).catch(function (e) { btn.disabled = false; error.textContent = e.message; });
      return false;
    } });
  }
  actions.push({ label: tr('geo.save'), onclick: go });
  var buttons = openModal(tr('geo.dialog_title'), body, actions);
  save = buttons[buttons.length - 1];
  check();
  lat.input.focus();
  var enter = function (ev) { if (ev.key === 'Enter') go(save); };
  lat.input.addEventListener('keydown', enter);
  lon.input.addEventListener('keydown', enter);

  if (!geo.on) { pick.hidden = true; return; }
  leafletReady().then(function () {
    if ($('modal').hidden) return;
    map = newMap(mapBox, { worldCopyJump: true, minZoom: 1 });
    var v = values();
    if (v[0] !== null && v[1] !== null) { map.setView([v[0], v[1]], 12); place(v[0], v[1], true); }
    else map.setView([25, 10], 2);
    map.on('click', function (ev) {
      var p = map.wrapLatLng(ev.latlng);
      setFields(p.lat, p.lng);
      place(p.lat, p.lng, true);
    });
    setTimeout(function () { map.invalidateSize(); }, 0);
    var was = closeModal.onclose;
    closeModal.onclose = function () { map.remove(); if (was) was(); };
  }).catch(function () { pick.hidden = true; });
}

// A position was set or removed: show it in the open photo and the places.
function geoPositionChanged(info) {
  api(fileBase(info.id)).then(function (fresh) {
    if (lb.details && lb.details.id === fresh.id) { lb.details = fresh; renderPanel(); }
  }).catch(function () {});
  loadPlaces();
  if (state.filter.view === 'locations' && geo.map) refreshPoints();
}

// ------------------------------------------------------------------ places in the sidebar

function loadPlaces() {
  return api(LIBAPI + '/places').then(function (list) {
    geo.places = list;
    renderLocationsSection();
    if (geo.map) drawPlaces();
    var f = state.filter;
    if (f && f.place && !placeOf(f.place)) setFilter(withFilter({ place: null })); // deleted meanwhile
    else if (f && f.place) renderChips();
  }).catch(function () {});
}

function renderLocationsSection() {
  var sec = $('locations-section');
  if (!sec) return;
  sec.hidden = !geo.on || isAll();
  var list = $('place-list');
  list.textContent = '';
  var f = state.filter;
  geo.places.forEach(function (p) {
    var li = el('li');
    var row = el('div', 'row');
    row.appendChild(el('span', 'toggle', ''));
    var b = el('button', 'name', p.name);
    b.dataset.place = p.id;
    b.title = p.name;
    b.classList.toggle('active', f.view === 'locations' && f.id === p.id);
    b.onclick = function () { showView('locations', p.id); };
    row.appendChild(b);
    row.appendChild(el('span', 'count', I18n.number(p.count)));
    li.appendChild(row);
    list.appendChild(li);
  });
  var li = el('li');
  var row = el('div', 'row');
  row.appendChild(el('span', 'toggle', ''));
  var add = el('button', 'name', tr('side.new_place'));
  add.onclick = function () {
    geo.pendingDraw = true;
    if (state.filter.view === 'locations' && geo.map) { geo.pendingDraw = false; startDraw({ type: 'new' }); }
    else showView('locations');
  };
  row.appendChild(add);
  li.appendChild(row);
  list.appendChild(li);
  $('nav-locations').classList.toggle('active', f.view === 'locations' && !f.id);
}

// ------------------------------------------------------------------ the Locations page

function geoLeave() {
  cancelDraw();
  if (geo.map) { geo.map.remove(); geo.map = null; }
  geo.page = null;
  geo.sel = null;
  geo.data = null;
  geo.rects = {};
  geo.selRect = null;
  geo.cluster = null;
}

function loadLocations() {
  // Already open (another place picked, or a reload): only change the selection.
  if (geo.map && geo.page && geo.page.isConnected) { selectFromHash(); return; }
  geoLeave();
  var page = $('page');
  page.textContent = '';
  if (!geo.on) {
    page.appendChild(el('h2', '', tr('geo.page_title')));
    page.appendChild(el('p', 'sub', tr('geo.maps_needed')));
    var b = el('button', 'btn', tr('geo.open_settings'));
    b.onclick = function () { showView('settings'); };
    page.appendChild(b);
    return;
  }
  var root = el('div', 'geo-page');
  var main = el('div', 'geo-main');
  var mapBox = el('div', 'geo-map');
  main.appendChild(mapBox);
  var bar = el('div', 'geo-bar');
  main.appendChild(bar);
  var empty = el('div', 'geo-empty', tr('geo.none_yet'));
  empty.hidden = true;
  main.appendChild(empty);
  var side = el('aside', 'geo-side');
  var tab = el('button', 'geo-tab', '‹');
  tab.type = 'button';
  tab.title = tr('geo.side_show');
  tab.onclick = function () { setSide(true); };
  main.appendChild(tab);
  root.appendChild(main);
  root.appendChild(side);
  page.appendChild(root);
  geo.page = root;
  geo.ui = { root: root, main: main, bar: bar, side: side, tab: tab, empty: empty };
  setSide(geo.side, true);

  Promise.all([leafletReady(), api(LIBAPI + '/geo/points'), loadPlaces()]).then(function (r) {
    if (geo.page !== root) return; // left meanwhile
    var map = newMap(mapBox, { worldCopyJump: true, minZoom: 2 });
    geo.map = map;
    map.on('click', onMapClick);
    geo.cluster = L.markerClusterGroup({ chunkedLoading: true, showCoverageOnHover: false, maxClusterRadius: 55 });
    geo.cluster.on('clusterclick', function (e) {
      if (geo.drawing) { drawClick(e); return; }
      // The cluster's own photos (its bounds may hold others that sit apart).
      var only = {};
      e.layer.getAllChildMarkers().forEach(function (m) { only[m.options.pid] = true; });
      selectArea(boundsArea(e.layer.getBounds()), only);
    });
    geo.cluster.on('click', function (e) {
      if (geo.drawing) { drawClick(e); return; }
      var p = e.layer.getLatLng();
      selectArea(boundsArea(L.latLngBounds(p, p)));
    });
    map.addLayer(geo.cluster);
    fillPoints(r[1]);
    drawPlaces();
    fitAll(r[1]);
    barButtons();
    setTimeout(function () { map.invalidateSize(); selectFromHash(); if (!geo.sel) renderSide(); }, 0);
  }).catch(function (e) {
    mapBox.appendChild(el('div', 'geo-offline', tr('geo.offline')));
    console.error(e);
  });
}

function fillPoints(pts) {
  geo.cluster.clearLayers();
  var layers = [];
  for (var i = 0; i < pts.ids.length; i++) layers.push(L.marker([pts.lats[i], pts.lons[i]], { pid: pts.ids[i] }));
  geo.cluster.addLayers(layers);
  geo.ui.empty.hidden = pts.ids.length > 0;
}

function refreshPoints() {
  api(LIBAPI + '/geo/points').then(function (pts) { if (geo.map) fillPoints(pts); });
}

function fitAll(pts) {
  if (!pts.ids.length) { geo.map.setView([25, 10], 2); return; }
  var b = L.latLngBounds(pts.ids.map(function (_, i) { return [pts.lats[i], pts.lons[i]]; }));
  geo.map.fitBounds(b.pad(0.1), { maxZoom: 12 });
}

var PLACE_STYLE = { color: '#e0a46d', weight: 2, fillColor: '#e0a46d', fillOpacity: 0.08 };
var PLACE_ON = { color: '#c0392b', weight: 3, fillColor: '#c0392b', fillOpacity: 0.12 };

function drawPlaces() {
  if (!geo.map) return;
  Object.keys(geo.rects).forEach(function (id) { geo.map.removeLayer(geo.rects[id]); });
  geo.rects = {};
  geo.places.forEach(function (p) {
    var r = L.rectangle([[p.south, p.west], [p.north, p.east]], PLACE_STYLE).addTo(geo.map);
    r.bindTooltip(p.name, { sticky: true });
    r.on('click', function (e) { if (geo.drawing) drawClick(e); else showView('locations', p.id); });
    geo.rects[p.id] = r;
  });
  markSelection();
}

function markSelection() {
  Object.keys(geo.rects).forEach(function (id) {
    geo.rects[id].setStyle(geo.sel && geo.sel.kind === 'place' && geo.sel.id === Number(id) ? PLACE_ON : PLACE_STYLE);
  });
}

function boundsArea(b) {
  // A single spot gets a little room, so its own photos are inside.
  var pad = 0.0005;
  var s = Math.max(-90, b.getSouth() - pad), n = Math.min(90, b.getNorth() + pad);
  var w = Math.max(-180, b.getWest() - pad), e = Math.min(180, b.getEast() + pad);
  return { south: s, west: w, north: n, east: e };
}

function barButtons() {
  var bar = geo.ui.bar;
  bar.textContent = '';
  if (geo.drawing) {
    bar.appendChild(el('span', 'geo-hint', tr(geo.drawing.first ? 'geo.draw_second' : 'geo.draw_first')));
    var c = el('button', 'btn quiet small', tr('geo.draw_cancel'));
    c.onclick = cancelDraw;
    bar.appendChild(c);
    return;
  }
  bar.appendChild(el('span', 'geo-hint', tr('geo.page_hint')));
  var n = el('button', 'btn small', tr('geo.new_place'));
  n.onclick = function () { startDraw({ type: 'new' }); };
  bar.appendChild(n);
}

// ---- selection

function selectFromHash() {
  var id = state.filter.id;
  if (id && geo.places.some(function (p) { return p.id === id; })) { selectPlace(id, true); }
  else if (geo.sel && geo.sel.kind === 'place') { geo.sel = null; markSelection(); renderSide(); }
  if (geo.pendingDraw) { geo.pendingDraw = false; startDraw({ type: 'new' }); }
}

function placeOf(id) {
  return geo.places.filter(function (p) { return p.id === id; })[0] || null;
}

function selectPlace(id, fit) {
  var p = placeOf(id);
  if (!p) return;
  geo.sel = { kind: 'place', id: id };
  if (geo.selRect) { geo.map.removeLayer(geo.selRect); geo.selRect = null; }
  markSelection();
  if (fit && geo.map && geo.rects[id]) {
    geo.map.fitBounds(geo.rects[id].getBounds().pad(0.15), { maxZoom: 17 });
  }
  loadSelection();
}

function selectArea(area, only) {
  if (geo.drawing) return;
  geo.sel = { kind: 'area', area: area, only: only || null };
  if (geo.selRect) geo.map.removeLayer(geo.selRect);
  geo.selRect = L.rectangle([[area.south, area.west], [area.north, area.east]], { color: '#c0392b', weight: 1, dashArray: '4 4', fill: false, interactive: false }).addTo(geo.map);
  markSelection();
  loadSelection();
}

function onMapClick(ev) {
  if (geo.drawing) drawClick(ev);
}

var SIDE_PAGE = 150;

function loadSelection() {
  var sel = geo.sel;
  if (!sel) { renderSide(); return; }
  var q = sel.kind === 'place' ? { place: sel.id }
    : { area: [sel.area.south, sel.area.west, sel.area.north, sel.area.east].join(',') };
  if (!geo.side) setSide(true);
  var seq = (geo.seq = (geo.seq || 0) + 1);
  api(LIBAPI + '/timeline' + query(q)).then(function (data) {
    if (seq !== geo.seq) return;
    geo.data = sel.only ? onlyIds(data, sel.only) : data;
    geo.shown = 0;
    renderSide();
  }).catch(failed);
}

// The timeline columns of the photos in `set` (ids) only.
function onlyIds(data, set) {
  var out = { count: 0, ids: [], kinds: '', days: [], versions: '', live: [], tags: [], people: [], favs: [] };
  data.ids.forEach(function (id, i) {
    if (!set[id]) return;
    out.ids.push(id);
    out.kinds += data.kinds[i];
    out.days.push(data.days[i]);
    out.versions += data.versions.substr(i * 8, 8);
  });
  out.count = out.ids.length;
  out.live = data.live.filter(function (p) { return set[p[0]]; });
  out.favs = (data.favs || []).filter(function (id) { return set[id]; });
  return out;
}

function setSide(open, quiet) {
  geo.side = open;
  try { localStorage.setItem('geoSide', open ? '1' : '0'); } catch (e) { /* ignore */ }
  if (!geo.ui) return;
  geo.ui.root.classList.toggle('side-hidden', !open);
  geo.ui.tab.hidden = open;
  if (!quiet && geo.map) setTimeout(function () { geo.map.invalidateSize(); }, 0);
}

function renderSide() {
  var side = geo.ui && geo.ui.side;
  if (!side) return;
  side.textContent = '';
  var head = el('div', 'geo-side-head');
  var sel = geo.sel;
  var title = el('h3', '');
  var place = sel && sel.kind === 'place' ? placeOf(sel.id) : null;
  if (!sel) title.textContent = tr('geo.pick');
  else if (place) title.textContent = place.name;
  else title.textContent = geo.data ? tr('geo.here_count', { count: trn('count.photos', geo.data.count) }) : '';
  head.appendChild(title);
  var tools = el('div', 'geo-tools');
  var tool = function (text, label, fn) {
    var b = el('button', 'icon-btn', text);
    b.type = 'button';
    b.title = label;
    b.setAttribute('aria-label', label);
    b.onclick = fn;
    tools.appendChild(b);
  };
  if (place) {
    tool('✎', tr('geo.rename'), function () { renamePlace(place); });
    tool('⬚', tr('geo.redraw'), function () { startDraw({ type: 'redraw', place: place }); });
    tool('🗑', tr('app.delete'), function () { deletePlace(place); });
  }
  tool('›', tr('geo.side_hide'), function () { setSide(false); });
  head.appendChild(tools);
  side.appendChild(head);
  if (place && geo.data) side.appendChild(el('p', 'geo-sub', tr('geo.place_count', { count: trn('count.photos', geo.data.count) })));
  if (!sel || !geo.data) return;
  var grid = el('div', 'geo-grid');
  side.appendChild(grid);
  var more = el('button', 'btn quiet', tr('app.show_more'));
  more.onclick = function () { addCells(grid, more); };
  side.appendChild(more);
  addCells(grid, more);
}

function addCells(grid, more) {
  var d = geo.data;
  var end = Math.min(d.count, geo.shown + SIDE_PAGE);
  for (var i = geo.shown; i < end; i++) {
    (function (i) {
      var b = el('button', 'geo-cell');
      b.type = 'button';
      var img = el('img');
      img.alt = '';
      img.loading = 'lazy';
      img.decoding = 'async';
      img.src = LIBAPI + '/files/' + d.ids[i] + '/thumb?v=' + d.versions.substr(i * 8, 8);
      b.appendChild(img);
      if (d.kinds[i] === 'v') b.appendChild(el('span', 'badge', KIND_BADGE.v));
      b.onclick = function () { openFromSide(d, i); };
      grid.appendChild(b);
    })(i);
  }
  geo.shown = end;
  more.hidden = end >= d.count;
}

// The viewer over the selected photos; closing it comes back to the map.
function openFromSide(data, i) {
  var saved = { data: state.data, favs: state.favs, live: state.live };
  state.data = data;
  state.favs = {};
  (data.favs || []).forEach(function (id) { state.favs[id] = true; });
  state.live = {};
  data.live.forEach(function (p) { state.live[p[0]] = p[1]; });
  var from = document.activeElement;
  lb.restore = function () {
    state.data = saved.data;
    state.favs = saved.favs;
    state.live = saved.live;
    if (from && from.isConnected && from.focus) from.focus();
  };
  openLightbox(i);
}

// ---- places: create, rename, redraw, delete

function startDraw(purpose) {
  if (!geo.map) return;
  cancelDraw();
  geo.drawing = { purpose: purpose, first: null, rect: null };
  geo.cluster.options.zoomToBoundsOnClick = false; // a click on a pin is a corner while drawing
  geo.map.getContainer().classList.add('drawing');
  geo.map.on('mousemove', drawMove);
  barButtons();
}

function drawMove(ev) {
  var d = geo.drawing;
  if (d && d.first && d.rect) d.rect.setBounds(L.latLngBounds(d.first, ev.latlng));
}

function drawClick(ev) {
  var d = geo.drawing;
  if (!d) return;
  if (!d.first) {
    d.first = ev.latlng;
    d.rect = L.rectangle([d.first, d.first], { color: '#c0392b', weight: 2, dashArray: '5 4', interactive: false }).addTo(geo.map);
    barButtons();
    return;
  }
  var a = geo.map.wrapLatLng(d.first), b = geo.map.wrapLatLng(ev.latlng);
  var area = {
    south: Math.max(-90, Math.min(a.lat, b.lat)), north: Math.min(90, Math.max(a.lat, b.lat)),
    west: Math.max(-180, Math.min(a.lng, b.lng)), east: Math.min(180, Math.max(a.lng, b.lng)),
  };
  // A click twice on the same spot: a little room, so the area has a size.
  if (area.north - area.south < 0.0002) { area.south = Math.max(-90, area.south - 0.0005); area.north = Math.min(90, area.north + 0.0005); }
  if (area.east - area.west < 0.0002) { area.west = Math.max(-180, area.west - 0.0005); area.east = Math.min(180, area.east + 0.0005); }
  var purpose = d.purpose, rect = d.rect;
  d.first = null; // the rectangle stays until the answer
  d.rect = null;
  geo.drawing = null;
  geo.map.off('mousemove', drawMove);
  geo.cluster.options.zoomToBoundsOnClick = true;
  geo.map.getContainer().classList.remove('drawing');
  barButtons();
  var drop = function () { if (geo.map && rect) geo.map.removeLayer(rect); };
  if (purpose.type === 'new') {
    nameDialog(tr('geo.name_place'), '', tr('geo.create'), function (name) {
      return post(LIBAPI + '/places', Object.assign({ name: name }, area)).then(function (p) {
        toast(tr('geo.created', { name: p.name }));
        return loadPlaces().then(function () { showView('locations', p.id); });
      });
    });
    var was = closeModal.onclose;
    closeModal.onclose = function () { drop(); if (was) was(); };
  } else {
    drop();
    post(LIBAPI + '/places/' + purpose.place.id + '/area', area).then(function (p) {
      toast(tr('geo.redrawn', { name: p.name }));
      return loadPlaces().then(function () { selectPlace(p.id, false); });
    }).catch(failed);
  }
}

function cancelDraw() {
  var d = geo.drawing;
  geo.drawing = null;
  if (!geo.map) return;
  if (d && d.rect) geo.map.removeLayer(d.rect);
  geo.map.off('mousemove', drawMove);
  if (geo.cluster) geo.cluster.options.zoomToBoundsOnClick = true;
  geo.map.getContainer().classList.remove('drawing');
  if (geo.ui) barButtons();
}

document.addEventListener('keydown', function (ev) {
  if (ev.key === 'Escape' && geo.drawing && $('modal').hidden && $('lightbox').hidden) cancelDraw();
});

function renamePlace(p) {
  nameDialog(tr('geo.rename'), p.name, tr('geo.rename'), function (name) {
    return post(LIBAPI + '/places/' + p.id + '/rename', { name: name }).then(function (r) {
      toast(tr('geo.renamed', { name: r.name }));
      return loadPlaces().then(renderSide);
    });
  });
}

function deletePlace(p) {
  openModal(tr('geo.delete_title'), tr('geo.delete_text', { name: p.name }), [
    { label: tr('app.cancel'), cls: 'quiet' },
    { label: tr('app.delete'), cls: 'danger', onclick: function () {
      post(LIBAPI + '/places/' + p.id + '/delete').then(function () {
        toast(tr('geo.deleted', { name: p.name }));
        geo.sel = null;
        geo.data = null;
        return loadPlaces().then(function () { showView('locations'); renderSide(); });
      }).catch(failed);
    } },
  ]);
}

$('nav-locations').onclick = function () { sectionTitleClick('locations-section', function () { showView('locations'); }); };

// ------------------------------------------------------------------ places in the search

// Places whose name contains what is typed, with their photo counts. Only with
// maps on, and never the one the search already has.
function placeSuggestions(needle, f) {
  if (!geo.on || isAll()) return [];
  var low = fold(needle);
  return geo.places.filter(function (p) {
    return p.count && p.id !== f.place && (!low || fold(p.name).indexOf(low) >= 0);
  }).slice(0, 5).map(function (p) { return { kind: 'place', id: p.id, label: p.name, count: p.count }; });
}

// A place picked in the search. With nothing else in the search (the text just
// typed is replaced by the pick, like for a tag) or on the Locations page it
// shows the place on the map, as a click in the sidebar does. Otherwise it
// joins the search as a chip and the timeline stays; a second place replaces
// the first, since two areas rarely overlap.
function pickPlace(id) {
  var f = state.filter;
  var chips = f.folder || f.tags.length || f.people.length || f.pets.length || f.fav || f.place;
  if (f.view === 'locations' || !chips) showView('locations', id);
  else setFilter(withFilter({ place: id, q: '' }));
}
