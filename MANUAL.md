# Hotline Computer

A Linux desktop for your coding agent, in a container on your machine.

Any MCP client drives it with eight tools. You watch from your browser and take
the mouse when it needs you.

![Pinta displaying an orange cat illustration on the left, with the job observer on the right.](docs/screens/02-native-app-pinta.png "An illustration open in Pinta. The observer shows a completed Nix preparation job.")

![Chromium displaying the James Webb Space Telescope article on Wikipedia, with the job observer on the right.](docs/screens/01-browser-research.png "Reading Wikipedia in Chromium.")

![A terminal filling the left pane runs btop, showing CPU, memory, network use and processes.](docs/screens/03-terminal-btop.png "btop running in a full-height terminal on the left.")

![Chromium displaying an OpenStreetMap view of San Francisco, with the job observer on the right.](docs/screens/04-browser-openstreetmap.png "Exploring San Francisco in OpenStreetMap.")

![The computer's desktop: Chromium on the left showing a site served at 127.0.0.1:4321, the job log on the right with two jobs finished and one running, and the top bar reading agent at work, 2 running, 43 done.](docs/screens/desk.webp "Claude serving a site on its own computer, every job it ran logged on the right. 1920 by 1080.")

```console title="Install"
$ export TOKEN=$(openssl rand -hex 24)
$ docker run -d --name computer \
    --cap-drop=ALL --security-opt no-new-privileges \
    --pids-limit 1024 --memory 4g --shm-size 1g \
    -p 127.0.0.1:8787:8787 -e HOTLINE_COMPUTER_TOKEN="$TOKEN" \
    -v computer-home:/home/agent \
    ghcr.io/1broseidon/hotline-computer:0.11.0
```

```console title="For your agent"
Install Hotline Computer for me and connect it to this client.
1. Check that Docker, Podman or Apple container is running.
2. Generate a token with `openssl rand -hex 24` and start
   ghcr.io/1broseidon/hotline-computer:0.11.0 with the run command at
   https://computer.hotline.dev/#install, published on 127.0.0.1 only.
3. Add it as an HTTP MCP server at http://127.0.0.1:8787/mcp with the
   header `Authorization: Bearer <token>`.
4. Call `state info`, then `state guide`, and follow the guide.
5. Give me the viewer link, http://127.0.0.1:8787/#<token>, and no one else.
```

## Overview


One container is one machine. Inside it:

| Part | Role |
| --- | --- |
| Xvfb and Mesa | The X11 display and software OpenGL. No GPU needed |
| Chromium | The visible browser, driven over its DevTools protocol |
| Signed-in browser | A second, headless Chromium for the sites you sign the agent in to. Split computer only |
| D-Bus and AT-SPI | The accessibility tree the agent reads native apps by |
| gnome-keyring | The Secret Service that apps and CLIs keep passwords in |
| Nix | Toolchains a project needs, prepared per workspace |
| `hotline-computer` | PID 1: supervisor, window manager, top bar and MCP server. On the split computer, PID 1 only holds the ports and starts each part as its own user |

The agent connects directly to the local container over MCP. No account or
hosted relay is required.

A compatible computer exposes the same eight tools over streamable HTTP at
`/mcp` and an unauthenticated `/health` endpoint.

## Install

You need Docker, Podman or Apple container 1.5+, and 4 GiB of memory for the
container. The image ships for `arm64` and `amd64`.

```console title="Docker"
$ export TOKEN=$(openssl rand -hex 24)
$ docker run -d --name computer \
    --cap-drop=ALL --security-opt no-new-privileges \
    --pids-limit 1024 --memory 4g --shm-size 1g \
    -p 127.0.0.1:8787:8787 -e HOTLINE_COMPUTER_TOKEN="$TOKEN" \
    -v computer-home:/home/agent \
    ghcr.io/1broseidon/hotline-computer:0.11.0
```

| Flag | Why |
| --- | --- |
| `--cap-drop=ALL` | The single-user computer needs no Linux capabilities |
| `--security-opt no-new-privileges` | Nothing inside can gain privileges it did not start with |
| `--pids-limit 1024 --memory 4g` | Limits the container to 1,024 processes and 4 GiB of memory |
| `--shm-size 1g` | Chromium needs a sized `/dev/shm` |
| `-p 127.0.0.1:8787:8787` | Loopback only. Never publish it on every interface |
| `-e HOTLINE_COMPUTER_TOKEN` | Without a token, the computer refuses to start |
| `-v computer-home:/home/agent` | The home survives an upgrade or a recreate |

This command starts the single-user computer: everything runs as one
non-root user, `agent`. From 0.13.0, three more flags start the split computer
instead, and Hotline 0.50 creates it that way:

```console title="Split computer flags"
    --user 0 --cap-add SETUID --cap-add SETGID
```

| Flag | Why |
| --- | --- |
| `--user 0` | PID 1 starts as root, so it can start each part as its own user |
| `--cap-add SETUID --cap-add SETGID` | The only capabilities PID 1 keeps. It drops every other one, and nothing it starts keeps these |

On the split computer, PID 1 holds port 8787 and the key proxy's port
`127.0.0.1:3128`, so no job can take either. Then it runs:

| User | Runs |
| --- | --- |
| `hotline` (1001) | The broker: the MCP endpoint, the token, logins, passkeys and protected keys |
| `signin` (1002) | The signed-in browser, with DevTools on a pipe and no port |
| `agent` (1000) | Every job, the files, the desktop, Xvfb, D-Bus, the keyring and the visible browser on port 9222 |

`GET /capabilities?nonce=…` says which of the two is running. See
[HTTP routes](/guides/routes/).

To share a project, add `-v "$PWD:/home/agent/workspace"`.

> [!WARNING]
> Mount the project, never your whole home and never the container runtime's
> socket.

## Quickstart

Point your agent at the MCP endpoint with the same token.

```console title="Claude Code"
$ claude mcp add --transport http computer \
    http://127.0.0.1:8787/mcp \
    --header "Authorization: Bearer $TOKEN"
```

Codex, ChatGPT desktop and other HTTP MCP clients take the same URL and
`Authorization` header in their own config.

The agent's first two calls are `state info`, which names the running release,
and `state guide`, which returns the guide that ships inside that release. The
guide comes from the running binary, so it always matches the image.

Open the desktop in your browser:

```console title="Viewer"
$ open "http://127.0.0.1:8787/#$TOKEN"
```

## Tools

#### capture — screenshots

A scaled PNG of the screen plus the AT-SPI accessibility tree: windows,
elements, roles, coordinates, values and states. Pass a window to read one app,
or a region to see part of the screen at full size. `mode=png` writes the
original instead.

#### input — mouse and keyboard

Clicks, moves, drags, scrolls, types, presses keys and pastes, through XTEST.
`batch` runs up to ten steps under one lock.

#### browser — Chromium

Navigates, snapshots the page as text with element refs, and acts on those
refs: click, fill, select, check, hover, upload, tabs, dialogs and downloads.
`fill` types text, or one of your stored secrets by name. No action runs longer
than a minute.

#### shell — jobs

Starts jobs with a label you can read, keeps 4 MiB of output each, and lets the
agent read, wait on, write to or cancel them. Up to 16 run at once and the
newest 64 are kept. `run` starts a named entry from the workspace manifest.

#### files — file transfer

Gets, puts and lists paths under the home, except the computer's own records
under `~/.hotline`. Downloads URLs and GitHub release
assets with optional SHA-256 checks, extracts archives safely, and runs scripts
as managed jobs.

#### windows — window layout

Lists, focuses, closes, maximizes and tiles windows, and verifies the result.

#### wait — text polling

Polls the accessibility tree and the browser's page text for a phrase.

#### state — machine state

The running version and what the computer has to work with (CPUs, memory,
the disk left under the home and the Nix store, software rendering), your
stored secrets by name, the release guide, workspace manifests and Nix
preparation, desktop leases, browser logins and snapshots of the home.

## Viewer

The computer serves its own screen. `/` is a page with one canvas, and `/ws` is
the socket behind it. The page reads the token from the URL fragment. The
fragment is omitted from the initial page request; the viewer then sends the
token with its WebSocket and file requests.

The viewer opens in view-only mode. Mouse and keyboard input are disabled until
you press **Take control**. While you have control, the agent's tools that
change the screen are rejected. Control returns to the agent when you press
**Give it back**, close the viewer, or send no input for ten seconds.

![The Activity window: timestamped tool calls, among them Started Clone the Hotline site, a failed Opened 127.0.0.1/ and then a successful one, and Started Check the page answers.](docs/screens/activity.webp "The Activity window, failures included.")

The top bar shows who is working and what is running: `agent at work`,
`2 running · 5 done`, or amber `you're driving`. The jobs observer on the right
shows every job by its label, with its output and a ✓ or ✗ when it ends. The
Activity window lists the last 500 tool calls, failures included.

**Files** in the viewer opens the home: download anything from it, or drop a
file on the page to upload it. The computer's own records under `~/.hotline`
cannot be downloaded.

## Workspaces

The image has the desktop, the browser and Nix, and no language toolchains.
A project describes what it needs once, in `.hotline/manifest.json`, and the
agent prepares it:

```json
{
  "packages": ["nodejs", "pnpm"],
  "runs": {
    "test": { "command": ["pnpm", "test"] },
    "web": { "command": ["pnpm", "dev"], "kind": "web" }
  }
}
```

Packages are Nixpkgs names, or the repository's own flake. Jobs started inside
the workspace get the prepared environment. A `web` run gets a free `$PORT`
and answers with its URL once it listens; a `desktop` run answers once its
window is up, with a screenshot. A desktop run's `ready_title`, part of the
title its window has once it shows the right document, makes it wait for that
window; without one the answer says it is only the first window, which a
splash or an empty document can be.

A workspace is ready when its environment is built, its create hooks have
succeeded and its services are up, and `state manifest` says each of those in
one answer: every hook ran, was done before, or failed with its job and the
end of its output, and every service is up or not. A hook whose script is not
in the workspace is refused before anything is built. A create hook that
failed runs again on the next prepare; one that succeeded never does.

The prepared environment, its cache, the job history and the browser profile
all live in the home, so they survive an upgrade when the home is a volume.

## Security

> [!IMPORTANT]
> The container is the boundary. Inside it, the agent can do anything the
> container's user can do.

<div id="holds"></div>

### Protections

- No job runs as root. The image has no `sudo` and no `apt`, and with
  `--cap-drop=ALL` and `no-new-privileges` nothing a job starts can elevate.
  On the single-user computer nothing runs as root. On the split computer
  PID 1 is root with `SETUID` and `SETGID` alone, and runs nothing a job
  wrote.
- The MCP endpoint, the viewer's socket and its file routes all require the
  token. Only `/health` and the viewer page itself are open.
- A job never inherits `HOTLINE_COMPUTER_TOKEN`. On the split computer the
  token is held by the broker's user, out of a job's reach.
- Stored secrets are sent by name. A readable variable reaches jobs as an
  environment variable. A login is typed only on its own sites. A passkey's
  private key is never put in the browser.
- On the split computer, a protected key never reaches a job. Each job gets a
  stand-in, and the key proxy puts the real value only in the service's own
  header, only for the origins it is bound to.
- On the split computer, logins, passkeys and the sites you share cookies for
  are used only in the signed-in browser. The agent cannot run scripts there, and
  sees nothing of a tab from the first secret typed into it until it shows a
  new page. Your typing on its screen goes through DevTools, never the X
  display jobs can listen to.

<div id="gaps"></div>

### Limitations

- Redaction replaces secret values with `[redacted NAME]` in tool text
  responses. Code can still expose values by splitting or encoding them.
  Redaction does not cover non-text results or the screen.
- On the single-user computer, jobs and Chromium run as the same user, so a
  job can reach the browser's DevTools port and the extension's socket, and
  the browser's cookies are in reach. On the split computer this is still
  true of the visible browser on port 9222, which holds none of your logins
  or shared sites.
- A readable variable is in every job's environment, so any code a job runs
  can read it.
- The keyring's password is empty. Its file in the home is not encrypted, so
  the home volume is the boundary around what is kept there.
- Chromium runs with `--no-sandbox`, both browsers; the container is their
  sandbox.

## Limits

- No GPU. Graphics render on Mesa in software.
- X11 only. No Wayland.
- Linux apps only.
- One desktop per container. A second agent gets a second container.
- A workspace's first prepare downloads its Nix packages, which takes a while.
- Tk and Fyne apps have no accessibility tree; the agent sees them as pixels.

## Configuration

| Variable | Default | |
| --- | --- | --- |
| `HOTLINE_COMPUTER_TOKEN` | unset | The bearer for `/mcp`, the viewer and its files. Unset, the computer refuses to start unless `HOTLINE_COMPUTER_ALLOW_NO_TOKEN=1` |
| `HOTLINE_COMPUTER_ADDR` | `0.0.0.0:8787` | Where the server listens inside the container |
| `HOTLINE_COMPUTER_SCREEN` | `1920x1080` | The display's size |
| `HOTLINE_COMPUTER_HOME` | `/home/agent` | The directory `files` is confined to |
| `HOTLINE_COMPUTER_GUTTER` | `16` | Space around app windows; `0` for edge to edge |
| `TZ` | `UTC` | The clock in the top bar |

## For agents

Start every session with `state info` and `state guide`, and follow the guide
that comes back: it is the one for the release that is actually running.

Prefer `browser text` and refs for web pages, and `capture` with a window for
native apps. Give every job a `label` in plain words, because that is what the
person watching reads. When a step needs the person, such as a sign-in or a
2FA tap, say so and wait; they can take control from the viewer.

Never ask the person to paste a secret. If a task needs one that `state info`
does not list, ask them to store it.

## Notes

Hotline Computer is the desktop each teammate gets in
[Hotline](https://hotline.dev), released on its own. Hotline starts one
container per teammate and gives it the token; everything on this page works
without it.

The [guides](/guides/) cover each subject in more detail.

## Upgrading to 0.12.0

Set `HOTLINE_COMPUTER_TOKEN` before starting the service. Startup now refuses a missing or empty bearer by default. Keep the endpoint private even with authentication enabled.

Loopback and IP-address connections work without extra configuration. If you connect by DNS name, list it in `HOTLINE_COMPUTER_ALLOWED_HOSTS`. If a browser frontend calls from another origin, list that trusted origin (scheme, host and port if non-default) in `HOTLINE_COMPUTER_ALLOWED_ORIGINS`. Both settings accept comma-separated entries.

HTTP uploads default to 50 MiB. Larger uploads receive HTTP 413; a deployment can set `HOTLINE_COMPUTER_UPLOAD_LIMIT_MIB` to another limit.

Use a separate computer without credentials for workloads you do not trust. The browser, credential services and shell jobs are not isolated tenants. A desktop lease coordinates input; it is not a sandbox for background jobs. Grant only the folders and credentials needed for the task.

## Upgrading to 0.13.0

Created as before, the computer is the single-user computer, as in 0.12.0,
with four leaks closed:

- `files` and the viewer's download refuse paths under `~/.hotline`: job
  output, the browser profile, saved logins and snapshots. A job can still
  read them.
- A login's current six TOTP digits are redacted as `[redacted NAME.code]`.
- `browser fill` refuses a variable. It stays in every job's environment.
- The Activity and job observer windows are sealed as the service is, so a
  job cannot attach to them or read their memory.

Created with `--user 0 --cap-add SETUID --cap-add SETGID` beside
`--cap-drop=ALL`, it is the split computer, described under [Install](#install). Hotline 0.50 creates it that
way. A Hotline 0.49 desk creates the single-user computer and works with it
unchanged.

`PUT /secrets` also takes a v2 body. A variable in it is either readable, as
before, or a protected key bound to a service. Only the split computer takes a
protected key; the single-user computer refuses one. A v1 body is read as
before. See [Secrets](/guides/secrets/).

The first time a home boots as a split computer, it is migrated once:

- The saved logins in `~/.hotline/logins` are deleted.
- The visible browser's cookies for those sites, and for every login's sites,
  are cleared.
- Snapshots made before are quarantined, since they hold the old browser
  profile, saved logins and job output. `state snapshot_load` refuses them.

`GET /migration` reports what was done and lists the quarantined snapshots.
`POST /migration/answer {"snapshots": "clean"}` removes those parts from them,
`"delete"` deletes them, and `"keep"` leaves them quarantined. Nothing is
erased without that answer. The snapshots stay the agent's own files, so a
job can still unpack one by hand; `clean` is the answer that removes what
they hold.
