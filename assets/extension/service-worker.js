// The teammate's passkeys, answered by this computer. Chromium hands every
// WebAuthn request a page makes to this extension (webAuthenticationProxy),
// with the page's origin; the extension carries it to the computer's
// service through native messaging, and the service signs or mints there.
// A private key never enters the browser. See src/bridge.rs and
// src/authenticator.rs.

const HOST = 'dev.hotline.computer';
const proxy = chrome.webAuthenticationProxy;
// The port each waiting request rides on, so a page that gives up closes it.
const waiting = new Map();

function ask(requestId, message) {
  return new Promise((resolve) => {
    let port;
    try {
      port = chrome.runtime.connectNative(HOST);
    } catch (error) {
      resolve({ error: { name: 'NotAllowedError', message: `Hotline: this computer did not answer (${error.message})` } });
      return;
    }
    let answered = false;
    waiting.set(requestId, port);
    port.onMessage.addListener((reply) => {
      answered = true;
      waiting.delete(requestId);
      port.disconnect();
      resolve(reply);
    });
    port.onDisconnect.addListener(() => {
      waiting.delete(requestId);
      if (!answered) {
        const why = chrome.runtime.lastError ? ` (${chrome.runtime.lastError.message})` : '';
        resolve({ error: { name: 'NotAllowedError', message: `Hotline: this computer did not answer${why}` } });
      }
    });
    port.postMessage(message);
  });
}

async function attach() {
  try {
    await proxy.attach();
  } catch (error) {
    // Already attached is the steady state after the worker wakes.
    if (!/already/i.test(String(error && error.message))) console.error('Hotline:', error);
  }
}
chrome.runtime.onInstalled.addListener(attach);
chrome.runtime.onStartup.addListener(attach);
attach();

proxy.onIsUvpaaRequest.addListener(({ requestId }) => {
  proxy.completeIsUvpaaRequest({ requestId, isUvpaa: true }).catch(() => {});
});

function answer(complete, kind) {
  return async ({ requestId, requestDetailsJson }) => {
    const reply = await ask(requestId, { kind, details: JSON.parse(requestDetailsJson) });
    const done = reply.error
      ? { requestId, error: reply.error }
      : { requestId, responseJson: JSON.stringify(reply.response) };
    // A request the page already gave up on cannot be completed; that is fine.
    complete(done).catch(() => {});
  };
}
proxy.onGetRequest.addListener(answer((d) => proxy.completeGetRequest(d), 'get'));
proxy.onCreateRequest.addListener(answer((d) => proxy.completeCreateRequest(d), 'create'));
proxy.onRequestCanceled.addListener((requestId) => {
  const port = waiting.get(requestId);
  waiting.delete(requestId);
  if (port) port.disconnect();
});

// The person's logins, filled into a page the way a password manager would.
// The service answers only for the origin it is given, so that origin always
// comes from Chromium: the sending frame's own origin for the inline filler
// (autofill.js), the tab's current page for the popup. A message never names
// its own origin. Values are fetched on a click, handed to the one frame that
// fills them, and not kept: no storage, no cache, no log.

const refusal = (message) => ({ error: { name: 'NotAllowedError', message: `Hotline: ${message}` } });

// The origin of a web page, or null for anything that is not one.
function webOrigin(origin, url) {
  try {
    const parsed = new URL(origin && origin !== 'null' ? origin : url);
    return /^https?:$/.test(parsed.protocol) ? parsed.origin : null;
  } catch {
    return null;
  }
}

async function tabOrigin(tabId) {
  const tab = await chrome.tabs.get(tabId);
  return webOrigin(null, tab.url);
}

// Which logins may be used on an origin: names and usernames, never passwords.
async function loginsFor(origin) {
  const reply = await ask(crypto.randomUUID(), { kind: 'logins', origin });
  if (reply.error) return { error: reply.error };
  const logins = (reply.logins || []).map(({ name, username, code }) => ({ name, username, code: !!code }));
  return { logins };
}

// One login's values for an origin; the service refuses a login not allowed there.
async function valuesFor(origin, name) {
  const reply = await ask(crypto.randomUUID(), { kind: 'fill', origin, name: String(name) });
  if (reply.error) return { error: reply.error };
  return { username: reply.username, password: reply.password, code: reply.code ?? null };
}

// The inline filler, from a frame of a page: that frame's origin, and its values back to it alone.
async function fromPage(message, sender) {
  const origin = webOrigin(sender.origin, sender.url);
  if (!origin) return refusal('this page has no web origin');
  if (message.type === 'logins') return loginsFor(origin);
  return valuesFor(origin, message.name);
}

