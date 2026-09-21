// Runs in every new document of a tab that carries the teammate's passkeys
// (see src/browser.rs). A site may ask the browser to sign in with a passkey
// whenever it likes; it may make one only while the person has armed this
// computer for that site. The armed site is baked in when the script is
// installed, and __hotlineArm changes it on a document already open; the
// token it wants is this browser's alone and no page ever sees it.
(() => {
  const token = __TOKEN__;
  let armed = __ARMED__;
  const within = (rp) => armed !== null && (rp === armed || rp.endsWith('.' + armed) || armed.endsWith('.' + rp));
  if (typeof globalThis.__hotlineArm === 'function') {
    globalThis.__hotlineArm(token, armed);
    return;
  }
  const proto = CredentialsContainer.prototype;
  const create = proto.create;
  Object.defineProperty(proto, 'create', {
    configurable: false, writable: false, enumerable: true,
    value: function (options) {
      if (options && options.publicKey) {
        const rp = (options.publicKey.rp && options.publicKey.rp.id) || location.hostname;
        if (!within(rp)) {
          return Promise.reject(new DOMException(
            `Hotline: making a passkey for ${rp} is not armed. The person arms it in Settings → Secrets → Add passkey.`,
            'NotAllowedError'));
        }
      }
      return create.apply(this, arguments);
    },
  });
  Object.defineProperty(globalThis, '__hotlineArm', {
    configurable: false, writable: false, enumerable: false,
    value: (presented, rp) => {
      if (presented !== token) return false;
      armed = rp;
      return true;
    },
  });
})();
