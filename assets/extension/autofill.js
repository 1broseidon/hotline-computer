// The inline login filler. Runs in the extension's own world in every frame,
// so the page sees neither this code nor its messages. It looks for a login
// form; only then does it ask the worker which of the person's logins this
// frame's origin may use, and only if there are some does it offer them: a
// small Hotline button inside the right edge of the focused field. A login's
// values are fetched when one is clicked, typed into the fields, and dropped.
// The worker, not this script, decides the origin (see service-worker.js).
(() => {
  if (globalThis.__hotlineAutofill || document.documentElement?.namespaceURI !== 'http://www.w3.org/1999/xhtml') return;
  globalThis.__hotlineAutofill = true;

  const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
  const TEXTUAL = new Set(['text', 'email', 'tel']);
  const CODE_WORDS = /otp|totp|2fa|code/i;
  // Fields named "code" that are not a one-time code.
  const NOT_CODE = /zip|post|promo|coupon|voucher|gift|discount|country|area|region|invite|referr|product|qr/i;
  const RECEIPT_MS = 2500;
  const FAIL_MS = 4000;

  let asking = null; // the one request for this frame's logins
  let logins = []; // what came back; names and usernames only

  // ---- Finding the fields ----

  const tokens = (el) => (el.getAttribute('autocomplete') || '').toLowerCase().split(/\s+/);
  const scopeOf = (el) => el.form || el.getRootNode();
  const before = (a, b) => !!(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);

  function usable(el) {
    return el instanceof HTMLInputElement && el.isConnected && !el.disabled && !el.readOnly &&
      el.getClientRects().length > 0 &&
      (!el.checkVisibility || el.checkVisibility({ visibilityProperty: true, checkVisibilityCSS: true }));
  }
  const inputs = (scope) => [...scope.querySelectorAll('input')].filter(usable);

  function isCode(el) {
    if (tokens(el).includes('one-time-code')) return true;
    if (!TEXTUAL.has(el.type) && el.type !== 'number') return false;
    const words = `${el.name} ${el.id}`;
    return CODE_WORDS.test(words) && !NOT_CODE.test(words);
  }

  // The username for a password field: the field before it that says it is
  // one (or is an email field), else the nearest text field before it.
  function usernameFor(password) {
    const earlier = inputs(scopeOf(password)).filter((el) => TEXTUAL.has(el.type) && !isCode(el) && before(el, password));
    return earlier.findLast((el) => el.type === 'email' || tokens(el).some((t) => t === 'username' || t === 'email')) ||
      earlier.at(-1) || null;
  }

  function isUsername(el) {
    if (!TEXTUAL.has(el.type) || isCode(el)) return false;
    // A username asked for on its own page, before the password's.
    if (tokens(el).includes('username')) return true;
    return inputs(scopeOf(el)).some((other) => other.type === 'password' && usernameFor(other) === el);
  }

  function kindOf(el) {
    if (!usable(el)) return null;
    if (el.type === 'password') return 'password';
    if (isCode(el)) return 'code';
    if (isUsername(el)) return 'username';
    return null;
  }

  // Whether the page holds a login field at all, shown or not yet: cheap, and
  // only decides whether to ask which logins exist (which reveals no secret).
  function hasLoginField() {
    for (const el of document.querySelectorAll('input')) {
      if (el.type === 'hidden') continue;
      if (el.type === 'password' || isCode(el) || tokens(el).includes('username')) return true;
    }
    return false;
  }

  // The fields one login fills, around the field it was asked from.
  function fieldsNear(anchor) {
    const all = inputs(scopeOf(anchor));
    const kind = kindOf(anchor);
    let password = kind === 'password' ? anchor : null;
    if (!password && kind === 'username') password = all.find((el) => el.type === 'password' && before(anchor, el)) || null;
    const username = password ? usernameFor(password) : kind === 'username' ? anchor : null;
    const code = kind === 'code' ? anchor : all.find(isCode) || null;
    return { username, password, code };
  }

  // The field the popup's fill starts from: the focused one, else the page's first login field.
  function anchorInPage() {
    let active = document.activeElement;
    while (active && active.shadowRoot && active.shadowRoot.activeElement) active = active.shadowRoot.activeElement;
    if (active && kindOf(active)) return active;
    const all = inputs(document);
    return all.find((el) => el.type === 'password') ||
      all.find((el) => tokens(el).includes('username') && kindOf(el)) ||
      all.find(isCode) || null;
  }

  // ---- Filling ----

  // Typed the way a framework will notice: the native setter, then input and change.
  function type(el, value) {
    el.focus();
    setValue.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true, composed: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  }

  // Fills what the values cover around an anchor; returns the last field filled.
  let busy = false;
  function fill(anchor, values, codeOnly) {
    const fields = fieldsNear(anchor);
    let last = null;
    busy = true;
    try {
      if (!codeOnly && fields.username && values.username) type((last = fields.username), values.username);
      if (!codeOnly && fields.password && values.password) type((last = fields.password), values.password);
      if (fields.code && values.code) type((last = fields.code), values.code);
    } finally {
      busy = false;
    }
    return last;
  }

  async function send(message) {
    try {
      return (await chrome.runtime.sendMessage(message)) || {};
    } catch {
      // The extension was reloaded under this page: this copy is spent.
      if (!chrome.runtime?.id) stop();
      return { error: { name: 'NotAllowedError', message: 'Hotline: this computer did not answer' } };
    }
  }

  function load() {
    asking ||= send({ type: 'logins' }).then((reply) => {
      logins = Array.isArray(reply.logins) ? reply.logins : [];
      if (!logins.length) stop();
      return logins;
    });
    return asking;
  }

  // ---- The chip, its dropdown and its receipt, all in one shadow root ----

  const MARK = [['rect', { x: 4, y: 30, width: 56, height: 18, rx: 6 }], ['circle', { cx: 20, cy: 30, r: 10.5 }], ['circle', { cx: 44, cy: 30, r: 10.5 }]];
  const CSS = `
    * { box-sizing: border-box; }
    [hidden] { display: none !important; }
    button { margin: 0; border: 0; font: inherit; color: inherit; cursor: pointer; background: transparent; }
    button:focus-visible { outline: 2px solid #8fb8ff; outline-offset: 2px; }
    .chip, .menu, .receipt { pointer-events: auto; font: 13px/1.3 Geist, system-ui, sans-serif; color: #ececef; }
    .chip { position: absolute; right: 6px; top: 50%; transform: translateY(-50%); display: grid; place-items: center;
      width: 22px; height: 22px; padding: 0; border: 1px solid #2e2e35; border-radius: 6px; background: #17171b; }
    .chip:hover, .chip[aria-expanded=true] { background: #1f1f25; }
    svg { width: 14px; height: 14px; flex: none; fill: currentColor; }
    .menu { position: absolute; right: 0; top: calc(100% + 4px); min-width: 220px; max-width: 300px; padding: 4px;
      border: 1px solid #2e2e35; border-radius: 10px; background: rgba(23,23,27,.97); box-shadow: 0 18px 40px rgba(0,0,0,.5); }
    .menu.up { top: auto; bottom: calc(100% + 4px); }
    .menu button { display: flex; flex-direction: column; align-items: flex-start; width: 100%; padding: 7px 10px; border-radius: 7px; text-align: left; }
    .menu button:hover, .menu button:focus-visible { background: #1f1f25; outline: none; }
    .menu span { max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .name { font-weight: 500; }
    .sub { color: #8e8e95; font-size: 12px; }
    .receipt { position: absolute; right: 6px; top: 50%; transform: translateY(-50%); display: flex; align-items: center; gap: 6px;
      height: 22px; max-width: 320px; padding: 0 8px; border: 1px solid #2e2e35; border-radius: 11px; background: #17171b;
      font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; pointer-events: none; }
    .receipt:empty { display: none; }
    .receipt.fail { color: #ff8f80; }
  `;

  function h(tag, attrs = {}, ...children) {
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
    el.append(...children);
    return el;
  }
  function mark() {
    const ns = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(ns, 'svg');
    svg.setAttribute('viewBox', '0 0 64 64');
    svg.setAttribute('aria-hidden', 'true');
    for (const [tag, attrs] of MARK) {
      const shape = document.createElementNS(ns, tag);
      for (const [k, v] of Object.entries(attrs)) shape.setAttribute(k, v);
      svg.append(shape);
    }
    return svg;
  }

  let host = null, chip, menu, receipt;
  let field = null; // the field the chip sits in
  let receiptTimer = 0;
  let sizes = null;

  function build() {
    host = document.createElement('hotline-autofill');
    // Out of the page's flow and out of its styles: nothing here moves the page.
    const style = { all: 'initial', position: 'fixed', display: 'block', width: '0', top: '0', left: '0', 'z-index': '2147483647', 'pointer-events': 'none' };
    for (const [k, v] of Object.entries(style)) host.style.setProperty(k, v, 'important');
    const root = host.attachShadow({ mode: 'open' });
    // A constructed sheet, not a <style>, so a page's style-src policy has no say.
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(CSS);
    root.adoptedStyleSheets = [sheet];
    chip = h('button', { type: 'button', class: 'chip', 'aria-label': 'Hotline: fill a login', 'aria-expanded': 'false', 'aria-controls': 'menu' }, mark());
    menu = h('div', { id: 'menu', class: 'menu', role: 'group', 'aria-label': 'Hotline logins', hidden: '' });
    receipt = h('div', { class: 'receipt', role: 'status' });
    root.append(chip, menu, receipt);
    // Pressing the chip or an item keeps focus in the field, as a dropdown should.
    root.addEventListener('mousedown', (e) => e.preventDefault());
    chip.addEventListener('click', () => (menu.hidden ? openMenu() : closeMenu()));
    root.addEventListener('keydown', onMenuKey);
    sizes = new ResizeObserver(place);
  }

  function place() {
    if (!host || !field) return;
    if (!field.isConnected) return hide();
    const r = field.getBoundingClientRect();
    const out = r.bottom < 0 || r.top > innerHeight || r.width === 0;
    host.style.setProperty('visibility', out ? 'hidden' : 'visible', 'important');
    host.style.setProperty('top', `${r.top}px`, 'important');
    host.style.setProperty('left', `${r.right}px`, 'important');
    host.style.setProperty('height', `${r.height}px`, 'important');
  }
  let placing = 0;
  const placeSoon = () => (placing ||= requestAnimationFrame(() => { placing = 0; place(); }));

  function attachTo(el) {
    if (!host) build();
    if (field !== el) {
      if (field) sizes.unobserve(field);
      field = el;
      sizes.observe(el);
    }
    if (!host.isConnected) {
      document.documentElement.append(host);
      addEventListener('scroll', placeSoon, { capture: true, passive: true });
      addEventListener('resize', placeSoon, { passive: true });
    }
    place();
  }

  function show(el) {
    attachTo(el);
    if (!receipt.textContent) chip.hidden = false;
  }

  function hide() {
    if (!host || receipt.textContent) return; // a receipt stays for its moment
    closeMenu();
    if (field) sizes.unobserve(field);
    field = null;
    host.remove();
    removeEventListener('scroll', placeSoon, { capture: true });
    removeEventListener('resize', placeSoon);
  }

  function openMenu() {
    if (!field) return;
    const near = fieldsNear(field);
    const items = [];
    for (const login of logins) {
      if (near.username || near.password) {
        const item = h('button', { type: 'button', 'aria-label': `Fill ${login.name} (${login.username || 'no username'})` },
          h('span', { class: 'name' }, login.name), h('span', { class: 'sub' }, login.username || ''));
        item.addEventListener('click', () => choose(login, false));
        items.push(item);
      }
      if (login.code && near.code) {
        const item = h('button', { type: 'button', 'aria-label': `Fill code from ${login.name}` },
          h('span', { class: 'name' }, 'Fill code'), h('span', { class: 'sub' }, login.name));
        item.addEventListener('click', () => choose(login, true));
        items.push(item);
      }
    }
    if (!items.length) return;
    menu.replaceChildren(...items);
    const r = field.getBoundingClientRect();
    menu.classList.toggle('up', r.bottom + 48 * items.length + 16 > innerHeight && r.top > innerHeight / 2);
    menu.hidden = false;
    chip.setAttribute('aria-expanded', 'true');
    // Opened from the keyboard (focus on the chip): carry focus into the list.
    if (document.activeElement === host) items[0].focus();
  }

  function closeMenu() {
    if (!menu || menu.hidden) return;
    const hadFocus = document.activeElement === host;
    menu.hidden = true;
    menu.replaceChildren();
    chip.setAttribute('aria-expanded', 'false');
    if (hadFocus && field) field.focus();
  }

  function onMenuKey(e) {
    if (e.key === 'Escape') { e.stopPropagation(); closeMenu(); return; }
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    const items = [...menu.querySelectorAll('button')];
    if (!items.length) return;
    e.preventDefault();
    const at = items.indexOf(host.shadowRoot.activeElement);
    items[(at + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length].focus();
  }

  function showReceipt(el, text, failed) {
    attachTo(el);
    closeMenu();
    clearTimeout(receiptTimer);
    chip.hidden = true;
    receipt.classList.toggle('fail', !!failed);
    receipt.textContent = text;
    receiptTimer = setTimeout(() => {
      receipt.textContent = '';
      chip.hidden = false;
      if (field && field.getRootNode().activeElement === field) place();
      else hide();
    }, failed ? FAIL_MS : RECEIPT_MS);
  }

  // A login picked from the dropdown: fetch its values, type them, let them go.
  async function choose(login, codeOnly) {
    const anchor = field;
    closeMenu();
    if (!anchor) return;
    const reply = await send({ type: 'fill', name: login.name });
    if (reply.error) return showReceipt(anchor, reply.error.message || 'Hotline could not fill this login', true);
    if (codeOnly && !reply.code) return showReceipt(anchor, `Hotline: no code for ${login.name}`, true);
    const last = anchor.isConnected && fill(anchor, reply, codeOnly);
    if (last) showReceipt(last, `Filled from Hotline · ${login.name}`);
    else showReceipt(anchor, 'Hotline: nothing here to fill', true);
  }

  // ---- Following focus ----

  function onFocusIn(e) {
    if (busy || e.target === host) return;
    const el = e.composedPath()[0];
    if (!(el instanceof HTMLInputElement) || !kindOf(el)) return hide();
    load().then((list) => {
      if (list.length && el.getRootNode().activeElement === el) show(el);
    });
  }

  function onFocusOut(e) {
    if (busy || !field || e.composedPath()[0] !== field || e.relatedTarget === host) return;
    // Let focus land first: it may be moving into the dropdown.
    setTimeout(() => {
      if (document.activeElement !== host && field && field.getRootNode().activeElement !== field) hide();
    }, 0);
  }

  function onPointerDown(e) {
    if (host && !e.composedPath().includes(host)) closeMenu();
  }

  // ---- Watching for login forms, cheaply ----

  let scanTimer = 0;
  const observer = new MutationObserver(() => {
    scanTimer ||= setTimeout(() => { scanTimer = 0; scan(); }, 400);
  });

  function scan() {
    if (asking) return;
    if (hasLoginField()) {
      observer.disconnect();
      load();
    }
  }

  function watch() {
    scan();
    if (!asking) observer.observe(document.documentElement, { childList: true, subtree: true });
  }

  function stop() {
    observer.disconnect();
    clearTimeout(scanTimer);
    removeEventListener('focusin', onFocusIn, true);
    removeEventListener('focusout', onFocusOut, true);
    removeEventListener('pointerdown', onPointerDown, true);
    clearTimeout(receiptTimer);
    if (receipt) receipt.textContent = '';
    hide();
  }

  addEventListener('focusin', onFocusIn, true);
  addEventListener('focusout', onFocusOut, true);
  addEventListener('pointerdown', onPointerDown, true);
  // Leaving the page (or into the back/forward cache): take everything down; back again, look again.
  addEventListener('pagehide', () => {
    observer.disconnect();
    clearTimeout(receiptTimer);
    if (receipt) receipt.textContent = '';
    hide();
  });
  addEventListener('pageshow', (e) => { if (e.persisted && !asking) watch(); });
  watch();

  // The popup's fill, sent by the worker to this tab's top frame with the values.
  chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
    if (sender.id !== chrome.runtime.id || !message || message.type !== 'hotline-fill') return false;
    if (message.origin !== location.origin) {
      sendResponse({ error: { name: 'NotAllowedError', message: 'Hotline: the page changed before it could be filled' } });
      return false;
    }
    const anchor = anchorInPage();
    const last = anchor && fill(anchor, message, false);
    if (last) showReceipt(last, `Filled from Hotline · ${message.name}`);
    sendResponse(last ? { filled: true } : { error: { name: 'NotFoundError', message: 'Hotline: no login form on this page' } });
    return false;
  });
})();
