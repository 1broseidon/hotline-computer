# 0.10.0-rc.4 local verification (2026-09-23)

Image `hotline-computer:0.10.0-rc.4`, channel `candidate`, on the rc.2 volumes
(`--memory 4g`, all capabilities dropped), driven over MCP as an agent.

## Platforms follow published lists

rc.3's `prebuilt` was a list grown one `ImportError` at a time. rc.4 splits it:

- `prebuilt` is the manylinux system library policy (PEP 600, as auditwheel enforces it):
  libstdc++/libgcc_s, zlib, expat, GLib, libnsl, X11, Xext, Xrender, ICE, SM, and OpenGL
  through `gl`. A wheel that needs more is outside the standard.
- `qt-wheel` adds what Qt's wheels load beyond it: the xcb plugin's libraries from Qt's
  Linux requirements page, plus D-Bus, zstd, Brotli, libdrm and Kerberos, which the
  wheels link. Drafted for PySide and PyQt projects.

Checked by listing every native module in each venv under Nix's loader with only these
libraries on the path: nothing outside Wayland and optional SQL drivers is missing.

## Drafts used unedited

| App | Draft platforms | Result |
|---|---|---|
| numpy + pandas | prebuilt | `test` run: 1 passed |
| pygame | prebuilt | `app` ready with the window; three clicks counted |
| PySide6 6.8.1 | prebuilt, qt-wheel | `app` ready; clicks counted; tree read |
| PySide6 6.11.2 | prebuilt, qt-wheel | `app` ready; click counted; tree read (6.11 also needs xcb-util and Brotli, now in qt-wheel) |
| Electron | — | `start` ready; tree read down to the page's heading and button |

## Capture crashed Qt 6.8 apps; Qt now reads

`capture` killed a running PySide6 6.8 app (SIGSEGV in `QVariant::toString` inside Qt's
AT-SPI adaptor), on rc.3 as well. zbus filled its property cache with
`Properties.GetAll`, which Qt's bridge does not implement: 6.11 refuses it, 6.8 crashes.
rc.4 reads properties one at a time. Qt apps now expose their accessibility tree to
`capture`, where rc.3 showed none.

Chromium only builds its tree once a client reads a window's attributes, which that
`GetAll` had been doing by accident. rc.4 calls `GetAttributes` on each window first,
so Electron reads as it did on rc.3.

The contract test passes against the running image.

## Known gaps

- Tk and Fyne still expose no accessibility tree; screenshots and input work.
- A workspace whose manifest already names `prebuilt` for a Qt wheel needs `qt-wheel` added.
