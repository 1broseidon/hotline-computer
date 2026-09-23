# 0.10.0-rc.3 local verification (2026-09-23)

Image `hotline-computer:0.10.0-rc.3`, channel `candidate`, on the rc.2 volumes
(`--memory 4g`, all capabilities dropped), driven over MCP as an agent.

rc.2 had opened Tauri, Qt 6 C++ and Rust minifb windows. This run covers the GUI
stacks it had not: Go (Fyne), Node (Electron) and Python (PySide6 and Tk).

## On rc.2, what an agent hit

| App | Draft | Launch |
|---|---|---|
| Fyne, no go.sum yet | go and build runs only: Fyne went unnoticed | cgo build of GLFW: `X11/extensions/Xinerama.h: No such file` |
| Electron 33 | nodejs, npm install, `start` as a desktop run | aborted: the SUID `chrome-sandbox` must be root-owned 4755 |
| PySide6 wheel | venv and install only, no GUI | `ImportError: libstdc++.so.6` (then libX11, libzstd, ...) |
| Tk | venv only | worked once `python312Packages.tkinter` was added by hand |

## Fixed in rc.3

- Platform `prebuilt`: libstdc++, zlib, zstd, expat, krb5, libdrm, GLib, D-Bus, fontconfig,
  freetype, XKB, X11 and XCB libraries on the library path, over `gl`. Drafted for every
  Python project, since PyPI wheels expect them.
- Platform `native` adds libXinerama and libXxf86vm, which GLFW (Fyne) compiles against.
- The image sets `ELECTRON_DISABLE_SANDBOX=1`.
- Drafts: Go GUI modules read from go.mod as well as go.sum, with a `go mod tidy` create
  hook when go.sum is missing; PySide6, PyQt, pygame, Kivy, wxPython and Dear PyGui, and
  `import tkinter` (adding `python312Packages.tkinter`), give an `app` run for main.py or app.py.

## On rc.3, drafts used unedited

| App | Result |
|---|---|
| Fyne | tidy hook, `app` ready with the window; button click counted |
| Electron | `start` ready with the window; click counted; page reads through the accessibility tree |
| PySide6 | `app` ready with the window; click counted |
| Tk | `app` ready with the window; click counted |

## Known gaps

- Qt (PySide included), Fyne and Tk expose no accessibility tree; screenshots and input work.
- A Fyne cold build takes about two minutes; `app` answers "still building" until then.
