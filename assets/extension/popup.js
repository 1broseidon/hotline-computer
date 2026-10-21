// The toolbar popup: the logins this computer may fill on the active tab's
// site, each with a Fill button. It names only the tab; the worker reads the
// tab's own origin from Chromium, asks the service, and hands the values to
// the page's filler (autofill.js) directly. They never reach this popup.

const $ = (id) => document.getElementById(id);

function showError(message) {
  $('error').textContent = message || 'Hotline: something went wrong';
  $('error').hidden = false;
}

function ask(message) {
  return chrome.runtime.sendMessage(message).catch((error) => ({ error: { message: `Hotline: ${error.message}` } }));
}

function row(tabId, login) {
  const button = document.createElement('button');
  button.type = 'button';
  button.textContent = 'Fill';
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

  const name = document.createElement('div');
  name.className = 'name';
  name.textContent = login.name;
  const user = document.createElement('div');
  user.className = 'user';
  user.textContent = login.username || '';
  const who = document.createElement('div');
  who.className = 'who';
  who.append(name, user);

  const li = document.createElement('li');
  li.append(who, button);
  return li;
}

// The passkey the desk armed this computer to make, if any, as the worker's
// standing line to the service tells it.
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
  const ask = arming.ask || {};
  $('passkey').className = arming.state;
  $('passkey-title').textContent = shown[0];
  $('passkey-rp').textContent = arming.rpId;
  $('passkey-text').textContent = shown[1] ||
    `${ask.rpName || arming.rpId} asked for a passkey${ask.userName ? ` for ${ask.userName}` : ''}. Approve it at the desk.`;
  expiresAt = arming.state === 'registered' ? null : arming.expiresAt;
  tick();
}

function tick() {
  const left = expiresAt ? Math.max(0, Math.round((expiresAt - Date.now()) / 1000)) : null;
  $('passkey-left').textContent = left === null ? '' : `${Math.floor(left / 60)}:${String(left % 60).padStart(2, '0')} left`;
}
setInterval(tick, 1000);

chrome.runtime.onMessage.addListener((message) => {
  if (message && message.type === 'arming-changed') showPasskey(message.arming);
});
ask({ type: 'arming' }).then(showPasskey);

async function main() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  let url = null;
  try { url = new URL(tab.url); } catch {}
  if (!tab || !url || !/^https?:$/.test(url.protocol)) {
    $('status').textContent = 'Hotline fills logins on web pages.';
    return;
  }
  $('site').textContent = url.host;

  const reply = await ask({ type: 'logins', tabId: tab.id });
  $('status').hidden = true;
  if (reply && reply.error) return showError(reply.error.message);
  const logins = (reply && reply.logins) || [];
  if (!logins.length) {
    $('empty').hidden = false;
    return;
  }
  $('logins').append(...logins.map((login) => row(tab.id, login)));
  $('logins').hidden = false;
}

main().catch((error) => {
  $('status').hidden = true;
  showError(`Hotline: ${error.message}`);
});
