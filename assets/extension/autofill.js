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

  // ---- A note while the agent works ----------------------------------------
  // The worker sends one when a key of this computer was used or refused for
  // the agent. Only the top frame shows it, bottom right, and it goes by
  // itself; nothing on the page moves.
  if (window.top === window) {
    let notes = null;
    const notesRoot = () => {
      if (notes && notes.isConnected) return notes.shadowRoot.querySelector('.stack');
      notes = document.createElement('hotline-notes');
      const style = { all: 'initial', position: 'fixed', right: '12px', bottom: '12px', 'z-index': '2147483647', 'pointer-events': 'none' };
      for (const [k, v] of Object.entries(style)) notes.style.setProperty(k, v, 'important');
      const root = notes.attachShadow({ mode: 'open' });
      const sheet = new CSSStyleSheet();
      sheet.replaceSync(`
        .stack { display: flex; flex-direction: column; gap: 8px; align-items: flex-end; }
        .note { display: flex; gap: 10px; width: 280px; padding: 10px 12px; border: 1px solid #2a2a30; border-radius: 10px;
          background: #17171b; color: #ececef; box-shadow: 0 10px 28px rgba(0,0,0,.35);
          font: 13px/1.45 Geist, system-ui, sans-serif; opacity: 0; transform: translateY(6px); transition: opacity .2s, transform .2s; }
        .note.in { opacity: 1; transform: none; }
        .dot { width: 6px; height: 6px; border-radius: 50%; flex: none; margin-top: 7px; }
        .ok { background: #86d69c; } .fail { background: #ff8f80; }
        .t { font-weight: 500; } .s { color: #8e8e95; font-size: 12px; margin-top: 1px; }`);
      root.adoptedStyleSheets = [sheet];
      const stack = document.createElement('div');
      stack.className = 'stack';
      root.append(stack);
      document.documentElement.append(notes);
      return stack;
    };
    chrome.runtime.onMessage.addListener((message, sender) => {
      if (sender.id !== chrome.runtime.id || !message || message.type !== 'hotline-note') return false;
      const note = document.createElement('div');
      note.className = 'note';
      const dot = document.createElement('span');
      dot.className = `dot ${message.ok ? 'ok' : 'fail'}`;
      const text = document.createElement('div');
      const title = document.createElement('div');
      title.className = 't';
      title.textContent = String(message.title || '');
      const sub = document.createElement('div');
      sub.className = 's';
      sub.textContent = String(message.sub || '');
      text.append(title, sub);
      note.append(dot, text);
      notesRoot().append(note);
      requestAnimationFrame(() => note.classList.add('in'));
      setTimeout(() => { note.classList.remove('in'); setTimeout(() => note.remove(), 250); }, 6000);
      return false;
    });

    // Its own scope: the filler above has a hide() and show() of its own.
    (() => {
      // ---- The card -----------------------------------------------------------
      // Hotline's one surface in the browser: a card at the top right of the
      // page, not a toolbar popup. It opens when the person is needed (the
      // computer handed to them, a passkey being made) or when they click the
      // Hotline mark, and closes on Not now, Escape or the cross. Its root is
      // closed and its buttons take only real clicks, so the page can neither
      // read it nor press it.
      const MARK = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAYAAABzenr0AAABHElEQVR4nOyUSwrCMBCGZ9rqRkGRUryFXsDjKKI7D+JOETxOca+3kCKioBsfHTtUMX2orY/ERX4oDTM/mS9MMgYolgGKpQE0gAZQDmC9MiyXVEJcN3hNVFvU67j/ph8fJTzPKyMWh4jQFuNEMCU6DBzH2X3ifwoQblZwEbGZlieiOdGxdds0r19U6h0IT5K+WZjHJnve9Udy8QD30LK2O8ig06lS5n8ef/xOGEna8AKxgv51bLuK4scx0ZvXH68XAVittiPTNF3IqMA74y+H3+UaYgzF4sEZeiBFOLbtSj8GsDmDvMHkB+0xeSEOIplT0UgsVOmvAHyQJz8FACcgTfdakUl4fYpd+F1rfC5+e4IJABXSr0ADaADlABcAAAD//+Q0FKEAAAAGSURBVAMAf0t8WB/LVrkAAAAASUVORK5CYII=';
      const ask = (message) => chrome.runtime.sendMessage(message).catch((error) => ({ error: { message: `Hotline: ${error.message}` } }));
      const el = (tag, className, text) => {
        const node = document.createElement(tag);
        if (className) node.className = className;
        if (text !== undefined) node.textContent = text;
        return node;
      };
      const press = (button, run) => button.addEventListener('click', (event) => { if (event.isTrusted) run(); });
      const ICONS = {
        key: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="6" cy="6" r="3"/><path d="M8.2 8.2 14 14M11.5 11.5l1.8-1.8"/></svg>',
        lock: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="3" y="7" width="10" height="7" rx="1.5"/><path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2"/></svg>',
        close: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M4 4l8 8M12 4l-8 8"/></svg>',
      };
      const PASSKEY = {
        armed: ['Ready to make a passkey', 'On the site, choose to add a passkey. Hotline then asks for approval at the desk.'],
        asked: ['Waiting for approval', null],
        approved: ['Making the passkey…', 'Approved at the desk.'],
        registered: ['Passkey made', 'Hotline is storing it for this teammate.'],
      };

      let card = null; // the host element, while the card is shown
      let parts = null;
      let state = { arming: { state: 'idle' }, wheel: { driving: 'none', agent: 'Agent', task: '' }, uses: [] };
      let dismissed = ''; // the reason the person closed it for, so it does not reopen for the same one
      let ticking = null;

      function build() {
        card = document.createElement('hotline-card');
        const style = { all: 'initial', position: 'fixed', right: '12px', top: '12px', 'z-index': '2147483647' };
        for (const [k, v] of Object.entries(style)) card.style.setProperty(k, v, 'important');
        const root = card.attachShadow({ mode: 'closed' });
        const sheet = new CSSStyleSheet();
        sheet.replaceSync(`
          * { box-sizing: border-box; }
          [hidden] { display: none !important; }
          .card { width: 316px; max-height: calc(100vh - 24px); overflow: auto; background: #17171b; color: #ececef;
            border: 1px solid #2a2a30; border-radius: 14px; box-shadow: 0 18px 48px rgba(0,0,0,.45);
            font: 13px/1.45 Geist, system-ui, sans-serif; opacity: 0; transform: translateY(-6px); transition: opacity .18s, transform .18s; }
          .card.in { opacity: 1; transform: none; }
          header { display: flex; align-items: center; gap: 8px; height: 42px; padding: 0 8px 0 14px; }
          header img { width: 16px; height: 16px; }
          .name { font-weight: 600; }
          .site { margin-left: auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: #8e8e95; font: 12px 'Geist Mono', ui-monospace, monospace; }
          .x { width: 26px; height: 26px; padding: 0; display: grid; place-items: center; border: 0; background: none; color: #8e8e95; border-radius: 7px; }
          .x:hover { background: #1f1f25; color: #ececef; } .x svg { width: 12px; height: 12px; }
          section { padding: 4px 6px; border-top: 1px solid #2a2a30; }
          .pad { padding: 8px; }
          .chip { display: inline-flex; align-items: center; gap: 6px; height: 22px; padding: 0 8px; border-radius: 6px; font-size: 12px; background: #352d24; color: #f0c27a; }
          .chip i { width: 6px; height: 6px; border-radius: 50%; background: #f0c27a; }
          .t { font-weight: 500; } .s { color: #8e8e95; font-size: 12px; }
          .quote { margin: 0 8px 10px; padding: 6px 10px; border-left: 2px solid #2a2a30; color: #b4b4ba; font-size: 12.5px; }
          .acts { display: flex; gap: 6px; padding: 0 8px 8px; }
          button { font: inherit; cursor: pointer; }
          .b { height: 28px; padding: 0 12px; border: 1px solid #2e2e35; border-radius: 8px; background: #1f1f25; color: #ececef; font-weight: 500; flex: none; }
          .b:hover:not(:disabled) { background: #2e2e35; } .b:disabled { opacity: .45; cursor: default; }
          .b.you { background: #352d24; border-color: transparent; color: #f0c27a; }
          .b.ghost { background: none; border-color: #2a2a30; color: #b4b4ba; }
          button:focus-visible { outline: 2px solid #8fb8ff; outline-offset: 2px; }
          .row { display: flex; align-items: center; gap: 10px; padding: 8px; border-radius: 9px; }
          .row:hover { background: #1f1f25; }
          .ico { width: 28px; height: 28px; border-radius: 8px; background: #1f1f25; border: 1px solid #2e2e35; display: grid; place-items: center; color: #b4b4ba; flex: none; }
          .ico svg { width: 14px; height: 14px; }
          .who { flex: 1; min-width: 0; } .who div { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
          .when { color: #8e8e95; font-size: 12px; flex: none; }
          .sec { padding: 8px 8px 2px; color: #8e8e95; font: 11px 'Geist Mono', ui-monospace, monospace; letter-spacing: .04em; }
          .log { display: flex; gap: 10px; padding: 5px 8px; font-size: 12.5px; color: #b4b4ba; }
          .at { width: 40px; flex: none; color: #8e8e95; font: 12px 'Geist Mono', ui-monospace, monospace; }
          .dot { width: 6px; height: 6px; border-radius: 50%; flex: none; margin-top: 7px; } .ok { background: #86d69c; } .fail { background: #ff8f80; }
          .rp { margin-top: 2px; font: 12px 'Geist Mono', ui-monospace, monospace; }
          .left { margin-top: 6px; color: #8e8e95; font: 11.5px 'Geist Mono', ui-monospace, monospace; }
          .note { margin: 0; padding: 10px 8px; color: #b4b4ba; }
          .err { color: #ff8f80; }`);
        root.adoptedStyleSheets = [sheet];
        const box = el('div', 'card');
        box.setAttribute('role', 'dialog');
        box.setAttribute('aria-label', 'Hotline');
        const head = el('header');
        const mark = el('img'); mark.src = MARK; mark.alt = '';
        const close = el('button', 'x'); close.type = 'button'; close.innerHTML = ICONS.close; close.setAttribute('aria-label', 'Close');
        press(close, () => hide(true));
        head.append(mark, el('span', 'name', 'Hotline'), el('span', 'site', location.host), close);

        const turn = el('section'); turn.hidden = true;
        const turnTask = el('div', 'quote');
        const handBack = el('button', 'b you'); handBack.type = 'button';
        const notNow = el('button', 'b ghost', 'Not now'); notNow.type = 'button';
        const chipRow = el('div', 'pad'); const chip = el('span', 'chip'); chip.append(el('i'), 'Your turn'); chipRow.append(chip);
        const turnTitle = el('div', 'pad'); turnTitle.style.paddingTop = '0'; turnTitle.append(el('div', 't', 'You have the computer.'));
        const acts = el('div', 'acts'); acts.append(handBack, notNow);
        turn.append(chipRow, turnTitle, turnTask, acts);
        press(handBack, async () => {
          handBack.disabled = true;
          const reply = await ask({ type: 'card-hand-back' });
          handBack.disabled = false;
          if (reply && reply.error) return fail(reply.error.message);
          hide(false);
        });
        press(notNow, () => hide(true));

        const passkey = el('section', ''); passkey.hidden = true;
        const pk = el('div', 'pad'); const pkTitle = el('div', 't'); const pkRp = el('div', 'rp'); const pkText = el('div', 's'); pkText.style.marginTop = '6px'; const pkLeft = el('div', 'left');
        pk.append(pkTitle, pkRp, pkText, pkLeft); passkey.append(pk);

        const keys = el('section'); keys.hidden = true;
        const record = el('section'); record.hidden = true;
        const status = el('p', 'note'); status.hidden = true;
        box.append(head, turn, passkey, keys, record, status);
        root.append(box);
        root.addEventListener('keydown', (event) => { if (event.key === 'Escape') hide(true); });
        parts = { box, turn, turnTask, handBack, passkey, pkTitle, pkRp, pkText, pkLeft, keys, record, status };
        document.documentElement.append(card);
        requestAnimationFrame(() => box.classList.add('in'));
      }

      function fail(message) {
        parts.status.hidden = false;
        parts.status.className = 'note err';
        parts.status.textContent = message || 'Hotline: something went wrong';
      }

      function row(icon, title, sub, trailing) {
        const r = el('div', 'row');
        const ico = el('div', 'ico'); ico.innerHTML = ICONS[icon];
        const who = el('div', 'who'); who.append(el('div', 't', title), el('div', 's', sub));
        r.append(ico, who, trailing);
        return r;
      }

      async function loadKeys() {
        const reply = await ask({ type: 'card-keys' });
        if (!card) return;
        if (reply && reply.error) return fail(reply.error.message);
        const rows = [
          ...(reply.passkeys || []).map((p) => row('key', p.userName || p.rpId, 'passkey · used when the page asks', el('span'))),
          ...(reply.logins || []).map((login) => {
            const fill = el('button', 'b', 'Fill'); fill.type = 'button';
            fill.setAttribute('aria-label', `Fill ${login.name}${login.username ? ` (${login.username})` : ''}`);
            press(fill, async () => {
              fill.disabled = true;
              const done = await ask({ type: 'card-fill', name: login.name });
              if (done && done.error) { fill.disabled = false; return fail(done.error.message); }
              fill.textContent = 'Filled';
              setTimeout(() => hide(false), 600);
            });
            return row('lock', login.username || login.name, `${login.name} · from this computer`, fill);
          }),
        ];
        parts.keys.replaceChildren(...rows);
        parts.keys.hidden = !rows.length;
        quiet();
      }

      // Nothing to show at all: say so in one line rather than an empty card.
      function quiet() {
        const empty = parts.turn.hidden && parts.passkey.hidden && parts.keys.hidden && parts.record.hidden;
        if (parts.status.classList.contains('err')) return;
        parts.status.hidden = !empty;
        parts.status.textContent = 'Nothing for this site. Grant a login or passkey from the desk.';
      }

      function within(site) {
        const host = location.hostname;
        return host === site || host.endsWith(`.${site}`) || site.endsWith(`.${host}`);
      }

      function draw() {
        if (!card) return;
        const { wheel, arming, uses } = state;
        const yours = wheel.driving === 'person';
        parts.turn.hidden = !yours;
        parts.turnTask.hidden = !wheel.task;
        parts.turnTask.textContent = wheel.task ? `${wheel.agent} was: ${wheel.task}` : '';
        parts.handBack.textContent = `Hand back to ${wheel.agent}`;

        const shown = arming && PASSKEY[arming.state];
        parts.passkey.hidden = !shown;
        if (shown) {
          const asked = arming.ask || {};
          parts.pkTitle.textContent = shown[0];
          parts.pkRp.textContent = arming.rpId;
          parts.pkText.textContent = shown[1] || `${asked.rpName || arming.rpId} asked for a passkey${asked.userName ? ` for ${asked.userName}` : ''}. Approve it at the desk.`;
          clearInterval(ticking);
          const tick = () => {
            const left = arming.state === 'registered' || !arming.expiresAt ? null : Math.max(0, Math.round((arming.expiresAt - Date.now()) / 1000));
            parts.pkLeft.textContent = left === null ? '' : `${Math.floor(left / 60)}:${String(left % 60).padStart(2, '0')} left`;
          };
          tick();
          ticking = setInterval(tick, 1000);
        }

        const midnight = new Date(); midnight.setHours(0, 0, 0, 0);
        const here = (uses || []).filter((u) => within(u.site) && u.ts >= midnight.getTime()).slice(-4).reverse();
        parts.record.hidden = !here.length;
        parts.record.replaceChildren(el('div', 'sec', 'today here'), ...here.map((u) => {
          const line = el('div', 'log');
          const at = new Date(u.ts);
          line.append(
            el('span', 'at', `${String(at.getHours()).padStart(2, '0')}:${String(at.getMinutes()).padStart(2, '0')}`),
            el('span', `dot ${u.ok ? 'ok' : 'fail'}`),
            el('span', '', u.who === 'you' ? `You ${u.what.replace(/^was /, 'were ')}` : `${u.who} ${u.what}`),
          );
          return line;
        }));
        quiet();
      }

      async function show() {
        if (card && card.isConnected) return;
        build();
        const now = await ask({ type: 'card-state' });
        if (now && !now.error) state = now;
        draw();
        loadKeys();
      }

      function hide(byPerson) {
        if (byPerson) dismissed = reason();
        clearInterval(ticking);
        if (!card) return;
        const gone = card;
        card = null;
        parts.box.classList.remove('in');
        setTimeout(() => gone.remove(), 200);
      }

      // Why the card would open by itself now, if at all.
      function reason() {
        if (state.wheel.driving === 'person') return 'turn';
        if (['armed', 'asked'].includes(state.arming.state)) return `passkey-${state.arming.rpId}`;
        return '';
      }

      chrome.runtime.onMessage.addListener((message, sender) => {
        if (sender.id !== chrome.runtime.id || !message) return false;
        if (message.type === 'hotline-card') {
          if (card) hide(true); else { dismissed = ''; show(); }
        } else if (message.type === 'hotline-state' && message.state) {
          state = message.state;
          const why = reason();
          if (!why) dismissed = '';
          if (card) {
            draw();
            // The turn handed back or the passkey stored: nothing left to ask of the person.
            if (!why && message.settled) setTimeout(() => hide(false), message.settled);
          } else if (why && why !== dismissed && document.visibilityState === 'visible') {
            show();
          }
        }
        return false;
      });
    })();
  }
})();
