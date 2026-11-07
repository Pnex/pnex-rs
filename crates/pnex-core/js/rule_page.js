// Page side of a navigation rule (site-recipes.md, agent-unification.md
// D780): the only code that runs inside the visited page, shared by both
// players. The server browser embeds this file and evaluates one function
// over CDP; the browser extension imports it and hands one function to
// `scripting.executeScript`, which serializes it alone. So every function is
// self-contained: no helper outside its own body, no global state besides
// the `data-pnex-target` mark it leaves for `pnexClick`.

/** Finds the target of a step, marks the first visible match: `{found, href}`. */
export function pnexFind(spec) {
  const norm = s => (s || '').normalize('NFD').replace(/[̀-ͯ]/g, '').replace(/\s+/g, ' ').trim().toLowerCase();
  // What a person can see: a box, not hidden, and no transparent ancestor
  // (closed menus are often opacity 0).
  const visible = e => { const r = e.getBoundingClientRect(); const s = getComputedStyle(e);
    if (!(r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none')) return false;
    for (let a = e; a && a.nodeType === 1; a = a.parentElement) { if (parseFloat(getComputedStyle(a).opacity) === 0) return false; }
    return true; };
  document.querySelectorAll('[data-pnex-target]').forEach(e => { e.style.removeProperty('outline'); e.style.removeProperty('box-shadow'); e.removeAttribute('data-pnex-target'); });
  let c = [];
  try {
    if (spec.selector) c = [...document.querySelectorAll(spec.selector)];
    else if (spec.role) {
      c = [...document.querySelectorAll('[role="' + spec.role.replace(/"/g, '') + '"]' + (spec.role === 'button' ? ',button' : spec.role === 'link' ? ',a[href]' : spec.role === 'tab' ? ',[role=tab]' : ''))];
      if (spec.name) c = c.filter(e => norm(e.getAttribute('aria-label') || e.innerText).includes(norm(spec.name)));
    } else if (spec.href_contains || spec.link_text_contains) {
      c = [...document.querySelectorAll('a[href]')].filter(a => spec.href_contains
        ? a.href.toLowerCase().includes(spec.href_contains.toLowerCase())
        : norm(a.innerText).includes(norm(spec.link_text_contains)));
    } else {
      const want = norm(spec.text || spec.text_contains);
      c = [...document.querySelectorAll('body *')].filter(e => {
        const t = norm(e.innerText); return spec.text ? t === want : t.includes(want); });
      c = c.filter(e => !c.some(o => o !== e && e.contains(o)));
    }
  } catch (err) { return { found: 0, error: String(err) }; }
  c = c.filter(visible);
  if (!c.length) return { found: 0 };
  const el = c[0];
  el.scrollIntoView({ block: 'center' });
  el.setAttribute('data-pnex-target', '1');
  el.style.setProperty('outline', '4px solid #e11d48', 'important');
  el.style.setProperty('box-shadow', '0 0 0 8px rgba(225, 29, 72, 0.25)', 'important');
  return { found: c.length, href: el.closest('a[href]') ? el.closest('a[href]').href : null };
}

/** Clicks the element `pnexFind` marked, right after returning. */
export function pnexClick() {
  const el = document.querySelector('[data-pnex-target]');
  if (!el) return false;
  el.style.removeProperty('outline');
  el.style.removeProperty('box-shadow');
  el.removeAttribute('data-pnex-target');
  const target = el.closest('a,button,[role=button],[role=tab],[role=link],summary,label') || el;
  // Clicked after this call returns: a navigation must not swallow its reply.
  setTimeout(() => target.click(), 0);
  return true;
}

/**
 * Types `spec.text` into a search field (D789): `{ok}`, or `{ok: false,
 * reason}` with `target-not-found` or `not-a-search-field`. A password, a
 * field of a form holding a password, or any field that is not a search is
 * refused: a rule never fills a form nor signs in.
 */
export function pnexType(spec) {
  const visible = e => { const r = e.getBoundingClientRect(); const s = getComputedStyle(e);
    return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; };
  const field = e => e && (e.tagName === 'INPUT' || e.tagName === 'TEXTAREA') ? e : e && e.querySelector ? e.querySelector('input,textarea') : null;
  const searchy = /search|recherch|suche|buscar|cerca|zoek|pesquis/i;
  const isSearch = e => {
    if (!e || e.disabled || e.readOnly) return false;
    const type = (e.getAttribute('type') || 'text').toLowerCase();
    if (!['text', 'search', ''].includes(type) && e.tagName !== 'TEXTAREA') return false;
    if (/password|username|email|one-time-code/i.test(e.getAttribute('autocomplete') || '')) return false;
    const form = e.closest('form');
    if (form && form.querySelector('input[type=password]')) return false;
    if (type === 'search') return true;
    const role = (e.getAttribute('role') || '').toLowerCase();
    if (role === 'searchbox' || role === 'combobox') return true;
    if (e.closest('[role=search],[role=combobox]')) return true;
    const hint = [e.name, e.id, e.getAttribute('aria-label'), e.getAttribute('placeholder')].join(' ');
    return searchy.test(hint);
  };
  let el = null;
  try {
    if (spec.selector) el = field(document.querySelector(spec.selector));
    else if (spec.role) el = field([...document.querySelectorAll('[role="' + spec.role.replace(/"/g, '') + '"]')].find(visible));
    else el = [...document.querySelectorAll('input,textarea')].find(e => visible(e) && isSearch(e)) || null;
  } catch (err) { return { ok: false, reason: 'target-not-found', detail: String(err) }; }
  if (!el) return { ok: false, reason: 'target-not-found' };
  if (!isSearch(el)) return { ok: false, reason: 'not-a-search-field' };
  el.scrollIntoView({ block: 'center' });
  el.focus();
  const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  // The native setter: frameworks read the value through it.
  Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, String(spec.text));
  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
  return { ok: true };
}

/** Sends one key to the focused element; Enter submits its form when no page handler did. */
export function pnexPress(key) {
  const codes = { Enter: 13, Tab: 9, Escape: 27, ArrowDown: 40, ArrowUp: 38 };
  if (!(key in codes)) return false;
  const el = document.activeElement || document.body;
  const init = { key, code: key, keyCode: codes[key], which: codes[key], bubbles: true, cancelable: true };
  const down = el.dispatchEvent(new KeyboardEvent('keydown', init));
  el.dispatchEvent(new KeyboardEvent('keypress', init));
  el.dispatchEvent(new KeyboardEvent('keyup', init));
  if (key === 'Enter' && down && el.form) {
    setTimeout(() => (el.form.requestSubmit ? el.form.requestSubmit() : el.form.submit()), 0);
  }
  return true;
}

/** Reads a list of cards (D790): one object of string fields per card, in page order. */
export function pnexCards(spec) {
  const links = ['href', 'src'];
  let cards = [];
  try { cards = [...document.querySelectorAll(spec.selector)]; } catch (err) { return { error: String(err), cards: [] }; }
  const out = [];
  for (const card of cards) {
    const row = {};
    let any = false;
    for (const [name, f] of Object.entries(spec.fields || {})) {
      let el = card;
      try { if (f.selector) el = card.querySelector(f.selector); } catch (err) { el = null; }
      let v = '';
      if (el) {
        if (f.attr && links.includes(f.attr)) v = el[f.attr] || '';
        else if (f.attr) v = el.getAttribute(f.attr) || '';
        else v = (el.innerText || el.textContent || '');
      }
      v = String(v).replace(/\s+/g, ' ').trim();
      row[name] = v;
      if (v) any = true;
    }
    if (any) out.push(row);
    if (out.length >= (spec.max || 50)) break;
  }
  return { cards: out };
}

/** Text of every element matching a selector, joined by blank lines. */
export function pnexSelectorText(selector) {
  try {
    return [...document.querySelectorAll(selector)].map(e => e.innerText.trim()).filter(Boolean).join('\n\n');
  } catch (err) { return ''; }
}

/** Elements matching a selector. */
export function pnexCount(selector) {
  try { return document.querySelectorAll(selector).length; } catch (err) { return 0; }
}
