# toad.computer

A small Linux desktop for a coding agent. One container is one machine: a
display, a browser, a shell, a home directory, and one MCP server that lets
an agent see the screen, act on it, and check what happened. Toad starts one
per teammate; anything else that speaks MCP can point at it too.

The image is a contract, not a binary. Anything that serves these eight tools
over streamable HTTP at `/mcp`, with `/health` open, is a valid computer.

## What is in the box

```
toad-computer  PID 1, supervisor, window manager, dock, MCP server
├── Xvfb       the X server; pixels in RAM, no GPU
├── dbus       the session bus the accessibility tree rides on
└── chromium   the visible browser, driven over its DevTools protocol
```

The image uses Debian glibc, Xvfb, Mesa software rendering, Nix, and a
non-root agent account. It runs with every capability dropped. Native apps
and GPU-backed terminal emulators use llvmpipe when no hardware GPU exists.

| Component | Purpose |
| --- | --- |
| Xvfb and Mesa | X11 desktop and software OpenGL/EGL |
| D-Bus and AT-SPI | Native application accessibility |
| Chromium | Visible browser controlled through CDP |
| Alacritty | Reopenable observer for tool-driven shell jobs |
| Nix | Pinned Python, Go, Node, Rust, and Rust/Tauri environments |
| GTK3, WebKitGTK, AppIndicator | Native Linux app runtime and tray support |
| Git, curl, archive tools, Python, ripgrep, jq | Repository and artifact workflows |

The agent sends input through XTEST and owns the CLIPBOARD selection directly.

## The viewer

The machine serves its own screen. `/` is a page with one canvas; `/ws` is
the socket behind it. Frames go down as binary messages, six little-endian
`u16` (x, y, width, height, screen width, screen height) followed by a PNG of
that rectangle; X DAMAGE decides which rectangles, at most twenty a second,
and an idle desktop sends nothing. A viewer that joins or falls behind gets
the whole screen next. Input comes up as JSON and reaches the X server through
XTEST:

```json
{"t":"control","take":true}
{"t":"move","x":640,"y":400}
{"t":"button","b":1,"down":true}
{"t":"wheel","dy":1}
{"t":"key","key":"Enter","down":true}
```

The socket takes the bearer as a `token` query, because a browser cannot send
a header on a WebSocket; Toad opens `http://127.0.0.1:<port>/#<token>` and the
page reads the fragment, which never leaves the browser. The page opens
view-only, with `Take control` at the foot of the screen: until that is
pressed the socket shows the desktop and moves nothing, so watching a teammate
work never interrupts it. From then on a person's input holds the machine as
a unique viewer holder for ten seconds at a time, so the teammate's mutating tools are
refused while someone is driving and the desktop hands itself back when they
stop — at once when they give it back or close the page.

## The tools

- `capture` returns a scaled PNG and the AT-SPI tree, or writes an original PNG.
- `input` clicks, moves, drags, scrolls, types, presses keys, and uses the clipboard.
- `browser` drives the visible Chromium over CDP; element refs last for one text snapshot. No action runs longer than a minute, and a browser the person closed is replaced by the next call rather than waited on.
- `shell` starts managed jobs, reads retained output, writes stdin, waits, cancels, and opens the terminal observer.
- `files` gets, puts, and lists paths below the computer home; downloads, verifies, extracts, and runs artifacts through managed jobs.
- `windows` lists, focuses, closes, maximizes, and tiles windows.
- `wait` polls the accessibility tree and browser page text for a phrase.
- `state` identifies the running version, returns its guide/catalog, prepares workspaces, and manages leases, browser logins, and snapshots.

`/health` and the viewer page never require authentication. When
`TOAD_COMPUTER_TOKEN` is set, every method on `/mcp` requires
`Authorization: Bearer <token>`, the viewer's socket requires the same token
as its `token` query, and otherwise both return a JSON 401. `X-Computer-Holder` names the teammate using a lease or
run slot; an absent header means `anonymous`.

## The desktop

The top bar holds the Toad mark, browser and terminal buttons, running-job
count, window buttons, and an XEmbed application tray. Click a window button
to focus it; right-click to close it. Normal apps occupy the work area below
the bar. The terminal opens on the right, alongside the current app.
`windows` operations verify the resulting focus, geometry, or disappearance;
a refused operation reports the remaining windows.

The observer displays commands and retained output without typing into a
terminal window. Closing or reopening it does not stop jobs. Jobs retain
4 MiB of output each, report truncation, and keep the newest 64 records;
16 jobs may run concurrently. Pipe and PTY stdin are supported. Cancellation
and deadlines kill the process group and reap the child. After a computer
restart, unfinished jobs become `interrupted`; they are not resumed.

