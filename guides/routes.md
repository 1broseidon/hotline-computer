---
title: HTTP routes
description: Every route the computer serves, which ones need the token, and the settings that change where and how it listens.
cluster: Reference
reviewed: 2026-11-02
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
| `PUT /secrets` | header | The secret set. See [Secrets](/guides/secrets/) |
| `/passkeys/registration`, `/answer` | header | Passkey registration. See [Passkeys](/guides/passkeys/) |
| `/logins/{name}` | header | A stored login |
| `GET /pages` | header | The sites the browser has open |

A header token is `Authorization: Bearer <token>`; a query token is
`?token=<token>`, because a browser cannot send a header on a WebSocket.
Without the right one, every protected route answers a JSON 401. With no
`HOTLINE_COMPUTER_TOKEN` set at all, every route is open.

`X-Computer-Holder` names the client using a lease or run slot; without it
the holder is `anonymous`.

## Pages

`GET /pages` lists one entry per top-level tab on an `http://` or `https://`
page, newest first, so a client can offer you cookies for those sites:

```json
{"pages":[{"origin":"https://porkbun.com","url":"https://porkbun.com/account","title":"…","openedAt":1793580000000}]}
```

`openedAt` is when the computer first saw the tab, in milliseconds, and keeps
its time as the tab moves between sites. Chromium's own pages, the
extension's, DevTools, blank tabs, frames and workers are never listed. The
list comes from DevTools' target list, so it never waits on the browser and
never starts one. A computer older than 0.11.0-rc.8 answers 404.

## Settings

| Variable | Default | |
| --- | --- | --- |
| `HOTLINE_COMPUTER_ADDR` | `0.0.0.0:8787` | Where the server listens inside the container |
| `HOTLINE_COMPUTER_TOKEN` | unset | The bearer; unset means open |
| `HOTLINE_COMPUTER_HOME` | `/home/agent` | The directory `files` is confined to |
| `HOTLINE_COMPUTER_SCREEN` | `1920x1080` | The display `boot` creates |
| `DISPLAY` | `:0` | The display `boot` creates and `serve` uses |

`hotline-computer serve` serves on a display that already exists, for
running outside the container.
