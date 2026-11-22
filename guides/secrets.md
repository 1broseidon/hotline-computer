---
title: Secrets
description: Secret types, access rules and redaction limits.
cluster: Security
section: security
reviewed: 2026-11-18
---

**Secrets are supplied by name. Readable variables are added to job
environments, protected keys reach only their service, logins are filled on
their configured sites, and passkeys are used by the signing service.** Tools
redact secret values in text responses. Code running in a job can still read
the readable variables in its environment.

## Kinds

The client that started the computer supplies the secrets with `PUT /secrets`,
using a bearer token in the `Authorization` header. The JSON body maps each name
to an entry. The update replaces the entire set, which the service keeps in
memory without writing it to disk. Names must be valid shell variable names,
such as `GITHUB_TOKEN`. Names beginning with `HOTLINE_` and reserved names such
as `PATH` and `HOME` are rejected. An invalid entry causes the entire update to
fail with HTTP 400, identifying the entry and leaving the previous set
unchanged.

| Kind | Entry | Goes to |
| --- | --- | --- |
| Variable | `{"kind":"variable","value":…}`, or a bare string | An environment variable in every job started through `shell` or `files run`, not in preparation jobs. `browser fill` refuses one. 8 characters to 64 KiB |
| Login | `{"kind":"login","sites":[…],"username":…,"password":…,"totp":…}` | Never a job. `browser fill` types `NAME.username`, `NAME.password` or `NAME.code` (the current TOTP digits) only onto a page whose origin is one of the login's sites. On the split computer the signed-in browser types it |
| Passkey | `{"kind":"passkey","rpId":…,"credentialId":…,"privateKey":…,"userHandle":…,"userName":…}` | The service, which signs the site's request; the private key never enters the browser. See [Passkeys](/guides/passkeys/) |

A login's site is an `https://` origin, or `http://` on localhost. The page a
password is typed into can read it, so which page it is typed into is the
whole protection.

A login or a passkey with `"use": "ask"` is held until you approve each use,
through `GET /secrets/uses` and `POST /secrets/uses/answer`. See
[HTTP routes](/guides/routes/).

## Delivery v2

A body sent with `Content-Type: application/vnd.hotline.secrets.v2+json` is
`{"v":2,"generation":…,"secrets":{…}}`. `generation` is a whole number, and an
older one than the last taken is refused. In it a variable says one of two
things:

- `"readable": true`: a variable as above.
- `"use": {…}`: a protected key. Only the split computer takes one. The
  single-user computer refuses the whole body, and so does a split computer
  whose key proxy did not start; `GET /capabilities` names both `legacy`.

Logins and passkeys are as in a v1 body. A variable that says neither, or both,
is refused. A v1 body is read as before, a bare string as a readable variable;
it cannot carry a `use`, and it unbinds every protected key.

| `use` field | |
| --- | --- |
| `origins` | `https://host[:port]`, the only places the key is sent |
| `header` | The one request header it goes in. Default `Authorization` |
| `schemes` | The schemes that header may carry it under, such as `Bearer`, `token` or `Basic`. None means the header's whole value |
| `standIn` | `{"prefix":…,"length":…}`, the stand-in's shape, for a CLI that checks a token's prefix |
| `refuse` | Key-minting routes on those origins the key never goes to. Absent means the built-in list for GitHub, OpenAI and Anthropic; empty means none |

`state info` lists a protected key as a variable with `protected: true` and
its `origins`. Each job gets a stand-in of its own under the key's name, and
`HTTPS_PROXY`, `NO_PROXY` for localhost, `NODE_USE_ENV_PROXY=1`, and
`SSL_CERT_FILE`, `NIX_SSL_CERT_FILE`, `CURL_CA_BUNDLE`, `GIT_SSL_CAINFO`,
`REQUESTS_CA_BUNDLE` and `NODE_EXTRA_CA_CERTS` pointing at
`/run/hotline-computer/public/ca-bundle.pem`: the system roots and a CA the
proxy makes at each boot. A job with no protected key gets none of these.
A protected key cannot take one of those names.

The key proxy on `127.0.0.1:3128` answers `CONNECT` only and passes every
other host through untouched. For a bound origin it ends TLS itself, checks
the real server against the web roots built into the binary, and replaces a
live stand-in with the key only in the binding's header and schemes, never
in a body, path, query or another header. It follows no redirect, writes the
stand-in back over the key in every reply, and answers 403 to a keyed request
for a key-minting route. A stand-in dies when its job ends, and with the
delivery that unbinds or changes its key, so revoking a key stops a running
job too.

Every job runs as the same user, `agent`. On the split computer each job
starts in a Landlock domain of its own, so a job cannot trace another job,
read or write its memory or environment, or take a connection it holds,
and so cannot use another job's proxy credential, or go on using one after
its own job ended. Without Landlock in the kernel the broker logs a line
at start and jobs are not kept apart: one job can act as another. Either
way the jobs share that user's files, so a job can change a script, a
shell startup file or a dependency that a later job runs.

## Redaction

There is no `GET`. `state info` lists each secret's name and kind, and for a
login its sites and username. Every text a tool returns has each value
replaced with `[redacted NAME]`, in plain and in JSON spelling. A login's
current TOTP digits read `[redacted NAME.code]`. Usernames and
sites stay readable, so a page that says who is signed in still makes sense.

Redaction removes secret values from tool text responses when a job prints them
or a page echoes them. It does not stop code from exposing values by splitting,
encoding or writing them to a file. Non-text results and the screen are not
redacted. Jobs never inherit `HOTLINE_COMPUTER_TOKEN`.

## The signed-in browser

On the split computer, a login's sites, a granted passkey's site, the sites
you shared cookies for with `PUT /sessions/{name}`, and a site armed for a new
passkey open in a second,
headless Chromium that runs as `signin`, with DevTools on a pipe and no port.
The agent drives it with site actions only: `eval` is refused. From the first
secret typed into a tab until the tab shows another page, nothing of it
reaches the agent. A sign-in still on that page 30 seconds on is listed by
`GET /signin/pending`, and the agent is told to ask you to finish it. Open
Browser in the desk shows this browser through `/sites/ws`. A password or code
goes only into a password (or one-time-code) field on the login's own origin,
either in no form (a page whose script sends it) or in a form that sends to
that origin. A form that would carry what was typed to another site is
stopped, and so is any page load whose address would carry it, on any site,
and, while anything is typed mid sign-in, a form a frame would post to a site
that is not signed in.
Revoking a login, a passkey
or a share clears its site's cookies and data and closes its tabs.

A passkey comes with its `rpId` alone, so its site is that host and those
under it, over https on any port (and http on localhost), as WebAuthn scopes
it. Where a login or a share names a site on the same host, the passkey is
used on the sites they name.

On the single-user computer none of this applies. A shared sign-in is a saved
login in `~/.hotline/logins`, loaded into the visible browser, and
`PUT /sessions/{name}` answers 404.

## Keyring

Anything that uses libsecret, from a native app to `secret-tool`, finds a
Secret Service on the session bus from boot: gnome-keyring with its login
collection unlocked, so storing a password never prompts on the desktop.
Its password is empty, so its file under `~/.local/share/keyrings` is not
encrypted. The home volume is the boundary around what is kept there.
Environments prepared with Nix use the same bus and the same keyring.
