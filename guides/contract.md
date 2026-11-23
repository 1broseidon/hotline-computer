---
title: Contract
description: What a valid computer is, and what's inside.
cluster: Reference
section: overview
reviewed: 2026-11-18
---

**A compatible computer exposes the same eight tools over streamable HTTP at
`/mcp` and an unauthenticated `/health` endpoint.** Each container runs one
computer.

## Image

```
hotline-computer  PID 1, supervisor, window manager, bar, MCP server
├── Xvfb       the X server; pixels in RAM, no GPU
├── dbus       the session bus the accessibility tree rides on
├── keyring    gnome-keyring, the Secret Service, unlocked from boot
└── chromium   the visible browser, driven over its DevTools protocol
```

That is the single-user computer. The image is Debian glibc with a non-root
agent account, and by default runs as it with every capability dropped.

```
hotline-computer  PID 1 as root, SETUID and SETGID only: holds :8787 and 127.0.0.1:3128
├── broker     hotline (1001): the MCP endpoint, the token, logins, passkeys, protected keys, the key proxy
├── signin     signin (1002): the signed-in browser, headless, DevTools on a pipe
└── runner     agent (1000): Xvfb, dbus, keyring, the desktop, jobs, files, chromium on 9222
```

That is the split computer, which starts when the image is created with
`--user 0 --cap-add SETUID --cap-add SETGID` beside `--cap-drop=ALL`. Native apps and GPU-backed terminals use Mesa's llvmpipe
when there is no GPU. The agent's input goes through XTEST, and it owns the
CLIPBOARD selection directly.

| Component | Purpose |
| --- | --- |
| Xvfb and Mesa | X11 desktop and software OpenGL and EGL |
| D-Bus and AT-SPI | Accessibility for native apps |
| gnome-keyring | The Secret Service apps and CLIs keep passwords in |
| Chromium | The visible browser, over CDP |
| Alacritty | The job observer |
| Nix | Project packages and repository dev shells |
| GTK3, WebKitGTK, AppIndicator | Native Linux app runtime and tray |
| Git, curl, Python, ripgrep, jq, archive tools | Repository and artifact work |

## Boot

`hotline-computer boot` is the entrypoint. Started as any user but root, it is
the single-user computer. As PID 1 it forks: the parent reaps every child the
kernel hands it and forwards SIGTERM; the child starts Xvfb, dbus-daemon and
gnome-keyring, becomes the window manager, and serves. If the display or the
bus dies, the machine exits, and the container with it. A keyring that dies is
started again.

Started as root, it is the split computer. PID 1 drops every capability but
`SETUID` and `SETGID`, and refuses to go on without those two. It binds the
door and the proxy port and holds them for the container's life, then starts
the broker as `hotline` and the runner as `agent`, each with no capability and
`no_new_privs` set. The runner does what the single-user computer's child
does, without the door; the broker serves the door and hands the desktop's own
pages to the runner. PID 1 starts the signed-in browser as `signin` when the
broker asks, and stops the container when the broker or the runner exits.

`GET /capabilities` says which one is running.

## Hotline

Hotline starts one computer per teammate, gives it the token and the
person's secrets, and shows its screen in the desk. Everything in the
[manual](/) works without Hotline: any MCP client can drive a computer, and
any browser can watch it.
