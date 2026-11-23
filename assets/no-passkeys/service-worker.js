// The visible browser on the split computer: passkeys sign in only in the
// signed-in browser, so this one holds none. As Chromium's WebAuthn proxy,
// this answers every request at once with a refusal, where Chromium's own
// dialog would wait for a security key nobody can touch. No page script,
// no native host: nothing here reaches the service. See src/browser.rs.

const proxy = chrome.webAuthenticationProxy;
const refusal = {
  name: 'NotAllowedError',
  message: 'Hotline: passkeys sign in only in the signed-in browser on this computer, not in this one',
};

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
  proxy.completeIsUvpaaRequest({ requestId, isUvpaa: false }).catch(() => {});
});
proxy.onGetRequest.addListener(({ requestId }) => {
  proxy.completeGetRequest({ requestId, error: refusal }).catch(() => {});
});
proxy.onCreateRequest.addListener(({ requestId }) => {
  proxy.completeCreateRequest({ requestId, error: refusal }).catch(() => {});
});
