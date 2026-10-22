// The toolbar popup. It shows only what the person needs: the computer handed
// to them, a passkey being made, this site's keys, and what those keys did
// here today. It names only the tab; the worker reads the tab's own origin
// from Chromium and asks the service. Values go from the worker to the page's
// filler (autofill.js) directly and never reach this popup; no key ever does.

const $ = (id) => document.getElementById(id);

function showError(message) {
  $('error').textContent = message || 'Hotline: something went wrong';
  $('error').hidden = false;
}

function ask(message) {
  return chrome.runtime.sendMessage(message).catch((error) => ({ error: { message: `Hotline: ${error.message}` } }));
}

const ICONS = {
  key: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="6" cy="6" r="3"/><path d="M8.2 8.2 14 14M11.5 11.5l1.8-1.8"/></svg>',
  lock: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="3" y="7" width="10" height="7" rx="1.5"/><path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2"/></svg>',
};

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function row(icon, title, sub, trailing) {
  const li = el('li', 'row');
  const ico = el('div', 'ico');
  ico.innerHTML = ICONS[icon];
  const who = el('div', 'who');
  who.append(el('div', 't', title), el('div', 's', sub));
  li.append(ico, who, trailing);
  return li;
}

function loginRow(tabId, login) {
  const button = el('button', '', 'Fill');
  button.type = 'button';
  button.setAttribute('aria-label', `Fill ${login.name}${login.username ? ` (${login.username})` : ''}`);
  button.addEventListener('click', async () => {
    $('error').hidden = true;
    button.disabled = true;
    const reply = await ask({ type: 'fill', tabId, name: login.name });
    if (reply && reply.error) {
      button.disabled = false;
      showError(reply.error.message);
      return;
    }
    button.textContent = 'Filled';
    setTimeout(() => window.close(), 500);
  });
  return row('lock', login.username || login.name, `${login.name} · from this computer`, button);
}

function passkeyRow(passkey) {
  return row('key', passkey.userName || passkey.rpId, 'passkey · on this computer', el('span', 'when', 'when the page asks'));
}

// ---- the computer handed to the person ---------------------------------------
let state = { arming: { state: 'idle' }, wheel: { driving: 'none', agent: 'Agent', task: '' }, uses: [] };

function showTurn() {
  const { wheel } = state;
  const yours = wheel.driving === 'person';
  $('turn').hidden = !yours;
  if (!yours) return;
  $('turn-title').textContent = `You have the computer. ${wheel.agent} is waiting.`;
  $('turn-task').hidden = !wheel.task;
  $('turn-task').textContent = wheel.task ? `${wheel.agent} was: ${wheel.task}` : '';
  $('hand-back').textContent = `Hand back to ${wheel.agent}`;
  $('foot').textContent = `${wheel.agent} is waiting. Nothing you type here reaches it.`;
}

$('hand-back').addEventListener('click', async () => {
  $('hand-back').disabled = true;
  const reply = await ask({ type: 'hand-back' });
  if (reply && reply.error) {
    $('hand-back').disabled = false;
    showError(reply.error.message);
    return;
  }
  window.close();
});
$('not-now').addEventListener('click', () => window.close());

// ---- a passkey being made ----------------------------------------------------
const PASSKEY = {
  armed: ['Ready to make a passkey', 'On the site, choose to add a passkey. Hotline then asks for approval at the desk.'],
  asked: ['Waiting for approval', null],
  approved: ['Making the passkey…', 'Approved at the desk.'],
  registered: ['Passkey made', 'Hotline is storing it for this teammate.'],
};
let expiresAt = null;

function showPasskey(arming) {
  const shown = arming && PASSKEY[arming.state];
  $('passkey').hidden = !shown;
  if (!shown) return;
  const asked = arming.ask || {};
  $('passkey-title').textContent = shown[0];
  $('passkey-rp').textContent = arming.rpId;
  $('passkey-text').textContent = shown[1] ||
    `${asked.rpName || arming.rpId} asked for a passkey${asked.userName ? ` for ${asked.userName}` : ''}. Approve it at the desk.`;
  expiresAt = arming.state === 'registered' ? null : arming.expiresAt;
  tick();
}

function tick() {
  const left = expiresAt ? Math.max(0, Math.round((expiresAt - Date.now()) / 1000)) : null;
  $('passkey-left').textContent = left === null ? '' : `${Math.floor(left / 60)}:${String(left % 60).padStart(2, '0')} left`;
}
setInterval(tick, 1000);

// ---- what this site's keys did today -----------------------------------------
let host = null;

function within(site) {
  return host && (host === site || host.endsWith(`.${site}`) || site.endsWith(`.${host}`));
}

function showRecord() {
  const midnight = new Date();
  midnight.setHours(0, 0, 0, 0);
  const here = (state.uses || []).filter((u) => within(u.site) && u.ts >= midnight.getTime()).slice(-4).reverse();
  $('record').hidden = !here.length;
  $('record-rows').replaceChildren(...here.map((u) => {
    const line = el('div', 'log');
    const at = new Date(u.ts);
    line.append(
      el('span', 'at', `${String(at.getHours()).padStart(2, '0')}:${String(at.getMinutes()).padStart(2, '0')}`),
      el('span', `dot ${u.ok ? 'ok' : 'fail'}`),
      el('span', '', `${u.who === 'you' ? 'You' : u.who} ${u.what}`),
    );
    return line;
  }));
}

function redraw() {
  showTurn();
  showPasskey(state.arming);
  showRecord();
}

chrome.runtime.onMessage.addListener((message) => {
  if (!message) return;
  if (message.type === 'arming-changed') showPasskey(message.arming);
  if (message.type === 'state-changed' && message.state) {
    state = message.state;
    redraw();
  }
});

async function main() {
  const now = await ask({ type: 'state' });
  if (now && !now.error) state = now;
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  let url = null;
  try { url = new URL(tab.url); } catch {}
  if (!tab || !url || !/^https?:$/.test(url.protocol)) {
    redraw();
    $('status').textContent = 'Hotline fills logins and answers passkeys on web pages.';
    return;
  }
  host = url.hostname;
  $('site').textContent = url.host;
  redraw();

  const [logins, passkeys] = await Promise.all([ask({ type: 'logins', tabId: tab.id }), ask({ type: 'passkeys', tabId: tab.id })]);
  $('status').hidden = true;
  if (logins && logins.error) showError(logins.error.message);
  const rows = [
    ...((passkeys && passkeys.passkeys) || []).map(passkeyRow),
    ...((logins && logins.logins) || []).map((login) => loginRow(tab.id, login)),
  ];
  $('key-rows').replaceChildren(...rows);
  $('keys').hidden = !rows.length;
  $('empty').hidden = rows.length > 0 || !$('turn').hidden || !$('passkey').hidden;
}

main().catch((error) => {
  $('status').hidden = true;
  showError(`Hotline: ${error.message}`);
});
