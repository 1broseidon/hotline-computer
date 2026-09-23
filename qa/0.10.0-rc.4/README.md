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

## Independent review (Ada) and fixes

Ada ran the image on fresh volumes and drove it over MCP. She confirmed the platform split
and the accessibility change on Qt, Electron, GTK 3, WebKitGTK and Chromium, with zero GetAll
calls over a full bus trace. She found three problems, all now fixed and retested by her on
the rebuilt image:

- The drafter read no toolkit declared in `setup.py` or `setup.cfg`. Both are now read as
  text (never run).
- The first character typed through a freshly remapped keycode could be lost: the press
  arrived before the client reloaded its keymap. Typing now maps every new character first,
  and the first press after a remap waits 100 ms. This is a tested mitigation, not an
  acknowledgement from the client; five fresh containers typed `ΩЖ漢` intact on first use.
- A wheel needing a library outside the manylinux set (pyodbc: `libodbc.so.2`) had no remedy
  short of a flake. Manifests take `libraries`, Nixpkgs attributes put on the library path;
  `"libraries": ["unixODBC"]` alone makes pyodbc's test pass.

Also from her review: `browser eval` of `null` returned an error instead of null.

Regressions: the contract test captures a fresh Chromium page before anything else reads it
and expects a button in the tree, types `ΩЖ漢` on a fresh keymap, and evaluates `null`. The
native acceptance suite gains a PySide6 6.8.1 case declared only in setup.py: drafted, prepared,
captured three times without a crash, its tree read, and a click counted. The contract test
passed on its first run against a fresh container, and the Qt case passed.

## Links open in the managed browser

Signing Hotline in to xAI from its dev build did nothing: "Open xAI sign-in" went through
`xdg-open` to Debian's Chromium entry, which started a second browser without `--no-sandbox`
that died, and would not have held the person's cookies anyway. Web addresses now go to
`hotline-computer open`, which hands them over a local socket to the running agent; it opens
each as a tab of the managed browser, starting it if needed, and raises its window. `BROWSER`
is set too, for programs whose Nix environment hides `/usr/share/applications`: without it,
`xdg-open` from Hotline's dev shell finds no handler at all. Checked on a fresh container
through both routes; the xAI device sign-in completed with the person's session.
