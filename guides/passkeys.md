---
title: Passkeys
description: Passkey registration, approval, storage and sign-in.
cluster: Security
section: security
reviewed: 2026-11-18
---

**Creating a passkey requires registration to be enabled for the site and your
approval of its request.** In the default mode, the service signs requests
without sending the private key to the browser. During registration, it returns
the new credential, including the private key, to the client for storage.

## Signing in

Use the site's passkey button to sign in. The pinned Hotline extension forwards
the request and the page's origin to the service, which signs for that origin.
Passkey autofill is unavailable while the extension acts as Chromium's WebAuthn
proxy.

On the split computer the extension is loaded only in the signed-in browser,
and a passkey answers only for the origin of a signed-in tab: one of a login's
sites, a shared site, or the site armed for registration. The visible browser
has no extension and no passkey. You see the signed-in browser through Open
Browser in the desk.

## Registration

1. The client arms the computer for one site:
   `PUT /passkeys/registration {"rpId":"github.com"}`. Arming lasts ten
   minutes, covers one site, answers `{"state":"armed","rpId":…,"expiresAt":…}`,
   and starts the browser on that site if none is open. On the split computer
   that is the signed-in browser.
2. While armed, a `navigator.credentials.create()` for that site waits. The
   service records what the site asked for: the site, the origin, and the
   account's name and display name.
3. `GET /passkeys/registration` answers `asked` with that request (`id`,
   `rpId`, `origin`, `rpName`, `userName`, `userDisplayName`, `askedAt`). The
   client shows you a card.
4. Your answer goes back with
   `POST /passkeys/registration/answer {"id":…,"approved":true}`.
5. Approved, the service mints a P-256 key and answers the site. `GET` then
   answers `approved`, and `registered` with the new `credential`, until the
   client stores it, sends the secret set with it, and ends the arming with
   `DELETE`.

Denied, the site gets a `NotAllowedError`, as if you had cancelled the
browser's own prompt, and the arming ends, so neither the site nor the agent
can keep asking. An answer to a request that is not waiting is a 409. A
`create()` for any other site, or with nothing armed, is refused with a
`NotAllowedError` that says so. The browser holds one request at a time, and
a site that gives up withdraws its request.

## Limits

During registration, the service returns the credential, including its private
key, to the client over the authenticated loopback route.

On the single-user computer, the extension connects to the service through a
Unix socket restricted to the container's user. Jobs run as that same user and
can access the socket. On the split computer the socket is
`/run/hotline-computer/bridge/bridge.sock`, and it answers only the signed-in
browser's user, `signin`; the private key is held by the broker's user. A job
reaches neither.

On the single-user computer, `HOTLINE_COMPUTER_PASSKEYS=virtual` keeps the
older arrangement, Chromium's virtual authenticator filled over DevTools on
port 9222.
