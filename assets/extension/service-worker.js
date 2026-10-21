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

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (sender.id !== chrome.runtime.id || !message || !['logins', 'fill'].includes(message.type)) return false;
  const popup = !sender.tab && typeof sender.url === 'string' && sender.url.startsWith(chrome.runtime.getURL('popup.html'));
  const work = popup ? fromPopup(message) : sender.tab ? fromPage(message, sender) : Promise.resolve(refusal('unknown sender'));
  work.then(sendResponse, (error) => sendResponse(refusal(error && error.message ? error.message : 'something went wrong')));
  return true;
});