// The popup, for the active tab: the tab's top-level origin, and the values
// go to that tab's top frame, not back to the popup.
async function fromPopup(message) {
  const { tabId } = message;
  if (!Number.isInteger(tabId)) return refusal('no tab to fill');
  const origin = await tabOrigin(tabId);
  if (!origin) return refusal('this tab is not a web page');
  if (message.type === 'logins') return loginsFor(origin);
  const values = await valuesFor(origin, message.name);
  if (values.error) return values;
  // The tab may have moved on while the service answered; fill only the page it answered for.
  if ((await tabOrigin(tabId)) !== origin) return refusal('the page changed before it could be filled');
  try {
    const filled = await chrome.tabs.sendMessage(tabId, { type: 'hotline-fill', origin, name: message.name, ...values }, { frameId: 0 });
    return filled || refusal('this page did not answer; reload it and try again');
  } catch {
    // No filler in the page yet (it loaded before the extension did), or a page Chromium keeps scripts out of.
    return refusal('this page cannot be filled; reload it and try again');
  }
}

// The passkeys this computer may sign with on the active tab's site: which
// account each is for, never a key. The origin comes from Chromium.
async function passkeysFor(tabId) {
  if (!Number.isInteger(tabId)) return refusal('no tab');
  const origin = await tabOrigin(tabId);
  if (!origin) return { passkeys: [] };
  const reply = await ask(crypto.randomUUID(), { kind: 'passkeys', origin });
  return reply.error ? { error: reply.error } : { passkeys: reply.passkeys || [] };
}

// The sites whose offered card the person closed: not offered again until
// the worker restarts.
const passedOver = new Set();

// The card in a tab's top frame asks for that tab only: Chromium names the
// tab, never the page.
async function fromCard(message, tabId) {
  if (message.type === 'card-state') return { arming, wheel, uses };
  if (message.type === 'card-dismiss' || message.type === 'card-offer') {
    const origin = await tabOrigin(tabId);
    if (!origin) return { offer: false };
    const host = new URL(origin).hostname;
    if (message.type === 'card-dismiss') {
      passedOver.add(host);
      return {};
    }
    if (passedOver.has(host)) return { offer: false };
    const [passkeys, logins] = await Promise.all([passkeysFor(tabId), loginsFor(origin)]);
    return { offer: (passkeys.passkeys || []).length + (logins.logins || []).length > 0 };
  }
  if (message.type === 'card-hand-back') return ask(crypto.randomUUID(), { kind: 'hand_back' });
  if (message.type === 'card-fill') return fromPopup({ type: 'fill', tabId, name: message.name });
  const origin = await tabOrigin(tabId);
  if (!origin) return { passkeys: [], logins: [] };
  const [passkeys, logins] = await Promise.all([passkeysFor(tabId), loginsFor(origin)]);
  if (logins.error) return logins;
  return { passkeys: passkeys.passkeys || [], logins: logins.logins || [] };
}

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (sender.id !== chrome.runtime.id || !message || !String(message.type).startsWith('card-')) return false;
  if (!sender.tab || sender.frameId !== 0 || !Number.isInteger(sender.tab.id)) return false;
  fromCard(message, sender.tab.id).then(sendResponse, (error) => sendResponse(refusal(error && error.message ? error.message : 'something went wrong')));
  return true;
});

// The mark in the toolbar opens and closes the card on the page. Where
// Chromium keeps scripts out (its own pages, the Web Store), the old popup
// stands in.
chrome.action.onClicked.addListener(async (tab) => {
  try {
    await chrome.tabs.sendMessage(tab.id, { type: 'hotline-card' }, { frameId: 0 });
  } catch {
    await chrome.action.setPopup({ popup: 'popup.html' });
    await chrome.action.openPopup().catch(() => {});
    await chrome.action.setPopup({ popup: '' });
  }
});

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (sender.id !== chrome.runtime.id || !message) return false;
  const popup = !sender.tab && typeof sender.url === 'string' && sender.url.startsWith(chrome.runtime.getURL('popup.html'));
  if (!popup || !['passkeys', 'hand-back'].includes(message.type)) return false;
  const work = message.type === 'passkeys'
    ? passkeysFor(message.tabId)
    : ask(crypto.randomUUID(), { kind: 'hand_back' });
  work.then(sendResponse, (error) => sendResponse(refusal(error && error.message ? error.message : 'something went wrong')));
  return true;
});

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (sender.id !== chrome.runtime.id || !message || !['logins', 'fill'].includes(message.type)) return false;
  const popup = !sender.tab && typeof sender.url === 'string' && sender.url.startsWith(chrome.runtime.getURL('popup.html'));
  const work = popup ? fromPopup(message) : sender.tab ? fromPage(message, sender) : Promise.resolve(refusal('unknown sender'));
  work.then(sendResponse, (error) => sendResponse(refusal(error && error.message ? error.message : 'something went wrong')));
  return true;
});

