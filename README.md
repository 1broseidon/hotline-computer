# toad.computer

A small Linux desktop for a coding agent. One container is one machine: a
display, a browser, a shell, a home directory, and one MCP server that lets
an agent see the screen, act on it, and check what happened. Toad starts one
per teammate; anything else that speaks MCP can point at it too.

The image is a contract, not a binary. Anything that serves these eight tools
over streamable HTTP at `/mcp`, with `/health` open, is a valid computer.

## What is in the box

```
toad-computer  PID 1, supervisor, window manager, bar, MCP server
├── Xvfb       the X server; pixels in RAM, no GPU
├── dbus       the session bus the accessibility tree rides on
├── keyring    gnome-keyring, the Secret Service, unlocked from boot
└── chromium   the visible browser, driven over its DevTools protocol
```

The image uses Debian glibc, Xvfb, Mesa software rendering, Nix, and a
non-root agent account. It runs with every capability dropped. Native apps
and GPU-backed terminal emulators use llvmpipe when no hardware GPU exists.

| Component | Purpose |
| --- | --- |
| Xvfb and Mesa | X11 desktop and software OpenGL/EGL |
| D-Bus and AT-SPI | Native application accessibility |
| gnome-keyring | The Secret Service native apps and CLIs keep passwords in |
| Chromium | Visible browser controlled through CDP |
| Alacritty | Reopenable observer for tool-driven shell jobs |
| Nix | Project-selected Nix packages or repository dev shells |
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
- `browser` drives the visible Chromium over CDP; element refs last for one text snapshot. No action runs longer than a minute, and a browser the person closed is replaced by the next call rather than waited on. A managed policy (`assets/chromium-policy.json`) turns off the password manager, autofill and sign-in, so nothing typed into a form is offered for keeping and no bubble sits over the page.
- `shell` starts managed jobs, reads retained output, writes stdin, waits, cancels, and opens the terminal observer.
- `files` gets, puts, and lists paths below the computer home; downloads, verifies, extracts, and runs artifacts through managed jobs.
- `windows` lists, focuses, closes, maximizes, and tiles windows.
- `wait` polls the accessibility tree and browser page text for a phrase.
- `state` identifies the running version, returns its guide/catalog, prepares workspaces, and manages leases, browser logins, and snapshots.

`/health` and the viewer page never require authentication. When
`TOAD_COMPUTER_TOKEN` is set, every method on `/mcp` requires
`Authorization: Bearer <token>`, the viewer's socket and its `/files` routes
require the same token as their `token` query, and otherwise all return a JSON 401. `X-Computer-Holder` names the teammate using a lease or
run slot; an absent header means `anonymous`.

## The desktop

The top bar is three answers. On the left, the Toad mark opens a menu, and
each row's initial picks it while the menu is open: **B** the browser
(opened, or focused if open), **T** a terminal of the person's own, **O** the
observer of the teammate's jobs, **A** an About card (version, channel,
revision, architecture, nixpkgs revision, uptime); Escape closes it. In the middle, every open window is a pill with its own icon and title;
click one to focus it, right-click to close it. On the right, a jobs chip
(`no jobs`, `2 running · 5 done`, `1 failed · 2 running`) that drops the
jobs list from under itself, a lease chip that reads `agent at work`, `agent in control` or
`person in control`, an XEmbed application tray that appears when an app
uses it, and a clock in the host's zone (`TZ`, which Toad passes; UTC
otherwise). Everything on the bar is published as `_TOAD_BAR_LAYOUT` on the
root window, so tests find its parts by rectangle rather than by pixel.
Normal apps occupy the work area below the bar. The observer opens in the
right third of the screen, and the app being watched keeps the left two
thirds; `windows tile` uses the same split. The person's terminal opens in
the bottom third of that column, under an 8 px line, so it never covers
the app on the left: an interactive bash in the mounted workspace with the
environment the teammate prepared there, for signing in to something or
unblocking a stuck step. It is the system's bash with Toad's own rc file,
written to `~/.toad/bashrc` on each open: the blue prompt, history kept
under `~/.toad`, and no Debian skeleton files in the home. To customise it,
create `~/.bashrc`; it runs after Toad's. What is typed there is not a job:
it is not retained, and the teammate sees only what is on the screen. From there,
`toad-computer prepare --packages go gopls` prepares the workspace by hand
the way the teammate's job would, and `toad-computer packages` lists common
Nixpkgs names to choose from. What the person installs for themselves runs
by name: `~/.local/bin`, `~/go/bin` and `~/.cargo/bin` lead the shell's PATH.
The computer has no file manager; a folder opens as a fresh terminal in it.
The browser's **Show in folder**, `xdg-open` on a folder, and
`toad-computer open DIR` all do that, through the image's desktop entry for
`inode/directory`, titled with the folder (`~/Downloads`).
`windows` operations verify the resulting focus, geometry, or disappearance;
a refused operation reports the remaining windows.

