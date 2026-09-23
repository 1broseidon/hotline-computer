# 0.10.0-rc.2 local verification (2026-09-23)

Image `hotline-computer:0.10.0-rc.2`, channel `candidate`, branch
`george/manifest-0.10`. Not published. Run like the desk: home, src and /nix on
volumes, `--memory 4g`, all capabilities dropped; driven over MCP as an agent.

## What rc.2 adds for agent experience

- `state manifest` drafts a manifest from the repo's own files, with a reason per line.
- Desktop runs answer when the window is up and settled, with the window and a screenshot;
  web runs answer when localhost listens, with the URL; `shell ready` waits again.
- Platforms `qt`, `gtk4` and `native` (X11/XKB/Vulkan for winit, SDL, GLFW, Fyne, Gio).
- First prepare locks Nixpkgs from the base and takes its source from the image cache.

## Upgrade from rc.1 volumes

| Check | Result |
|---|---|
| rc.1 workspace after image swap | keeps base 0.10.0, reports `update_available` |
| its services and start hook | resumed by themselves; `psql`/`redis-cli` check passed |
| `prepare` with `upgrade: true` | base 0.10.0-rc.2, cache hit, no rebuild |

## Fresh computer, five repositories

| Repo | Draft | Result |
|---|---|---|
| Hotline (Tauri) | webkit, cargo-tauri, dbus, bun hook for `ui/`, app run in `crates/hotline-app` | agent dropped the docs-site hook; prepare 26 s with **no GitHub fetch**; `app` answered "still building" with a clean log tail, then the window with a settled Welcome screen |
| Qt 6 widgets (CMake) | platform qt, configure hook, build, app `./build/viewer` | built by name, window + screenshot, button clicks worked |
| Rust minifb | platform native, app `cargo run` | compiled and opened; X11 libraries loaded at run time |
| Vite | npm install hook, `dev` web run with `--port $PORT` | ready with URL, page and module loaded in Chromium |
| Python + pytest | uv venv hooks, `.venv/bin` on PATH, test run | 2 passed |

Every drafted package name was checked against the pinned Nixpkgs.
`cargo test --test contract` passes against the RC.

## Found and fixed during this run

- Vite binds localhost over IPv6; web readiness only probed 127.0.0.1.
- The first desktop screenshot caught the webview before it loaded; readiness now waits for the screen to settle.
- Output tails carried terminal colour codes.

## Known gaps

- Qt apps expose no accessibility tree even with `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1`; screenshots and input work.
- `flake` still cannot be combined with `platform` or `services`.
- Python GUI toolkits (PySide, Tk) are not drafted.
- A changed base must carry a new version for workspaces to see an update.
