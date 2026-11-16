---
title: Viewer
description: Watch it live, take control, and move files.
cluster: Using it
section: viewer
reviewed: 2026-11-02
---

**The computer serves its own screen: open `http://127.0.0.1:8787/#<token>` in
any browser to watch it, and press Take control to drive it.** The viewer
opens watch-only, so watching never interrupts the agent.

## Control

The viewer opens in view-only mode, with **Take control** at the bottom of the
screen. Mouse and keyboard input are disabled until you press it. Each input
then renews your control for ten seconds. While you have control, the agent's
tools that change the screen are rejected. Control returns to the agent after
ten seconds without input, or immediately when you press **Give it back** or
close the viewer. If another person has control, you must wait for them to
release it.

The control bar at the foot reads the machine's state while you watch: the
agent's name, job counts, failures and the measured ping. Ten seconds without
a pong shows **stalled**. While you drive it shows amber **You're driving**,
**Paste** and **Give it back**. A reconnect always starts view-only.

## Keys

Copy and paste shortcuts use the container's clipboard. To paste text from your
own computer, use the shortcut listed below.

| Keys | Does |
| --- | --- |
| <kbd>Ctrl</kbd> <kbd>V</kbd>, <kbd>Ctrl</kbd> <kbd>C</kbd> | Paste and copy inside the computer, with its own clipboard |
| <kbd>Ctrl</kbd> <kbd>Alt</kbd> <kbd>V</kbd> | Paste text from your own computer, up to 1 MiB (<kbd>⌥</kbd> <kbd>⌘</kbd> <kbd>V</kbd> on a Mac) |
| <kbd>Ctrl</kbd> <kbd>Alt</kbd> <kbd>Shift</kbd> | Give control back |

Pasting from your computer works only while your viewer holds control, and a
stale viewer cannot paste over a newer one. On macOS, Settings turns **Cmd as
Ctrl** on by default. **1:1** shows native pixels; Settings also offers
Fullscreen.

## Files

**Files** opens the container's home directory. Click a folder to open it, the
up arrow to open its parent, or a file to download it. Navigation is limited to
the home directory and its contents. Use **Add files** or drop files onto the
page to upload them to the displayed folder. Each upload is atomic: the complete
file is saved, or the upload leaves no partial file. Replacing an existing file
requires confirmation through **Replace {name}?**.

Behind the panel:

| Route | Does |
| --- | --- |
| `GET /files?path=` | Lists a folder as JSON: `path`, `home`, and `entries` with `name`, `size`, `is_dir`, `modified` |
| `GET /files/download?path=` | Streams a file as an attachment |
| `POST /files?path=` | Writes the body as a file, atomically, under the home only |

All three take the token as a `token` query, treat an empty path as the home,
and stop at the home like the `files` tool. With `create_only=true` an upload
publishes only if nothing is there, and a collision is a 409 that leaves the
existing file alone. An authenticated `GET /files` advertises
`x-hotline-upload-create-only: 1`, so a client can refuse unsafe uploads to an
older computer.

## Protocol

`/` is a page with one canvas, and `/ws` is the socket behind it. The server
sends frames to the viewer as binary messages: six little-endian `u16` values
(x, y, width, height, screen width, screen height), followed by a PNG of that
rectangle. X DAMAGE identifies changed rectangles, with at most twenty sent per
second. An idle desktop sends no frames. A viewer that connects or falls behind
receives a full-screen update.

The viewer sends input as JSON. The server passes it to the X server through
XTEST:

```json
{"t":"control","take":true}
{"t":"move","x":640,"y":400}
{"t":"button","b":1,"down":true}
{"t":"wheel","dy":1}
{"t":"key","key":"Enter","down":true}
```

The WebSocket connection supplies the bearer token in the `token` query
parameter because the browser's WebSocket API does not support a custom
`Authorization` header. The viewer reads the token from the URL fragment, which
is omitted from the initial page request. It then sends the token in the query
parameters of its WebSocket and file requests.
