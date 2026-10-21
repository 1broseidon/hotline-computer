// Runs in every page's own world before the page does. A person's browser
// answers a conditional passkey request (passkeys offered in a field's
// autofill) only when the person picks one; nothing on this computer picks
// from autofill, so say there is none, and leave a request made anyway
// waiting until the site gives up on it. Answered at once, it would race the
// passkey button the agent then clicks, and the site would refuse one.
(() => {
  const proto = CredentialsContainer.prototype;
  const get = proto.get;
  Object.defineProperty(proto, 'get', {
    configurable: false, writable: false, enumerable: true,
    value: function (options) {
      if (!options || !options.publicKey || options.mediation !== 'conditional') {
        return get.apply(this, arguments);
      }
      const signal = options.signal;
      return new Promise((_, reject) => {
        if (!signal) return;
        const abort = () => reject(signal.reason ?? new DOMException('The operation was aborted.', 'AbortError'));
        if (signal.aborted) abort();
        else signal.addEventListener('abort', abort, { once: true });
      });
    },
  });
  if (typeof PublicKeyCredential !== 'undefined') {
    Object.defineProperty(PublicKeyCredential, 'isConditionalMediationAvailable', {
      configurable: false, writable: false, enumerable: true,
      value: () => Promise.resolve(false),
    });
    const capabilities = PublicKeyCredential.getClientCapabilities;
    if (typeof capabilities === 'function') {
      Object.defineProperty(PublicKeyCredential, 'getClientCapabilities', {
        configurable: false, writable: false, enumerable: true,
        value: () => capabilities.call(PublicKeyCredential).then((all) => ({ ...all, conditionalGet: false })),
      });
    }
  }
})();
