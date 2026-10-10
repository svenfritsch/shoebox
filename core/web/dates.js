// Estimated capture dates (phase 12, docs/phase12.md): a date of the user's own
// for a photo, a year, a month or a day. It wins on the timeline over the date
// in the file, which stays in the info panel's "EXIF" row. Saved in the
// library's database only; the file is never changed.

var dates = { needs: 0 };

var PENCIL = '<svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><path fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" d="M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/></svg>';

function monthName(m) { return I18n.date(new Date(2000, m - 1, 1), { month: 'long' }); }

// "1987", "June 1987" or "June 14, 1987", whichever the date knows.
function formatEstimate(e) {
  if (!e.month) return String(e.year);
  var d = new Date(e.year, e.month - 1, e.day || 1);
  return I18n.date(d, e.day ? { day: 'numeric', month: 'long', year: 'numeric' } : { month: 'long', year: 'numeric' });
}

// A date chip of the URL: "1987", "1987-06" or "1987-06-14".
function dateTermLabel(term) {
  var p = term.split('-').map(Number);
  return formatEstimate({ year: p[0], month: p[1] || null, day: p[2] || null });
}

// ------------------------------------------------------------------ timeline: the "~"

var EST_TITLES = { m: 'dates.est_manual', f: 'dates.est_folder', c: 'dates.est_created', u: 'dates.est_modified' };

// The small "~" on a cell whose date is not the capture date in the file.
function estimateMark(code) {
  var s = el('span', 'est', '~');
  s.title = tr(EST_TITLES[code]);
  return s;
}

// ------------------------------------------------------------------ info panel

// The Date row: the date, "estimated" (a fallback) or "manual" (the user's),
// and a pencil at the right end; under it, when the user's date sits on top of
// a capture date, the file's own as "EXIF".
function infoDate(row, info) {
  var cell = el('div', 'datecell');
  var value = el('span', 'v', formatDate(info) + (!info.estimate && info.taken_offset ? ' (UTC' + info.taken_offset + ')' : ''));
  if (info.estimate) {
    value.appendChild(el('span', 'estimated mine', tr('info.manual')));
  } else if (info.date_source !== 'file') {
    // Not the day the photo was taken: say so next to the date; which
    // fallback was used is in the tooltip.
    var badge = el('span', 'estimated', tr('info.estimated'));
    badge.title = tr('info.estimated_hint') + '\n' + tr(info.date_source === 'created' ? 'info.no_date_created'
      : info.date_source === 'folder' ? 'info.no_date_folder' : 'info.no_date_file');
    value.appendChild(badge);
  }
  cell.appendChild(value);
  var pen = el('button', 'pen');
  pen.type = 'button';
  pen.innerHTML = PENCIL;
  var label = tr(info.estimate ? 'info.change_date' : 'info.set_date');
  pen.title = label;
  pen.setAttribute('aria-label', label);
  pen.onclick = function () { datesDialog([info.id], info); };
  cell.appendChild(pen);
  row(tr('info.date'), cell);
  if (info.estimate && info.exif) {
    row(tr('info.exif'), formatDate({ taken: info.exif.taken, date_source: 'file' }) + (info.exif.taken_offset ? ' (UTC' + info.exif.taken_offset + ')' : ''));
  }
}

// ------------------------------------------------------------------ the dialog

// A year as typed: four digits, or two ("87" is 1987, "05" is 2005, up to this year).
function parseYear(s) {
  s = s.trim();
  if (!/^(\d{2}|\d{4})$/.test(s)) return null;
  var n = parseInt(s, 10);
  if (s.length === 2) n = 1900 + n > new Date().getFullYear() ? 2000 + n : 1900 + n;
  return n;
}

function daysIn(year, month) { return new Date(year, month, 0).getDate(); }

// Set, change or remove the date of these photos. `info` is the open photo's
// details (one photo), else the dialog asks what it needs to say about all.
function datesDialog(ids, info) {
  var one = ids.length === 1 && info && info.id === ids[0];
  var ready = one
    ? Promise.resolve({ total: 1, with_exif: info.exif ? 1 : 0, with_estimate: info.estimate ? 1 : 0 })
    : post(LIBAPI + '/files/dates/check', { ids: ids });
  ready.then(function (check) { buildDatesDialog(ids, one ? info : null, check); }).catch(failed);
}

