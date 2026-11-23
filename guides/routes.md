---
title: HTTP routes
description: HTTP endpoints, authentication requirements and server settings.
cluster: Reference
section: configuration
reviewed: 2026-11-18
---

**One server answers everything on port 8787: the MCP endpoint, the viewer,
and a few routes for the client that started the computer.** Only `/health`
and the viewer page are open; everything else needs the token.

## Routes

| Route | Token | For |
| --- | --- | --- |
| `GET /health` | none | Liveness |
| `GET /` | none | The viewer page |
| `/mcp` | header | The eight tools, over streamable HTTP |
| `/ws` | query | The viewer's socket. See [Viewer](/guides/viewer/) |
| `/files`, `/files/download` | query | The viewer's file panel |
| `GET /capabilities?nonce=` | header | Which computer this is. See below |
| `POST /capabilities/key` | header | This boot's attest key, once. See below |
| `PUT /secrets` | header | The secret set, v1 or v2. See [Secrets](/guides/secrets/) |
| `GET /secrets/uses`, `POST /secrets/uses/answer` | header | Uses of an ask-me key waiting for you, and your answer |
| `/passkeys/registration`, `/answer` | header | Passkey registration. See [Passkeys](/guides/passkeys/) |
| `PUT`, `DELETE /sessions/{name}` | header | A shared sign-in, into the signed-in browser and out of it. Split computer only |
| `GET /signin/pending` | header | Sign-ins waiting for you to finish them |
| `/sites/ws` | query | The signed-in browser as a screen. Split computer only |
| `GET /migration`, `POST /migration/answer` | header | The first split boot of an old home. Split computer only |
| `DELETE /logins/{name}` | header | Takes a saved login back out of the browser |
| `GET /pages` | header | The sites the browser has open |

A header token is `Authorization: Bearer <token>`; a query token is
`?token=<token>`, because a browser cannot send a header on a WebSocket.
Without the right one, every protected route answers a JSON 401. With no
`HOTLINE_COMPUTER_TOKEN` the computer refuses to start, unless
`HOTLINE_COMPUTER_ALLOW_NO_TOKEN=1` says to run with every route open.

A route marked split computer only answers 404 on the single-user computer.

`X-Computer-Holder` names the client using a lease or run slot; without it
the holder is `anonymous`.

## Pages

`GET /pages` lists one entry per top-level tab on an `http://` or `https://`
page, newest first, so a client can offer you cookies for those sites:

```json
{"pages":[{"origin":"https://porkbun.com","url":"https://porkbun.com/account","title":"…","openedAt":1793580000000}]}
```

`openedAt` records when the computer first observed the tab, in milliseconds. It
does not change when the tab navigates. Chromium's own pages, extension pages,
DevTools, blank tabs, frames and workers are excluded. The list comes from
DevTools' target list without waiting on or starting the browser. Versions older
than 0.11.0-rc.8 return 404.

## Capabilities

`GET /capabilities?nonce=N` takes a nonce of 1 to 256 characters and answers:

```json
{"version":"0.13.0-rc.3","boot":"…","attest":"…",
 "protected":{"v":2,"mode":"split","loopback":"readable","other":"readable","readable":["LOCALAPP"]}}
```

`mode` is `split` when the computer keeps protected keys, and `legacy` for the
single-user computer or a split computer whose key proxy did not start.
`boot` changes with every start, so a new one means everything must be
delivered again. `readable` names what the computer holds but cannot keep
from its jobs: a login or passkey for a loopback site (`localhost`,
`*.localhost`, 127.0.0.0/8, `::1`; a job may serve that page) and a protected
key for an "Other" service (one not wholly on GitHub's, OpenAI's or
Anthropic's origins). Label those readable.

`attest` proves this boot's door answered. Each boot makes a 32-byte key in
the door's own memory (the broker's, in the split computer), independent of
the token. `POST /capabilities/key` hands it to the first caller with the
token, once: `{"v":2,"boot":"…","key":"<base64 of 32 bytes>"}`, then `409`.
Take it right after starting the computer, and again on a new `boot`; if it
was already taken, trust nothing that boot attests. Then

