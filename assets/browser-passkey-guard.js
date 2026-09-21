// Runs in every new document of a tab that carries the teammate's passkeys
// (see src/browser.rs). A site may ask the browser to sign in with a passkey
// whenever it likes; it may make one only while the person has armed this
// computer for that site, and then only once the person has approved the
// request the site makes. Under an arming, a request is parked here with
// what the site asked for — the site, the origin, the account — until the
// service, which looks in on every action and every poll, carries the
// person's answer back: approved, the browser makes the passkey; denied, or
// the arming gone, the site gets the NotAllowedError it would get from a
// person cancelling the browser's own prompt. The armed site is baked in
// when the script is installed, and __hotlineArm changes it on a document
// already open; the token every door here wants is this browser's alone and
// no page ever sees it.
(() => {
  const token = __TOKEN__;
  let armed = __ARMED__;
  let parked = null;
  const within = (rp) => armed !== null && (rp === armed || rp.endsWith('.' + armed) || armed.endsWith('.' + rp));
  const refusal = (message) => new DOMException(`Hotline: ${message}`, 'NotAllowedError');
  if (typeof globalThis.__hotlineArm === 'function') {
    globalThis.__hotlineArm(token, armed);
    return;
  }
  const proto = CredentialsContainer.prototype;
  const create = proto.create;
  Object.defineProperty(proto, 'create', {
    configurable: false, writable: false, enumerable: true,
    value: function (options) {
      if (!options || !options.publicKey) {
        return create.apply(this, arguments);
      }
      const key = options.publicKey;
      const rp = (key.rp && key.rp.id) || location.hostname;
      if (!within(rp)) {
        return Promise.reject(refusal(
          `making a passkey for ${rp} is not armed. The person arms it in the teammate's pane.`));
      }
      if (parked !== null) {
        return Promise.reject(refusal('another passkey request on this page is waiting for the person.'));
      }
      const container = this;
      const args = arguments;
      const text = (value) => (typeof value === 'string' ? value : null);
      return new Promise((resolve, reject) => {
        parked = {
          id: Math.random().toString(36).slice(2) + Date.now().toString(36),
          rp,
          origin: location.origin,
          rpName: text(key.rp && key.rp.name),
          userName: text(key.user && key.user.name),
          userDisplayName: text(key.user && key.user.displayName),
          resolve,
          reject,
          run: () => create.apply(container, args),
        };
      });
    },
  });
  Object.defineProperty(globalThis, '__hotlineArm', {
    configurable: false, writable: false, enumerable: false,
    value: (presented, rp) => {
      if (presented !== token) return false;
      armed = rp;
      if (parked !== null && !within(parked.rp)) {
        const { rp: site, reject } = parked;
        parked = null;
        reject(refusal(`the arming for ${site} ended before the person answered.`));
      }
      return true;
    },
  });
  // What is parked, for the service's look: what the site asked for, never
  // the promise's ends.
  Object.defineProperty(globalThis, '__hotlineLook', {
    configurable: false, writable: false, enumerable: false,
    value: (presented) => {
      if (presented !== token || parked === null) return null;
      const { id, rp, origin, rpName, userName, userDisplayName } = parked;
      return { id, rpId: rp, origin, rpName, userName, userDisplayName };
    },
  });
  // The person's answer to what is parked, by its id.
  Object.defineProperty(globalThis, '__hotlineAnswer', {
    configurable: false, writable: false, enumerable: false,
    value: (presented, id, approved) => {
      if (presented !== token || parked === null || parked.id !== id) return false;
      const request = parked;
      parked = null;
      if (approved === true) {
        request.run().then(request.resolve, request.reject);
      } else {
        request.reject(refusal('the person did not approve making this passkey.'));
      }
      return true;
    },
  });
})();
