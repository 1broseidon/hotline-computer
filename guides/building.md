---
title: Building
description: Building the image from source, the checks a change has to pass, and where each part of the code lives.
cluster: Development
reviewed: 2026-11-02
---

**`make image` builds the image from this repository, and `make run` starts it
the way Hotline does.** A release has to pass the same acceptance run on
ARM64 and x86_64 before it is tagged.

## Make

```sh
make image                # docker build -t hotline-computer:next .
make run                  # a hardened container on 127.0.0.1:8787, token in .token
make contract             # the contract test against it, through a real MCP client
make check                # fmt, clippy -D warnings, unit tests
make acceptance           # fresh image, real MCP and viewer tests, repository builds
```

`make run`, spelled out:

```sh
docker run -d --name hotline-computer-next \
  --cap-drop=ALL --security-opt no-new-privileges \
  --pids-limit 1024 --memory 4g --shm-size 1g \
  -p 127.0.0.1:8787:8787 \
  -e HOTLINE_COMPUTER_TOKEN="$(cat .token)" \
  -e TZ="$(cat /etc/timezone)" \
  hotline-computer:next
```

## Releases

The desktop acceptance workflow runs the fresh-image gate on native ARM64 and
x86_64 runners. Its evidence includes the image ID, a tool-call trace, job
output, screenshots and cold and warm timings. Checks on native screens need
both the expected accessibility controls and a visible pixel change.
Acceptance does not publish images. The publishing workflow repeats it,
checks that the pushed image has the same configuration digest as the tested
one, and only then publishes the combined manifest.

## Layout

```
src/boot.rs        PID 1, Xvfb, dbus, then the agent
src/desktop.rs     wallpaper, top bar, tray, window manager
src/serve.rs       the HTTP door: /health, bearer auth, /mcp, the viewer routes
src/viewer.rs      the viewer page and its socket
src/viewer.html    the page: one canvas, Take control, pointer and keys
src/screen.rs      DAMAGE-driven PNG rectangles for the viewer
src/xtest.rs       the person's pointer and keys, injected with XTEST
src/tools/         the eight tools
src/browser.rs     the managed Chromium over CDP
src/x11.rs         screenshots and EWMH window queries
src/a11y.rs        the AT-SPI tree as text
src/lease.rs       who holds the machine
src/jobs.rs        job lifetime, retained output, PTY, cancellation
src/secrets.rs     stored secrets by kind: the door, job environment, fill, redaction
src/passkeys.rs    passkey arming and its door
src/observer.rs    the job observer
src/workspace.rs   workspace-owned Nix definitions and preparation
src/guide.rs       the bundled release-matched skill
tests/contract.rs  the opt-in proof against a running container
```
