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
