// Where a login's field may be typed: the one rule, shared by the broker's
// `browser fill` (src/signin/mod.rs evaluates this file in an isolated world)
// and the extension's autofill (assets/extension/autofill.js loads it first).
//
// hotlineFillPolicy(element, kind, origins) answers {ok, why, print}:
// - kind "username": an <input> of type text or email.
// - kind "password": an <input type=password>, as the DOM reports it now,
//   inside a form whose every submission is POST to one of `origins`.
// - kind "code": a one-time-code field inside such a form: an <input> with
//   autocomplete=one-time-code of type text, tel, number or password, or one
//   of type text, tel or number (or inputmode=numeric) with a maxlength of 6
//   to 8. The form rule is what keeps the code out of an address and the
//   history; the shape is what keeps it out of a search box or a comment.
// - always: the field's frame is the top frame or a frame of the same
//   origin, and that origin is exactly one of `origins` (no subdomain).
// `print` is what must still hold right before and right after typing: a
// type, method or destination changed in between refuses the fill.
// eslint-disable-next-line no-unused-vars
function hotlineFillPolicy(el, kind, origins) {
  const refuse = (why) => ({ ok: false, why });
  if (!el || !(el instanceof HTMLInputElement)) return refuse('it is not a text field (an <input>)');
  if (!el.isConnected) return refuse('it is no longer on the page');
  if (window !== window.top) {
    let top = null;
    try { top = window.top.location.origin; } catch { top = null; }
    if (top !== location.origin) return refuse('it is inside a frame of another site');
  }
  if (!origins.includes(location.origin)) {
    return refuse(`this page is ${location.origin}, not exactly the login's own origin (${origins.join(', ')})`);
  }
  const type = el.type;
  const destination = (url) => { try { return new URL(url, location.href).origin; } catch { return ''; } };
  const form = el.form;
  const submitters = form
    ? [...form.elements].filter((s) => (s instanceof HTMLButtonElement && s.type === 'submit') ||
        (s instanceof HTMLInputElement && (s.type === 'submit' || s.type === 'image')))
    : [];
  const print = JSON.stringify([kind, type, el.name, form ? form.method : null, form ? form.action : null,
    submitters.map((s) => [s.getAttribute('formmethod') === null ? null : s.formMethod,
      s.getAttribute('formaction') === null ? null : s.formAction]), location.href]);
  if (kind === 'username') {
    if (!['text', 'email'].includes(type)) return refuse(`a username goes into a text or email field, and this is type=${type}`);
    return { ok: true, print };
  }
  if (kind === 'password' && type !== 'password') {
    return refuse(`a password goes only into a password field (type=password), and this is type=${type}`);
  }
  if (kind === 'code') {
    const tokens = (el.getAttribute('autocomplete') || '').toLowerCase().split(/\s+/);
    const max = parseInt(el.getAttribute('maxlength') || '', 10);
    const marked = tokens.includes('one-time-code') && ['text', 'tel', 'number', 'password'].includes(type);
    // inputmode=numeric changes only the keyboard: the type still decides.
    const shaped = ['text', 'tel', 'number'].includes(type) && max >= 6 && max <= 8;
    if (!marked && !shaped) {
      return refuse('a code goes only into a one-time-code field (autocomplete=one-time-code, or a numeric field of 6 to 8 characters)');
    }
  }
  if (kind !== 'password' && kind !== 'code') return refuse(`there is no ${kind} field rule`);
  if (!form) return refuse('it is not inside a form, so where it would be sent is not known');
  if (form.method !== 'post') return refuse(`its form sends by ${form.method.toUpperCase()}, which would put it in the address`);
  if (!origins.includes(destination(form.action))) return refuse(`its form sends to ${destination(form.action) || 'nowhere known'}, not the login's own origin`);
  for (const s of submitters) {
    if (s.getAttribute('formmethod') !== null && s.formMethod !== 'post') return refuse('a button of its form sends by GET');
    if (s.getAttribute('formaction') !== null && !origins.includes(destination(s.formAction))) return refuse('a button of its form sends elsewhere');
  }
  return { ok: true, print };
}