The viewer's **Paste clipboard** button and Cmd/Ctrl+V send plain text only
while that viewer owns control. A reconnect starts view-only. A stale viewer
cannot paste over a newer viewer's control. Paste is limited to 1 MiB.

## Workspaces and the release guide

Start an agent session with `state info` and `state guide`. The returned skill
and SHA-256 come from the actual running binary, so any MCP client receives
instructions matched to its image. Toad's agent preamble uses the same path.

Clone a repository, then call:

```json
{"action":"prepare","name":"rust-tauri","workspace":"/home/agent/src/project"}
```

`state prepare` returns a managed job on a cold cache, or `ready:true` for a
cache hit. After a successful preparation, every shell job whose `cwd` is
inside that workspace inherits the environment. Explicit job `env` values
win. `python`, `go`, `node`, `rust`, and `rust-tauri` use the Nixpkgs commit
reported by `state catalog`. Recipe hashes invalidate stale caches. Nix
profiles retain their store dependencies; workspace metadata is in
`.toad/environment.json` and shared profiles are in
`~/.cache/toad/environments`.

The catalog supplies development environments: dependency headers may live
outside the repository, and unoptimized C dependencies can compile without
Nix's optimization-dependent fortify flags. Rust builds default to two jobs,
no incremental cache, and no debug symbols to fit a 4 GiB computer; callers
can override those environment values.

`files download` supports URLs and GitHub release assets with optional
SHA-256 verification. Asset patterns must match exactly one file. `extract`
requires a new destination and rejects traversal, links, special files, and
expanded archives over 1 GiB. `run` supports bash, sh, and python3 scripts.
These operations expose the same job status, progress, and cancellation as
shell commands. The complete examples ship in `state guide`.

## Boot

`toad-computer boot` is the entrypoint. As PID 1 it forks: the parent reaps
every child the kernel hands it and forwards SIGTERM; the child starts Xvfb
and dbus-daemon, becomes the window manager, and serves. A machine whose
display or bus has died exits, and the container with it.

`toad-computer serve` serves on a display that already exists, for running
the agent outside the container.

| variable | default | |
| --- | --- | --- |
| `TOAD_COMPUTER_ADDR` | `0.0.0.0:8787` | where `/mcp`, `/health`, and the viewer listen |
| `TOAD_COMPUTER_TOKEN` | unset | bearer for `/mcp`; unset means open |
| `TOAD_COMPUTER_HOME` | `/home/agent` | the directory `files` is confined to |
| `TOAD_COMPUTER_SCREEN` | `1920x1080` | the Xvfb screen `boot` creates |
| `DISPLAY` | `:0` | the display `boot` creates and `serve` uses |

## Build and run

```sh
make image                # docker build -t toad-computer:next .
make run                  # a hardened container on 127.0.0.1:8787, token in .token
make contract             # the contract test against it, through a real MCP client
make check                # fmt, clippy -D warnings, unit tests
make acceptance           # fresh image, real MCP/viewer tests, repository builds
```

`make run` is the create command Toad uses, spelled out:

```sh
docker run -d --name toad-computer-next \
  --cap-drop=ALL --security-opt no-new-privileges \
  --pids-limit 1024 --memory 4g --shm-size 1g \
  -p 127.0.0.1:8787:8787 \
  -e TOAD_COMPUTER_TOKEN="$(cat .token)" \
  toad-computer:next
```

Mount a workspace with `-v "$PWD:/home/agent/workspace"`. Chromium needs the
sized `/dev/shm`.

## Layout

```
src/boot.rs      PID 1, Xvfb, dbus, then the agent
src/desktop.rs   wallpaper, top bar, tray, window manager
src/serve.rs     the HTTP door: /health, bearer auth, /mcp, the viewer routes
src/viewer.rs    the viewer page and its socket
src/viewer.html  the page: one canvas, Take control, pointer and keys
src/screen.rs    DAMAGE-driven PNG rectangles for the viewer
src/xtest.rs     the person's pointer and keys, injected with XTEST
src/tools/       the eight tools
src/browser.rs   the managed Chromium over CDP
src/x11.rs       screenshots and EWMH window queries
src/a11y.rs      the AT-SPI tree as text
src/lease.rs     who holds the machine
src/jobs.rs      job lifetime, retained output, PTY, cancellation
src/observer.rs  Alacritty observer
src/workspace.rs pinned Nix environment catalog
src/guide.rs     the bundled release-matched skill
tests/contract.rs  the opt-in proof against a running container
```

The `desktop acceptance` workflow runs the same fresh-image gate on native
ARM64 and x86_64 runners. Evidence includes the image ID, tool-call trace,
job output, screenshots, and cold/warm timings. It does not publish images.
A release must pass both architecture gates before a version tag is created.
