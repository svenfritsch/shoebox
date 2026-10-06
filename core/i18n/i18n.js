// shoebox translations, shared by the photo app and the launcher.
// Messages live in en.json and de.json (flat "area.name" keys, `{name}`
// placeholders); English is the fallback for anything missing in another
// language. Plain ES2017 without a build step, like the rest of the UI.
//
//   tr('key', { name: 'x' })      a message
//   trn('key', 3, { name: 'x' })  a message with plural forms: key.one /
//                                 key.other (more where a language needs them);
//                                 {n} is the count, formatted for the language
//   I18n.date(date, opts)         dates and numbers in the chosen language
//   I18n.ready                    a promise: the messages have loaded
'use strict';

var I18n = (function () {
  var LANGS = { en: 'English', de: 'Deutsch' };
  var KEY = 'shoebox.lang';
  var messages = {};
  var fallback = {};

  function stored() {
    try { return localStorage.getItem(KEY); } catch (e) { return null; }
  }

  // The saved choice, else the first browser language we have, else English.
  function detect() {
    var s = stored();
    if (s && LANGS[s]) return s;
    var prefs = (navigator.languages && navigator.languages.length) ? navigator.languages : [navigator.language || 'en'];
    for (var i = 0; i < prefs.length; i++) {
      var base = String(prefs[i]).toLowerCase().split('-')[0];
      if (LANGS[base]) return base;
    }
    return 'en';
  }

  var lang = detect();

  function fill(text, vars) {
    if (!vars) return text;
    return text.replace(/\{(\w+)\}/g, function (m, name) {
      if (!Object.prototype.hasOwnProperty.call(vars, name)) return m;
      var v = vars[name];
      return typeof v === 'number' ? v.toLocaleString(lang) : String(v);
    });
  }

  function lookup(key) {
    if (Object.prototype.hasOwnProperty.call(messages, key)) return messages[key];
    if (Object.prototype.hasOwnProperty.call(fallback, key)) return fallback[key];
    return null;
  }

  function tr(key, vars) {
    var text = lookup(key);
    return text == null ? key : fill(text, vars);
  }

  function trn(key, n, vars) {
    var form = new Intl.PluralRules(lang).select(n);
    var text = lookup(key + '.' + form);
    if (text == null) text = lookup(key + '.other');
    if (text == null) return key;
    var all = { n: n };
    if (vars) for (var k in vars) all[k] = vars[k];
    return fill(text, all);
  }

  function load(code) {
    return fetch('i18n/' + code + '.json', { credentials: 'same-origin' }).then(function (r) {
      if (!r.ok) throw new Error('i18n/' + code + '.json: HTTP ' + r.status);
      return r.json();
    });
  }

  // Static text in the HTML: data-i18n="key" sets the text, and
  // data-i18n-title / -placeholder / -aria-label set those attributes.
  function apply(root) {
    var ATTRS = ['title', 'placeholder', 'aria-label'];
    Array.prototype.forEach.call((root || document).querySelectorAll('[data-i18n]'), function (e) {
      e.textContent = tr(e.getAttribute('data-i18n'));
    });
    ATTRS.forEach(function (a) {
      Array.prototype.forEach.call((root || document).querySelectorAll('[data-i18n-' + a + ']'), function (e) {
        e.setAttribute(a, tr(e.getAttribute('data-i18n-' + a)));
      });
    });
  }

  var ready = Promise.all([
    load('en').catch(function () { return {}; }),
    lang === 'en' ? null : load(lang).catch(function () { return {}; }),
  ]).then(function (r) {
    fallback = r[0];
    messages = r[1] || r[0];
    document.documentElement.lang = lang;
    apply();
  });

  return {
    langs: LANGS,
    get lang() { return lang; },
    ready: ready,
    apply: apply,
    // Remember the choice and reload: the UI is built once from the messages.
    setLang: function (code) {
      if (!LANGS[code]) return;
      try { localStorage.setItem(KEY, code); } catch (e) { /* the page still follows the browser */ }
      location.reload();
    },
    date: function (d, opts) { return d.toLocaleDateString(lang, opts); },
    dateTime: function (d, opts) { return d.toLocaleString(lang, opts); },
    number: function (n) { return n.toLocaleString(lang); },
    t: tr,
    tn: trn,
  };
})();

var tr = I18n.t;
var trn = I18n.tn;