function buildDatesDialog(ids, info, check) {
  var current = info ? info.estimate : null;
  var body = el('div', 'dates-dialog');
  if (!info && check.with_exif) {
    body.appendChild(el('p', 'hint', tr(check.with_exif === check.total ? 'dates.batch_all' : 'dates.batch_some', { n: check.with_exif, total: check.total })));
  }
  var fields = el('div', 'dates-fields');
  var field = function (key, node) {
    var wrap = el('label');
    wrap.appendChild(el('span', 'hint', tr('dates.' + key)));
    wrap.appendChild(node);
    fields.appendChild(wrap);
  };
  var year = el('input');
  year.type = 'text';
  year.inputMode = 'numeric';
  year.maxLength = 4;
  year.autocomplete = 'off';
  year.placeholder = '1987';
  var month = el('select');
  var day = el('select');
  field('year', year);
  field('month', month);
  field('day', day);
  body.appendChild(fields);
  var error = el('p', 'error');
  body.appendChild(error);

  // What the app already knows: the month of the event folder.
  if (info && info.date_source === 'folder' && info.sort_date) {
    var quick = el('button', 'btn quiet small', tr('dates.folder', { date: formatEstimate({ year: +info.sort_date.slice(0, 4), month: +info.sort_date.slice(5, 7) }) }));
    quick.type = 'button';
    quick.onclick = function () { year.value = info.sort_date.slice(0, 4); month.value = String(+info.sort_date.slice(5, 7)); day.value = ''; render(); };
    body.appendChild(quick);
  }

  var shown = el('div', 'dates-shown');
  var lands = el('div', 'hint');
  var preview = el('div', 'dates-preview');
  preview.appendChild(shown);
  preview.appendChild(lands);
  body.appendChild(preview);
  body.appendChild(el('p', 'hint', tr('dates.hint')));

  month.appendChild(new Option(tr('dates.not_sure'), ''));
  for (var m = 1; m <= 12; m++) month.appendChild(new Option(monthName(m), String(m)));
  if (current) {
    year.value = String(current.year);
    month.value = current.month ? String(current.month) : '';
  }

  var save;
  var value = function () {
    var y = parseYear(year.value), mo = parseInt(month.value, 10) || null, d = parseInt(day.value, 10) || null;
    var bad = '';
    if (year.value.trim() && y === null) bad = tr('dates.err_year');
    else if (y && new Date(y, (mo || 1) - 1, d || 1) > new Date()) bad = tr('dates.err_future');
    return { year: y, month: mo, day: mo ? d : null, error: bad };
  };
  var render = function () {
    var v = value();
    // The days of that month: 1 up to 28, 29, 30 or 31; only once a month is chosen.
    var keep = day.value;
    day.textContent = '';
    day.appendChild(new Option(tr('dates.not_sure'), ''));
    if (v.month) for (var d = 1; d <= daysIn(v.year || 2000, v.month); d++) day.appendChild(new Option(String(d), String(d)));
    day.disabled = !v.month;
    if (keep && v.month && +keep <= daysIn(v.year || 2000, v.month)) day.value = keep;
    v = value();
    year.classList.toggle('bad', !!v.error);
    error.textContent = v.error;
    if (save) save.disabled = !v.year || !!v.error;
    if (!v.year) { shown.textContent = tr('dates.need_year'); lands.textContent = ''; return; }
    shown.textContent = formatEstimate(v);
    var group = formatEstimate({ year: v.year, month: v.month });
    lands.textContent = v.day ? tr('dates.lands_day', { group: group })
      : v.month ? tr('dates.lands_month', { group: group })
        : tr('dates.lands_year', { year: v.year });
  };
  [year, month, day].forEach(function (n) { n.addEventListener('input', render); n.addEventListener('change', render); });

  var send = function (btn, payload, removed) {
    btn.disabled = true;
    post(LIBAPI + '/files/dates', Object.assign({ ids: ids }, payload)).then(function (res) {
      datesSaved(res, ids, removed);
    }).catch(function (e) { btn.disabled = false; error.textContent = e.message; });
    return false;
  };
  var actions = [];
  if (check.with_estimate) {
    var back = info && info.exif ? tr('dates.back_to', { date: formatDate({ taken: info.exif.taken, date_source: 'file' }) }) : '';
    actions.push({ label: tr('dates.remove'), cls: 'quiet dates-remove', onclick: function (btn) { return send(btn, { clear: true }, true); }, title: back });
  }
  actions.push({ label: tr('app.cancel'), cls: 'quiet' });
  actions.push({ label: tr('dates.save'), onclick: function (btn) {
    var v = value();
    if (!v.year || v.error) return false;
    return send(btn, { year: v.year, month: v.month, day: v.day }, false);
  } });
  var title = !info ? tr('dates.title_batch', { count: trn('count.photos', ids.length) }) : tr(current ? 'dates.title_change' : 'dates.title_set');
  var buttons = openModal(title, body, actions);
  buttons.forEach(function (b, i) { if (actions[i].title) b.title = actions[i].title; });
  save = buttons[buttons.length - 1];
  render();
  if (current && current.day) { day.value = String(current.day); render(); }
  year.focus();
  year.addEventListener('keydown', function (ev) { if (ev.key === 'Enter' && !save.disabled) save.click(); });
}

