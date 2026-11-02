---
title: Contract
description: What makes something a valid computer, what is inside the image, and how it starts.
cluster: Reference
reviewed: 2026-11-02
---

**The image is a contract, not a binary: anything that serves the same eight
tools over streamable HTTP at `/mcp`, with `/health` open, is a valid
computer.** One container is one machine.

## Image

```
hotline-computer  PID 1, supervisor, window manager, bar, MCP server
├── Xvfb       the X server; pixels in RAM, no GPU
├── dbus       the session bus the accessibility tree rides on
├── keyring    gnome-keyring, the Secret Service, unlocked from boot
└── chromium   the visible browser, driven over its DevTools protocol
```

The image is Debian glibc with a non-root agent account, and runs with every
capability dropped. Native apps and GPU-backed terminals use Mesa's llvmpipe
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

`hotline-computer boot` is the entrypoint. As PID 1 it forks: the parent
reaps every child the kernel hands it and forwards SIGTERM; the child starts
Xvfb, dbus-daemon and gnome-keyring, becomes the window manager, and serves.
If the display or the bus dies, the machine exits, and the container with it.
A keyring that dies is started again.

## Hotline

Hotline starts one computer per teammate, gives it the token and the
person's secrets, and shows its screen in the desk. Everything in the
[manual](/) works without Hotline: any MCP client can drive a computer, and
any browser can watch it.
