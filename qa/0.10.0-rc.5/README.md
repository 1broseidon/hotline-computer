# 0.10.0-rc.5 local verification (2026-09-23)

Image `hotline-computer:0.10.0-rc.5`, on the rc.4 volumes. rc.5 removes the friction
found while building Hotline desktop features on rc.4: every change below came from a
real stumble in that session. Platform content (`assets/base.json`) is unchanged.

## Typing no longer loses text in webviews

A batch `type` followed by a click lost most of the text in Hotline (a Tauri app). WebKitGTK
queues key events; when the page is slow, as a React development build re-rendering per
keystroke is, the click lands first and moves focus away from the rest.

`tests/fixtures/webview.c` reproduces it: a WebKitGTK window whose text area costs 15 ms
per keystroke. Typing 114 characters and then clicking the next field:

| | Characters that landed |
|---|---|
| rc.4 | 29 of 114 |
| rc.5 | 114 of 114, three runs out of three |

`type` and `paste` now wait until the focused field's character count holds steady
(AT-SPI `Text`, at most 10 s). This adds about 200 ms per `type` in Chromium.

## Scrolling says which way

`scroll` took signed `clicks`, where a positive number scrolled up, so "scroll 5" moved the
page the wrong way. rc.5 adds `direction` (up, down, left or right; 3 notches by default).
Signed `clicks` without a direction still behaves the old way.

## Capture sized for the question

- `window` (an id, or a unique part of the title) captures one window: its picture and its
  tree only. `region` takes `[x, y, w, h]`. `image` mode returns the picture without a tree.
- Every picture comes with the mapping from image pixels to screen points, including the
  scale factor when the screenshot was shrunk to its 1568 px long edge.
- The tree skips hidden nodes and unnamed wrappers (panels, sections, fillers), but still
  walks their children, because WebKit's scroll pane reports no `showing` state while its
  content is on screen. Applications that own none of the requested windows are skipped.
  The tree lists at most 400 elements and says so when it stops.

| Capture | rc.4 | rc.5 |
|---|---|---|
| Whole desktop with Chromium open | over 25k tokens | 3.3 KB |
| The webview window | — | 4 lines |

A desktop `run`'s ready screenshot is now of the app's window rather than the whole screen;
the JSON result has its mapping in `ready.screenshot`.

## Calls answer before the client gives up

MCP clients abandon a call after about 60 s. A `shell run` with `wait_ms` 60000 did exactly
that while Hotline's dev build started. Every wait (`run`, `ready`, `wait`, the `wait` tool,
batch `wait` steps) is now capped at 50 s, and `exec` at 45 s. Longer work goes through
`ready` or `wait` again.

## Shell takes a command line

`shell exec {"command": "echo $HOME | tr a-z A-Z"}` failed with "No such file or directory".
A `command` containing spaces or shell syntax, with no `args`, now runs under `bash -c`.
A plain program name still runs directly.

## Links from apps open in the managed browser

(Committed in 79bf57b.) `xdg-open`, `$BROWSER` and the desktop's http and https handlers run
`hotline-computer open`, which gives the address to the running agent, and the agent opens
it as a tab of the managed browser, where the person's sign-ins are. Hotline's "Open xAI
sign-in" now lands in that browser.

`navigate` and new tabs no longer fail when a page answers with an HTTP error status. The
page is shown, with a note saying so.

## The bar's "failed" means something to look at

The bar counted every retained job that had not exited 0 as failed, across restarts: jobs
someone cancelled, jobs a restart cut short, and failures long since fixed. The person's
machine showed "14 failed" with nothing wrong. It now counts a failure only while it is
under an hour old and nothing has run the same command since; cancelled and interrupted jobs
never count. The jobs list still shows every job as it ended. The viewer's floating control
bar counted the old way on its own; it now takes the same counts and the same words as the
desktop's bar.

## Apps find a login shell

The image set no `SHELL`, so apps that read the person's login-shell `PATH` at startup fell
back to `/bin/zsh`, which is not installed, and logged "could not read shell PATH: No such
file or directory". The image now sets `SHELL=/bin/bash`, as a desktop session would.

## Checks

- `make check`: fmt, clippy and all unit tests pass (new: region clipping, wheel direction,
  command-line detection).
- `make contract` on a fresh rc.5 container passes. It covers new assertions for window-scoped
  capture, region images and shell command lines.
- Acceptance, quick suite on rc.5: 15 of 15 cases pass.
- Acceptance: the new `WebKitGTK typing and scrolling` case (native and full suites) passed in
  33 s. It types into the slow text area, clicks the next field, types there, checks both
  values in the tree, then scrolls down until "Bottom button" is showing. The Qt wheel case
  now waits until `PySide6.QtWidgets` imports rather than until its directory appears; the old
  check raced the install once.

## Not the computer's

Signing in to GitHub Copilot from Hotline fails with "incorrect device code". The device flow
reaches GitHub and the code is entered in the managed browser; the rejection is on Hotline's
side.
