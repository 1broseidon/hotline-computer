// Where a login's field may be typed: the one rule, shared by the broker's
// `browser fill` (src/signin/mod.rs evaluates this file in an isolated world)
// and the extension's autofill (assets/extension/autofill.js loads it first).
//
// hotlineFillPolicy(element, kind, origins, stopsGet) answers {ok, why, print}:
// - kind "username": an <input> of type text or email.
// - kind "password": an <input type=password>, as the DOM reports it now,
//   either in no form at all (a page built by script that sends it itself:
//   nothing the browser does on its own sends it anywhere), or inside a form
//   whose every submission is POST to one of `origins`. With `stopsGet`, the
//   caller stops any page load whose address carries what was typed (the
//   signed-in browser does), so a form sending by GET to one of `origins`
//   is taken too: a script-built form with no method, sent by its script.
// - kind "code": a one-time-code field under the same form rule: an <input>
//   with autocomplete=one-time-code of type text, tel, number or password,
//   or one of type text, tel or number (or inputmode=numeric) with a
//   maxlength of 6 to 8. The form rule is what keeps the code out of an
//   address and the history; the shape is what keeps it out of a search box
//   or a comment.
// - always: the field's frame is the top frame or a frame of the same
//   origin, and that origin is exactly one of `origins` (no subdomain).
// `print` is what must still hold right before and right after typing: a
// type, form, method or destination changed in between refuses the fill. A
// plain submit button the page adds as the field fills changes none of them.
// eslint-disable-next-line no-unused-vars
function hotlineFillPolicy(el, kind, origins, stopsGet) {
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
  // Only a button that sends elsewhere or otherwise than its form matters.
  const submitters = form
    ? [...form.elements].filter((s) => ((s instanceof HTMLButtonElement && s.type === 'submit') ||
        (s instanceof HTMLInputElement && (s.type === 'submit' || s.type === 'image'))) &&
        (s.getAttribute('formmethod') !== null || s.getAttribute('formaction') !== null))
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
  if (!form) return { ok: true, print };
  const own = origins.includes(destination(form.action));
  const sent = (method, home) => method === 'post' || (stopsGet === true && method === 'get' && home);
  if (!sent(form.method, own)) return refuse(`its form sends by ${form.method.toUpperCase()}, which would put it in the address`);
  if (!own) return refuse(`its form sends to ${destination(form.action) || 'nowhere known'}, not the login's own origin`);
  for (const s of submitters) {
    const home = s.getAttribute('formaction') === null || origins.includes(destination(s.formAction));
    if (s.getAttribute('formmethod') !== null && !sent(s.formMethod, home)) return refuse('a button of its form sends by GET');
    if (!home) return refuse('a button of its form sends elsewhere');
  }
  return { ok: true, print };
}