The observer is an Alacritty window in the bar's colours that displays
commands and retained output without typing into a terminal. Each job is a
block headed by its name, the `label` the teammate gave it (or its command
line when it gave none), with the job id, start time and directory in grey
underneath and a ✓ or ✗ line naming it again when it ends. Closing or
reopening it does not stop jobs. Jobs retain
4 MiB of output each, report truncation, and keep the newest 64 records;
16 jobs may run concurrently. Pipe and PTY stdin are supported. Cancellation
and deadlines kill the process group and reap the child. After a computer
restart, unfinished jobs become `interrupted`; they are not resumed.

The viewer's control bar sits at the foot of the page. Watching, it reads the
machine's state (whose it is, how many jobs, how many failed) beside **Take
control**; if another person holds the screen the button waits. Driving, it
shows **Paste** and **Give it back** and fades while the pointer is still, so
it never sits over the screen being driven. Ctrl+V, Ctrl+C and the rest
reach the machine as keys, so its apps use their own clipboard. Text from
the viewer's computer comes in on Ctrl+Alt+V (⌥⌘V on a Mac) or the
**Paste** button, as plain text, only while that viewer owns control. A
reconnect starts view-only. A stale viewer cannot paste over a newer
viewer's control. Paste is limited to 1 MiB.

**Files**, watching or driving, opens a panel over the screen: the home and
what is under it, as the machine lists it. A folder opens, the arrow goes up
as far as the home, and a file is saved on the viewer's own computer by the
page's browser. Behind it, `GET /files?path=` lists a folder as JSON
(`path`, `home`, `entries` with `name`, `size`, `is_dir`, `modified`) and
`GET /files/download?path=` streams a file as an attachment; both take the
token as a query like `/ws`, an empty path means the home, and both stop at
the home like the `files` tool. A file from the viewer's computer goes the
other way with the panel's **+** button or a drop onto the panel: it lands
where the panel is looking, through `POST /files?path=` with the file as
the body, written whole or not at all, under the home only. Inside the
computer, the managed Chromium lists a folder at `file:///home/agent/`.

## Workspaces and the release guide

Start an agent session with `state info` and `state guide`. The returned skill
and SHA-256 come from the actual running binary, so any MCP client receives
instructions matched to its image. Toad's agent preamble uses the same path.

Read the repository's requirements, then prepare the packages it needs:

```json
{"action":"prepare","packages":["nodejs","pnpm"],"workspace":"/home/agent/src/project"}
```

Or use a repository flake, including a named dev shell:

```json
{"action":"prepare","flake":".#dev","workspace":"/home/agent/src/project"}
```

Omit `packages` and `flake` to reuse the saved definition, or discover the
workspace's root `flake.nix` on first use. Package names are Nixpkgs attribute
paths. Flakes must be local directories under the computer home; relative
paths are resolved from the workspace. There are no language or framework
presets. `state catalog` describes these inputs and the default Nixpkgs pin.

Preparation returns a managed job, or `ready:true` for a package cache hit.
Follow or cancel it with the existing shell tools. Once it succeeds, shell
jobs with `cwd` inside the workspace inherit the exported environment;
explicit job `env` values win. A failed preparation retains the previous
active environment. Nix profiles keep the prepared store dependencies alive.