```
attest = base64(HMAC-SHA256(key, "hotline-broker-v2|<boot>|<mode>|<version>|<nonce>"))
```

with `boot`, `mode` and `version` as in the same answer, so a proof of one
mode, boot or release is none for another. A client that knew only v1 (an
HMAC under the token of `hotline-broker-v1|<boot>|<nonce>`) cannot read
`v: 2` as split, and treats the computer as one it cannot verify.
Versions before 0.13.0 return 404.

## Uses

`GET /secrets/uses` lists `{"uses":[…]}`, each with `id`, `secret`, `kind`,
`site`, `path`, `askedAt` and `expiresAt`. A use the signed-in browser asked
for also carries `operation` (`kind` `signin` or `passkey`, `origin`,
`account`, and `rpId` for a passkey), `attested`, and `flow`. Allow then covers
that sign-in only, or only this use when `attested` is false. `POST /secrets/uses/answer {"id":…,"answer":"approve"|"allow"|"deny"}`
answers `{"state":…}`, or 409 when nothing with that id is waiting.

## Sign-ins

`PUT /sessions/{name} {"sites":[…],"cookies":[…],"storage":{…}}` makes those
sites signed-in sites and loads the cookies and site storage into the
signed-in browser alone; nothing is written in the agent's home. A site is a
host or an origin. `DELETE /sessions/{name}`, with `{"domains":[…]}` or no
body for all of it, clears those sites' cookies and data and closes their
tabs.

`GET /signin/pending` lists `{"pending":[{"kind":"finish","name":…,"origin":…,"account":…,"since":…}]}`:
a sign-in where a secret was typed and the tab still shows the same page 30
seconds on. `since` is in milliseconds. An entry goes when the tab shows
another page, closes, or its login is revoked. The single-user computer
answers an empty list.

`/sites/ws?token=…&site=host` speaks the viewer's protocol (see
[Viewer](/guides/viewer/)) for the signed-in browser's tab on that site,
opening the site when no tab is on it. Input goes to the tab over DevTools,
never the X display. While you drive it, the agent sees nothing of the site.
This is what Open Browser shows on the split computer.

## Migration

`GET /migration` answers `{"state":"none"}` when nothing was migrated, else
`{"state":"pending"|"done","at":…,"snapshots":[{"name":…,"bytes":…}],"loginsDropped":…,"sitesCleared":[…],"answer":…}`.
`POST /migration/answer {"snapshots":"clean"|"delete"|"keep"}` cleans the
quarantined snapshots of the old browser profile, saved logins and job
output, deletes them, or leaves them quarantined. See Upgrading to 0.13.0 in
the [manual](/).

## Settings

| Variable | Default | |
| --- | --- | --- |
| `HOTLINE_COMPUTER_ADDR` | `0.0.0.0:8787` | Where the server listens inside the container |
| `HOTLINE_COMPUTER_TOKEN` | unset | The bearer; unset refuses to start |
| `HOTLINE_COMPUTER_ALLOW_NO_TOKEN` | unset | `1` starts without a token, every route open |
| `HOTLINE_COMPUTER_HOME` | `/home/agent` | The directory `files` is confined to |
| `HOTLINE_COMPUTER_SCREEN` | `1920x1080` | The display `boot` creates |
| `DISPLAY` | `:0` | The display `boot` creates and `serve` uses |
| `HOTLINE_COMPUTER_ALLOWED_HOSTS` | unset | DNS names the computer may be reached by, comma-separated |
| `HOTLINE_COMPUTER_ALLOWED_ORIGINS` | unset | Other origins whose pages may call it, comma-separated |

`hotline-computer serve` serves on a display that already exists, for
running outside the container.
