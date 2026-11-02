---
title: Passkeys
description: How a passkey is made for the agent with your approval, how it signs in, and where its private key lives.
cluster: Security
reviewed: 2026-11-02
---

**A passkey is made once, only for a site you armed and only with your
approval, and after that it signs the agent in by itself.** Its private key
stays in the computer's service and never enters the browser.

## Signing in

The browser's pinned Hotline extension takes every passkey request a page
makes, with the page's origin, and the service signs it for that origin. It
signs in without a prompt; nothing types it and no tool answers it. A site's
passkey autofill is never answered: with the extension as the WebAuthn
proxy, Chromium offers none, so it cannot race the passkey button.

## Registration

1. The client arms the computer for one site:
   `PUT /passkeys/registration {"rpId":"github.com"}`. Arming lasts ten
   minutes, covers one site, answers `{"state":"armed","rpId":…,"expiresAt":…}`,
   and starts the browser on that site if none is open.
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

The registration answer is the one time a private key leaves the computer,
over the bearer-guarded loopback route the client already uses. The
extension's line to the service is a socket only the container's user can
open, but jobs run as that user too: until jobs and the browser have
separate identities, that line is a convenience, not a wall.

`HOTLINE_COMPUTER_PASSKEYS=virtual` keeps the older arrangement, Chromium's
virtual authenticator filled over DevTools on port 9222.
