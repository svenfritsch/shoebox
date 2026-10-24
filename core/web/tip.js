// The quick label: one tooltip for every control with a `data-tip` (set it
// with `setTip`, or `data-i18n-tip` in the HTML). It shows after a short
// delay, at once while moving from one control to the next or on keyboard
// focus, and never on touch. Text that only reveals a clipped value (a path,
// a file name) keeps the browser's own `title`.
var TIP_DELAY = 150;

function setTip(node, text) {
  if (!text) {
    node.removeAttribute('data-tip');
    return node;
  }
  node.setAttribute('data-tip', text);
  // `title` used to name an icon-only control or describe the others for a screen reader.
  var iconOnly = !/[\p{L}\p{N}]/u.test(node.textContent);
  if (iconOnly && (node.tipNames || !node.hasAttribute('aria-label') && !node.hasAttribute('aria-labelledby'))) {
    node.tipNames = true;
    node.setAttribute('aria-label', text);
  } else {
    node.setAttribute('aria-description', text);
  }
  return node;
}

// The control a quick label belongs to: one with a `data-tip`, or a text button whose label is
// cut with an ellipsis (it then names itself).
function tipTarget(node) {
  if (!node.closest) return null;
  var t = node.closest('[data-tip]');
  if (t) return t;
  var b = node.closest('.btn, .sel-all');
  return b && b.scrollWidth > b.clientWidth ? b : null;
}

function tipText(target) {
  return target.getAttribute('data-tip') || target.textContent.trim();
}

(function () {
  var box = null, timer = 0, current = null, shown = false;

  function ensure() {
    if (box) return box;
    box = document.createElement('div');
    box.id = 'quick-tip';
    box.setAttribute('role', 'tooltip');
    document.body.appendChild(box);
    return box;
  }

  function hide() {
    clearTimeout(timer);
    timer = 0;
    current = null;
    if (box) box.classList.remove('show');
    shown = false;
  }

  function place(target) {
    var b = ensure(), r = target.getBoundingClientRect(), m = 6;
    b.style.left = '0px';
    b.style.top = '0px';
    var w = b.offsetWidth, h = b.offsetHeight, vw = document.documentElement.clientWidth;
    var top = r.top - h - 8;
    var below = top < m;
    if (below) top = r.bottom + 8;
    var left = Math.max(m, Math.min(vw - w - m, r.left + r.width / 2 - w / 2));
    b.style.left = Math.round(left) + 'px';
    b.style.top = Math.round(top) + 'px';
    b.classList.toggle('below', below);
  }

  function show(target) {
    if (!target.isConnected || target.getAttribute('aria-disabled') === 'true' && !target.getAttribute('data-tip')) return;
    var text = tipText(target);
    if (!text) return;
    var b = ensure();
    b.textContent = text;
    // A long sentence wraps; a short name stays on one line.
    b.classList.toggle('long', text.length > 48 || text.indexOf('\n') >= 0);
    place(target);
    // Moving straight from one control to another is not animated again.
    b.classList.toggle('instant', shown);
    b.classList.add('show');
    shown = true;
    current = target;
  }

  function over(ev) {
    if (ev.pointerType === 'touch') return;
    var t = tipTarget(ev.target);
    if (t === current) return;
    if (!t) { if (current || timer) hide(); return; }
    clearTimeout(timer);
    if (shown) show(t);
    else {
      current = null;
      timer = setTimeout(function () { show(t); }, TIP_DELAY);
    }
  }

  document.addEventListener('pointerover', over);
  document.addEventListener('pointerleave', function (ev) { if (ev.target === document.documentElement) hide(); }, true);
  document.addEventListener('pointerdown', hide, true);
  document.addEventListener('scroll', hide, true);
  document.addEventListener('keydown', function (ev) { if (ev.key === 'Escape') hide(); }, true);
  document.addEventListener('focusin', function (ev) {
    var t = tipTarget(ev.target);
    if (t && t.matches(':focus-visible')) { clearTimeout(timer); show(t); }
  });
  document.addEventListener('focusout', hide);
  window.addEventListener('blur', hide);
  window.addEventListener('resize', hide);
})();