The workspace owns `.toad/environment-spec.json`. Package definitions retain
their Nixpkgs revision across Computer upgrades and dependency additions;
`.toad/nix/` contains their generated flake and lock. Shared caches are under
`~/.cache/toad/environments`, keyed by recipe and architecture, independently
of the Computer version. Keep the definition and lock with the workspace.
The image keeps nothing of its own in the home: its Alacritty
configuration, bash rc and Chromium policy live under `/etc`. So a home
given a volume, as the desk gives the store and the teammate's scratch,
carries prepared environments, their cache, jobs and their output, the
shell's history and the browser profile across a recreate, whether for an
upgrade or a hibernate cycle. The acceptance run replaces its container on
the same volumes and checks that it does.

Repository flakes are re-evaluated on each preparation, reusing Nix's store
cache. Their existing locks cannot be silently updated; first use may create
a lock. Shell hooks run during preparation in the workspace, and exported
variables are retained. Aliases, functions, and hooks that must run for every
command require an explicit `nix develop --command ...`. To customize a
package environment, edit its generated flake and prepare with
`"flake":".toad/nix"`.

The base image supplies the desktop, browser, graphics, and Nix. Framework
libraries and compilers are project dependencies; WebKit and AppIndicator
are no longer installed in the base image. Native projects can specify their
runtime library paths in a flake.

The computer is rootless by design: nothing runs as root, nothing can
elevate, and the image carries no `apt` or `sudo`, so nobody is invited to
try. Software comes in this order: what the computer prepares
(`state prepare` for a teammate, `toad-computer prepare` for a person), then
Nix by hand (`nix shell nixpkgs#<name>` for one tool), then what the image
already has. The person's shell answers `apt` and `sudo` with that order.
A `.deb` or a system-wide install belongs in an image build. An AppImage
runs without FUSE, which a container cannot offer: the image sets
`APPIMAGE_EXTRACT_AND_RUN=1`, and it carries every library on the AppImage
exclude list, the ones AppImage tooling never bundles because every desktop
is assumed to have them, from `libgpg-error` to `libOpenGL`, so an AppImage
built anywhere finds what it expects.

**0.5 development API change:** `prepare name=<preset>` is replaced by
`packages` or `flake`. Old prepared environments remain usable while their
Nix store paths exist; adopting the new preparation API requires a package
list or flake. `catalog` now reports preparation sources instead of profiles.

`files download` supports URLs and GitHub release assets with optional
SHA-256 verification. Asset patterns must match exactly one file. `extract`
requires a new destination and rejects traversal, links, special files, and
expanded archives over 1 GiB. `run` supports bash, sh, and python3 scripts.
These operations expose the same job status, progress, and cancellation as
shell commands. The complete examples ship in `state guide`.

## Boot

`toad-computer boot` is the entrypoint. As PID 1 it forks: the parent reaps
every child the kernel hands it and forwards SIGTERM; the child starts Xvfb
dbus-daemon and gnome-keyring, becomes the window manager, and serves. A
machine whose display or bus has died exits, and the container with it; a
keyring that dies is started again.

## Secrets

Anything that uses libsecret, from a native app to `secret-tool`, finds a
Secret Service on the session bus from boot: gnome-keyring with its login
collection unlocked, so a store is a store and never a prompt on the desktop.
The keyring's password is empty, which means its file under
`~/.local/share/keyrings` is not encrypted. The home volume is the boundary
around a computer's secrets, as it is for everything else the person and the
agent keep there. Environments prepared with Nix share the same bus and so the
same keyring.

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
  -e TZ="$(cat /etc/timezone)" \
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
src/workspace.rs workspace-owned Nix definitions and preparation
src/guide.rs     the bundled release-matched skill
tests/contract.rs  the opt-in proof against a running container
```

The `desktop acceptance` workflow runs the same fresh-image gate on native
ARM64 and x86_64 runners. Evidence includes the image ID, tool-call trace,
job output, screenshots, and cold/warm timings. Native-screen checks require
both the expected accessibility controls and a visible pixel change. The runner
creates a temporary Python environment for its pinned image-comparison dependency.
It does not publish images. A release must pass both architecture gates before a
version tag is created. The tag/manual publishing workflow repeats acceptance,
then verifies the pushed image has the same configuration digest as the tested
image before publishing the combined architecture manifest.
