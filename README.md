# hotline.computer

A small Linux desktop for a coding agent. One container is one machine: a
display, a browser, a shell, a home directory, and one MCP server that lets
an agent see the screen, act on it, and check what happened. Hotline starts one
per teammate; anything else that speaks MCP can point at it too.

The image is a contract, not a binary. Anything that serves these eight tools
over streamable HTTP at `/mcp`, with `/health` open, is a valid computer.

## What is in the box

```
hotline-computer  PID 1, supervisor, window manager, bar, MCP server
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
a header on a WebSocket; Hotline opens `http://127.0.0.1:<port>/#<token>` and the
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
- `browser` drives the visible Chromium over CDP; element refs last for one text snapshot. `fill` types text, or one of the person's stored secrets by name (`secret` instead of `text`), and a login's field only onto that login's own sites. No action runs longer than a minute, and a browser the person closed is replaced by the next call rather than waited on. A managed policy (`assets/chromium-policy.json`) turns off the password manager, autofill and sign-in, so nothing typed into a form is offered for keeping and no bubble sits over the page.
- `shell` starts managed jobs, reads retained output, writes stdin, waits, cancels, and opens the terminal observer.
- `files` gets, puts, and lists paths below the computer home; downloads, verifies, extracts, and runs artifacts through managed jobs.
- `windows` lists, focuses, closes, maximizes, and tiles windows.
- `wait` polls the accessibility tree and browser page text for a phrase.
- `state` identifies the running version and lists the person's stored secrets by name and kind, returns its guide/catalog, prepares workspaces, and manages leases, browser logins, and snapshots.

`/health` and the viewer page never require authentication. When
`HOTLINE_COMPUTER_TOKEN` is set, every method on `/mcp` and the desk's
`/secrets`, `/passkeys/registration` (with its `/answer`) and `/logins/{name}` doors require `Authorization: Bearer <token>`, the viewer's socket
and its `/files` routes
require the same token as their `token` query, and otherwise all return a JSON 401. `X-Computer-Holder` names the teammate using a lease or
run slot; an absent header means `anonymous`.

## The desktop

The top bar is three answers. On the left, the Hotline mark opens a menu, and
each row's initial picks it while the menu is open: **B** the browser
(opened, or focused if open), **T** a terminal of the person's own, **O** the
observer of the teammate's jobs, **A** an About card (version, channel,
revision, architecture, nixpkgs revision, uptime); Escape closes it. In the middle, every open window is a pill with its own icon and title;
click one to focus it, right-click to close it. On the right, a jobs chip
(`no jobs`, `2 running · 5 done`, `1 failed · 2 running`) that drops the
jobs list from under itself, a lease chip that reads `agent at work`, `agent in control` or
`person in control`, an XEmbed application tray that appears when an app
uses it, and a clock in the host's zone (`TZ`, which Hotline passes; UTC
otherwise). Everything on the bar is published as `_HOTLINE_BAR_LAYOUT` on the
root window, so tests find its parts by rectangle rather than by pixel.
Normal apps occupy the work area below the bar. The observer opens in the
right third of the screen, and the app being watched keeps the left two
thirds; `windows tile` uses the same split. The person's terminal opens in
the bottom third of that column, under an 8 px line, so it never covers
the app on the left: an interactive bash in the mounted workspace with the
environment the teammate prepared there, for signing in to something or
unblocking a stuck step. It is the system's bash with Hotline's own rc file,
written to `~/.hotline/bashrc` on each open: the blue prompt, history kept
under `~/.hotline`, and no Debian skeleton files in the home. To customise it,
create `~/.bashrc`; it runs after Hotline's. What is typed there is not a job:
it is not retained, and the teammate sees only what is on the screen. From there,
`hotline-computer prepare --packages go gopls` prepares the workspace by hand
the way the teammate's job would, and `hotline-computer packages` lists common
Nixpkgs names to choose from. What the person installs for themselves runs
by name: `~/.local/bin`, `~/go/bin` and `~/.cargo/bin` lead the shell's PATH.
The computer has no file manager; a folder opens as a fresh terminal in it.
The browser's **Show in folder**, `xdg-open` on a folder, and
`hotline-computer open DIR` all do that, through the image's desktop entry for
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
instructions matched to its image. Hotline's agent preamble uses the same path.

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

The workspace owns `.hotline/environment-spec.json`. Package definitions retain
their Nixpkgs revision across Computer upgrades and dependency additions;
`.hotline/nix/` contains their generated flake and lock. Shared caches are under
`~/.cache/hotline/environments`, keyed by recipe and architecture, independently
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
`"flake":".hotline/nix"`.

The base image supplies the desktop, browser, graphics, and Nix. Framework
libraries and compilers are project dependencies; WebKit and AppIndicator
are no longer installed in the base image. Native projects can specify their
runtime library paths in a flake.

The computer is rootless by design: nothing runs as root, nothing can
elevate, and the image carries no `apt` or `sudo`, so nobody is invited to
try. Software comes in this order: what the computer prepares
(`state prepare` for a teammate, `hotline-computer prepare` for a person), then
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

`hotline-computer boot` is the entrypoint. As PID 1 it forks: the parent reaps
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

The person's own secrets come the other way, from the desk. `PUT /secrets`,
with the bearer in the `Authorization` header and a JSON object of name to
entry as the body, replaces the whole set the computer holds; the desk sends
it when the computer starts and again whenever the person stores, replaces
or removes one, or grants one to this teammate. The set stays in the
service's memory — the computer itself writes no value to disk. A name is
what a shell variable can be called (`GITHUB_TOKEN`), never one starting
with `HOTLINE_` nor one the shell owns (`PATH`, `HOME`, …), and every entry
has a kind that says where it goes:

