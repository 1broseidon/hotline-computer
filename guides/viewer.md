---
title: Viewer
description: The page that shows the computer's screen live, how taking control works, and the socket and file routes behind it.
cluster: Using it
reviewed: 2026-11-02
---

**The computer serves its own screen: open `http://127.0.0.1:8787/#<token>` in
any browser to watch it, and press Take control to drive it.** The viewer
opens watch-only, so watching never interrupts the agent.

## Control

The page opens view-only, with **Take control** at the foot of the screen.
Until that is pressed the socket shows the desktop and moves nothing. From
then on your input holds the machine for ten seconds at a time: the agent's
tools that change the screen are refused while you drive, and the desktop
hands itself back when you stop, or at once when you press **Give it back**
or close the page. If another person holds the screen, the button waits.

The control bar at the foot reads the machine's state while you watch: the
agent's name, job counts, failures and the measured ping. Ten seconds without
a pong shows **stalled**. While you drive it shows amber **You're driving**,
**Paste** and **Give it back**. A reconnect always starts view-only.

## Keys

Ctrl+V, Ctrl+C and the rest reach the machine as keys, so its apps use their
own clipboard. Text from your own computer comes in with Ctrl+Alt+V (⌥⌘V on a
Mac) or the **Paste** button, as plain text, up to 1 MiB, and only while your
viewer holds control; a stale viewer cannot paste over a newer one.
Ctrl+Alt+Shift gives control back. On macOS, Settings turns **Cmd as Ctrl** on
by default. **1:1** shows native pixels; Settings also offers Fullscreen.

## Files

**Files** opens a panel over the screen with the home and what is under it.
A folder opens, the arrow goes up as far as the home, and a file downloads to
your own computer. **Add files**, or a drop anywhere on the page, uploads to
the folder the panel is showing, whole or not at all. An existing name is
replaced only after you confirm **Replace {name}?**.

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

`/` is a page with one canvas, and `/ws` is the socket behind it. Frames go
down as binary messages: six little-endian `u16` (x, y, width, height, screen
width, screen height) followed by a PNG of that rectangle. X DAMAGE decides
which rectangles, at most twenty a second, and an idle desktop sends nothing.
A viewer that joins or falls behind gets the whole screen next.

Input comes up as JSON and reaches the X server through XTEST:

```json
{"t":"control","take":true}
{"t":"move","x":640,"y":400}
{"t":"button","b":1,"down":true}
{"t":"wheel","dy":1}
{"t":"key","key":"Enter","down":true}
```

The socket takes the bearer as a `token` query, because a browser cannot send
a header on a WebSocket. The page reads it from the URL fragment, which never
leaves the browser.