// Saved (or removed): update what is on screen and offer to undo it.
function datesSaved(res, ids, removed) {
  closeModal();
  if (state.selecting) endSelection();
  datesRefresh(ids);
  toastUndo(tr(removed ? 'dates.removed' : 'dates.saved', { count: trn('count.photos', ids.length) }), function () {
    post(LIBAPI + '/files/dates', { restore: res.previous }).then(function () {
      toast(tr('dates.undone'));
      datesRefresh(ids);
    }).catch(failed);
  });
}

function datesRefresh(ids) {
  reloadAll();
  loadInfo();
  var open = lb.details;
  if (open && ids.indexOf(open.id) >= 0 && !isAll() && !$('lightbox').hidden) {
    api(fileBase(open.id)).then(function (fresh) {
      if (!lb.details || lb.details.id !== fresh.id) return;
      lb.details = fresh;
      $('lb-title').textContent = formatDate(fresh) + ' · ' + fresh.name;
      renderPanel();
    }).catch(function () {});
  }
}

// A message with a button that takes the change back.
function toastUndo(text, onUndo) {
  var t = $('toast');
  t.textContent = text + ' ';
  var b = el('button', 'toast-undo', tr('dates.undo'));
  b.type = 'button';
  b.onclick = function () { t.hidden = true; clearTimeout(toastTimer); onUndo(); };
  t.appendChild(b);
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(function () { t.hidden = true; }, 10000);
}

$('sel-date').onclick = function () { datesDialog(selectedIds(), null); };

// ------------------------------------------------------------------ search: the Date group

// The `date:` shortcut ("datum:" works too): the box then looks for a date only.
var DATE_PREFIX = /^\s*(?:date|datum):\s*(.*)$/i;

// The rows of the "Date" group as suggestions: a year, a month or a day.
function dateItems(rows) {
  return rows.map(function (r) {
    var id = String(r.year) + (r.month ? '-' + ('0' + r.month).slice(-2) : '') + (r.day ? '-' + ('0' + r.day).slice(-2) : '');
    return { kind: 'date', id: id, label: formatEstimate(r), count: r.count };
  });
}

// The rest of the filter, which the counts of the rows are made within.
function datesWithin(f) {
  return { tag: f.tags, person: f.people, pet: f.pets, type: f.types, fav: f.fav ? 1 : null, folder: f.folder, place: f.place, date: f.dates, nodate: f.nodate ? 1 : null };
}

function suggestDates(needle, prefix, within) {
  return api(LIBAPI + '/dates' + query(Object.assign({ q: needle, prefix: prefix ? 1 : null }, within))).catch(function () { return []; });
}
