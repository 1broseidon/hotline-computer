---
title: Secrets
description: Secret types, access rules and redaction limits.
cluster: Security
section: security
reviewed: 2026-11-02
---

**Secrets are supplied by name. Variables are added to job environments, logins
are filled on their configured sites, and passkeys are used by the signing
service.** Tools redact secret values in text responses. Code running in a job
can still access variables in its environment.

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
| Variable | `{"kind":"variable","value":…}` | An environment variable in every job started through `shell` or `files run`, not in preparation jobs. 8 characters to 64 KiB |
| Login | `{"kind":"login","sites":[…],"username":…,"password":…,"totp":…}` | Never a job. `browser fill` types `NAME.username`, `NAME.password` or `NAME.code` (the current TOTP digits) only onto a page whose origin is one of the login's sites |
| Passkey | `{"kind":"passkey","rpId":…,"credentialId":…,"privateKey":…,"userHandle":…,"userName":…}` | The service, which signs the site's request; the private key never enters the browser. See [Passkeys](/guides/passkeys/) |

A login's site is an `https://` origin, or `http://` on localhost. The page a
password is typed into can read it, so which page it is typed into is the
whole protection.

## Redaction

There is no `GET`. `state info` lists each secret's name and kind, and for a
login its sites and username. Every text a tool returns has each value
replaced with `[redacted NAME]`, in plain and in JSON spelling. Usernames and
sites stay readable, so a page that says who is signed in still makes sense.

Redaction removes secret values from tool text responses when a job prints them
or a page echoes them. It does not stop code from exposing values by splitting,
encoding or writing them to a file. Non-text results and the screen are not
redacted. Jobs never inherit `HOTLINE_COMPUTER_TOKEN`.

## Keyring

Anything that uses libsecret, from a native app to `secret-tool`, finds a
Secret Service on the session bus from boot: gnome-keyring with its login
collection unlocked, so storing a password never prompts on the desktop.
Its password is empty, so its file under `~/.local/share/keyrings` is not
encrypted. The home volume is the boundary around what is kept there.
Environments prepared with Nix use the same bus and the same keyring.