// ---- the passkey being made -------------------------------------------------
// A standing line to the service: the arming as the desk sets it. When the
// desk arms this computer, the card opens on the page so the person sees it is
// ready to make the passkey; the badge says where it stands until the passkey
// is made or the arming ends. The line carries no key, only the state.

let arming = { state: 'idle' };
// Who has the computer, and what its keys did lately: the same standing line.
let wheel = { driving: 'none', agent: 'Agent', task: '' };
let uses = [];
let lastUse = null;
const BADGES = {
  armed: ['KEY', '#8fb8ff'],
  asked: ['1', '#f0c27a'],
  approved: ['…', '#8fb8ff'],
  registered: ['✓', '#86d69c'],
};

function show(next) {
  arming = next;
  badge();
  // An open popup redraws; with none open there is nobody to tell.
  chrome.runtime.sendMessage({ type: 'arming-changed', arming: next }).catch(() => {});
}

// The icon lights only when the person is needed: a passkey being made, or
// the computer handed to them. Otherwise it stays plain.
function badge() {
  let [text, color] = BADGES[arming.state] || ['', '#8fb8ff'];
  if (!text && wheel.driving === 'person') [text, color] = ['1', '#f0c27a'];
  chrome.action.setBadgeText({ text }).catch(() => {});
  if (text) chrome.action.setBadgeBackgroundColor({ color }).catch(() => {});
  const title = arming.state !== 'idle' ? `Hotline: a passkey for ${arming.rpId}`
    : wheel.driving === 'person' ? `Hotline: your turn on the computer` : 'Hotline';
  chrome.action.setTitle({ title }).catch(() => {});
}

// A note on the page for a key used or refused for the agent. The person's
// own uses need no note: they were there.
const NOTE = {
  'signed in with the passkey': (u) => [`${u.who} signed in to ${u.site}`, 'with the passkey on this computer'],
  'made a passkey': (u) => [`${u.who} made a passkey for ${u.site}`, 'kept on this computer'],
  'filled a login': (u) => [`${u.who} filled a login on ${u.site}`, 'from this computer'],
  'was refused a passkey': (u) => [`Refused a passkey on ${u.site}`, `no key here is for this site; ${u.who} was stopped`],
  'was refused a login': (u) => [`Refused a login on ${u.site}`, `not a site the login is for; ${u.who} was stopped`],
};

async function note(use) {
  const words = NOTE[use.what];
  if (!words || use.who === 'you') return;
  const [title, sub] = words(use);
  const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
  if (tab && Number.isInteger(tab.id)) {
    chrome.tabs.sendMessage(tab.id, { type: 'hotline-note', ok: use.ok, title, sub }, { frameId: 0 }).catch(() => {});
  }
}

function heard(message) {
  const before = { driving: wheel.driving, arming: arming.state };
  if (message.passkeys) show(message.passkeys);
  if (message.wheel) {
    const changed = message.wheel.driving !== wheel.driving;
    wheel = message.wheel;
    if (changed) badge();
  }
  if (Array.isArray(message.uses)) {
    // The first line after connecting is history: no notes for it.
    const fresh = lastUse === null ? [] : message.uses.filter((u) => u.id > lastUse);
    uses = message.uses;
    lastUse = uses.length ? uses[uses.length - 1].id : (lastUse ?? 0);
    fresh.forEach(note);
  }
  const state = { arming, wheel, uses };
  chrome.runtime.sendMessage({ type: 'state-changed', state }).catch(() => {});
  // The card on the active page: it opens itself when the person is needed,
  // and steps away a beat after the turn is handed back or the passkey kept.
  const settled = (before.driving === 'person' && wheel.driving !== 'person') ? 400
    : (before.arming !== 'registered' && arming.state === 'registered') ? 2500 : 0;
  tellActiveTab({ type: 'hotline-state', state, settled });
}

async function tellActiveTab(message) {
  const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
  if (tab && Number.isInteger(tab.id)) chrome.tabs.sendMessage(tab.id, message, { frameId: 0 }).catch(() => {});
}


function watch() {
  let port;
  try {
    port = chrome.runtime.connectNative(HOST);
  } catch {
    setTimeout(watch, 3000);
    return;
  }
  port.onMessage.addListener((message) => {
    if (message) heard(message);
  });
  port.onDisconnect.addListener(() => {
    wheel = { driving: 'none', agent: wheel.agent, task: '' };
    show({ state: 'idle' });
    setTimeout(watch, 3000);
  });
  port.postMessage({ kind: 'watch' });
}
watch();

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (sender.id !== chrome.runtime.id || !message) return false;
  if (message.type === 'arming') sendResponse(arming);
  else if (message.type === 'state') sendResponse({ arming, wheel, uses });
  else return false;
  return false;
});