- A **variable** (`{"kind":"variable","value":…}`, or a bare string, which
  is all a desk before 0.16 sends) is an environment variable in each job
  the agent starts through `shell` or `files run`, under its name. A
  preparation job is left out, because what it captures is written into the
  workspace. A value is at least eight characters and at most 64 KiB.
- A **login** (`{"kind":"login","sites":[…],"username":…,"password":…,
  "totp":…}`) never enters a job. `browser fill` with `secret` instead of
  `text` types one of its fields by name — `NAME.username`, `NAME.password`,
  or `NAME.code`, the six TOTP digits of this moment from the base32 seed —
  and only onto a page whose origin, as Chromium reports it, is one of the
  login's sites or lies under one. A site is an `https://` origin, or
  `http://` on localhost. The page a password is typed into can read it, so
  which page is the whole protection.
- A **passkey** (`{"kind":"passkey","rpId":…,"credentialId":…,
  "privateKey":…,"userHandle":…,"userName":…}`) is the teammate's own
  WebAuthn credential, kept in a virtual authenticator on every tab of the
  managed browser. It signs in by itself when a site asks the browser for
  one, without a prompt; nothing types it and no tool answers it.

One bad entry refuses the whole set with a 400 that names it, keeping the
last set. Nothing answers a value: there is no `GET`, `state info` lists
each secret's name, kind, and for a login its sites and username, and every
text a tool returns has each value — a variable's, a login's password and
seed, a passkey's private key — replaced with `[redacted NAME]` before it
leaves, in the spelling JSON gives it too. Usernames and sites stay
readable: a page that says who is signed in must still make sense. That
keeps a value out of the agent's context when a job prints it or a page
echoes it. It does not stop a command written to get one out, in pieces,
encoded, or into a file, and the screen the person watches is never
redacted. A job does not inherit `HOTLINE_COMPUTER_TOKEN` either: the
bearer is the service's, not a command's.

A passkey is made once, and making one is the person's act, twice over.
The desk arms this computer for one site with `PUT /passkeys/registration
{"rpId":"github.com"}` — for ten minutes, one site at a time, answering
`{"state":"armed","rpId":…,"expiresAt":…}` and starting the browser on
that site if none is up. While armed, and for that site alone, a call to
`navigator.credentials.create()` in the managed browser is not answered by
the browser: a guard installed in every document of every tab that carries
passkeys parks it and keeps what the site asked for — the site, the
origin, the account's name and display name — and the next look (every
action, every poll) records it as the request before the person.
`GET /passkeys/registration` then answers `asked` with that `ask` (`id`,
`rpId`, `origin`, `rpName`, `userName`, `userDisplayName`, `askedAt`); the
desk shows the person a card and carries their answer back with
`POST /passkeys/registration/answer {"id":…,"approved":true}`. Approved,
the page is told to go ahead, the tab's virtual authenticator mints, and
`GET` answers `approved` and then `registered` with the minted
`credential`, until the desk stores it in the vault, delivers the set with
it, and ends the arming with `DELETE`. Denied, the site gets a
`NotAllowedError` — as it would from a person cancelling the browser's own
prompt — and the arming ends with the denial, so neither a site nor a
teammate can keep asking; an answer to a request that is not waiting is a
409. The guard rejects `create()` for any other site, or with none armed,
with a `NotAllowedError` that says so, and holds one request before the
person at a time. The credential's answer is the one time a private key
leaves the computer, over the bearer-guarded loopback door the desk
already uses. A credential the authenticator holds that is neither granted
nor made under an approved request is removed at the next look, so a
passkey made outside an arming never survives one, and a revoked passkey
is gone from every tab as soon as the set without it arrives. DevTools on
port 9222 inside the container can reach the same authenticators;
nothing outside the container can.

`hotline-computer serve` serves on a display that already exists, for running
the agent outside the container.

| variable | default | |
| --- | --- | --- |
| `HOTLINE_COMPUTER_ADDR` | `0.0.0.0:8787` | where `/mcp`, `/health`, and the viewer listen |
| `HOTLINE_COMPUTER_TOKEN` | unset | bearer for `/mcp`, `/secrets`, `/passkeys/registration` (with its `/answer`) and `/logins/{name}`; unset means open |
| `HOTLINE_COMPUTER_HOME` | `/home/agent` | the directory `files` is confined to |
| `HOTLINE_COMPUTER_SCREEN` | `1920x1080` | the Xvfb screen `boot` creates |
| `DISPLAY` | `:0` | the display `boot` creates and `serve` uses |

## Build and run

```sh
make image                # docker build -t hotline-computer:next .
make run                  # a hardened container on 127.0.0.1:8787, token in .token
make contract             # the contract test against it, through a real MCP client
make check                # fmt, clippy -D warnings, unit tests
make acceptance           # fresh image, real MCP/viewer tests, repository builds
```

`make run` is the create command Hotline uses, spelled out:

```sh
docker run -d --name hotline-computer-next \
  --cap-drop=ALL --security-opt no-new-privileges \
  --pids-limit 1024 --memory 4g --shm-size 1g \
  -p 127.0.0.1:8787:8787 \
  -e HOTLINE_COMPUTER_TOKEN="$(cat .token)" \
  -e TZ="$(cat /etc/timezone)" \
  hotline-computer:next
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
src/secrets.rs   the person's stored secrets by kind: the door, the job environment, fill by name, redaction
src/passkeys.rs  the arming under which the browser may make a passkey, and its door
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

## License

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE)
or the [MIT license](LICENSE-MIT), at your option. Unless you explicitly
state otherwise, any contribution intentionally submitted for inclusion in
the work by you, as defined in the Apache-2.0 license, shall be dual
licensed as above, without any additional terms or conditions.
