---
title: Secrets
description: Where each secret goes, and what redaction stops.
cluster: Security
section: security
reviewed: 2026-11-02
---

**You give the computer secrets by name, and each kind goes to exactly one
place: a variable into the agent's jobs, a login into its own sites, a
passkey into the service that signs for it.** The agent never reads a value
back.

## Kinds

The set comes from the client that started the computer (Hotline's desk, or
your own) with `PUT /secrets`, the bearer in the `Authorization` header and a
JSON object of name to entry as the body. It replaces the whole set and stays
in the service's memory; the computer writes no value to disk. A name is what
a shell variable can be called (`GITHUB_TOKEN`), never one starting with
`HOTLINE_` and never one the shell owns, such as `PATH` or `HOME`. One bad
entry refuses the whole set with a 400 that names it, and the last set stays.

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

That keeps a value out of the agent's context when a job prints it or a page
echoes it. It does not stop a command written to get a value out, in pieces,
encoded or into a file, and the screen you watch is never redacted. A job
never inherits `HOTLINE_COMPUTER_TOKEN`.

## Keyring

Anything that uses libsecret, from a native app to `secret-tool`, finds a
Secret Service on the session bus from boot: gnome-keyring with its login
collection unlocked, so storing a password never prompts on the desktop.
Its password is empty, so its file under `~/.local/share/keyrings` is not
encrypted. The home volume is the boundary around what is kept there.
Environments prepared with Nix use the same bus and the same keyring.
