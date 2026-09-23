---
name: hotline-computer
description: Operate a Hotline Computer through its MCP tools for browser work, installing and testing GitHub tools, and building or testing native desktop apps in its container.
---

Read `state {"action":"info"}` and `state {"action":"guide"}` when connecting to a computer. Use the guide returned by that running release. A desktop's configured image tag may differ from the computer currently attached to it.

This guide ships with Hotline Computer {{version}}, channel `{{channel}}`, revision `{{revision}}`. A development build is not an official release; compare this identity with the requested image before reporting release acceptance. The computer exposes eight MCP tools: `capture`, `input`, `browser`, `shell`, `files`, `windows`, `wait`, and `state`. A host may prefix those tool names. `state info` lists installed executable paths, including the managed `/usr/bin/chromium`; use these paths instead of downloading a second browser.

## Start with the task's shortest path

- Browser tasks: `browser navigate`, then `browser text`. Use returned refs with `fill`, `select`, `check`, and `click_ref`. Take a fresh snapshot after navigation or a page change. `fill` takes `text` (for example, `{"action":"fill","ref":"e3","text":"QA"}`), an explicit empty string clearing a field, or `secret` to type one of the person's stored secrets by name (below). Date/time inputs use native formats; `select` accepts `values` for multiple choices. An action result includes the retained value and HTML validity. Verify the submitted result, not just the field entry.
- CLI installation or native app QA: use `shell` and workspace preparation below. The desktop is a Linux glibc container; its CPU architecture is reported by `state info`. Do not install host macOS artifacts into it.
- Native screens: `capture` supplies a screenshot and window-scoped accessibility nodes. Use `input` for mouse/keyboard work and `windows` to focus or arrange apps. To place an app beside its observer, call `windows tile` with `primary_id` and `observer_id` from `windows list`; the result verifies work-area geometry and respects app minimum sizes. When accessibility is unavailable, use screenshot coordinates; never assume another window's tree belongs to this app.

## Build and run a repository: write a manifest, then run names

Describe the project once in `.hotline/manifest.json` and run what it names. Do not guess build or launch commands in the shell: when you are about to type one, add it to the manifest's `runs` instead, prepare, and run it by name. The manifest is the project's record of how it is built, so the next teammate, and the next image, start from it.

1. `state {"action":"manifest","workspace":"/home/agent/src/project"}`. With no manifest yet it returns a `draft` made from the repository's own lockfiles, toolchain and build files, with `reasons` saying why each line is there, plus the shape and what this image offers. Check the draft against the README and correct it: remove what the project does not use, add what the README asks for, and name the runs you will need.
2. Prepare with the corrected manifest. `state {"action":"prepare","workspace":"/home/agent/src/project","manifest":{...}}` merges what you pass into the file and prepares it; editing the file and preparing without `manifest` does the same. For example, for a Tauri app:

```json
{
  "packages": ["cargo", "rustc", "cargo-tauri", "bun", "openssl", "gcc"],
  "platform": ["webkit"],
  "env": {"PATH": ["node_modules/.bin"]},
  "hooks": {"create": {"install": ["bun", "install", "--cwd", "ui"]}},
  "runs": {
    "build": {"command": ["cargo", "build", "-p", "app"]},
    "test": {"command": ["cargo", "test"]},
    "app": {"command": ["cargo", "tauri", "dev"], "kind": "desktop"}
  }
}
```

3. If preparation returns `ready:false`, follow its job with `shell wait` and `shell read` until it succeeds. Create hooks then run once, and services and start hooks start; `state manifest` shows them as jobs under `activation`.
4. `shell {"action":"run","name":"build","cwd":"/home/agent/src/project"}` starts the named run in the prepared environment and returns a job. `args` are appended to the run's own. A `desktop` run answers when its window is up, with the window and a screenshot; a `web` run receives a free `$PORT` and answers when it listens, with its `url`. Both wait up to `wait_ms` (default 45 s); if the app is still building, the answer says so with its latest output, and `shell {"action":"ready","job_id":...}` waits again. If it exits first, the error carries its last output.

The parts of a manifest:

- `packages`: Nixpkgs attribute names, not Debian package names. Choose them from the project. `state catalog` lists common ones by purpose; if a name is uncertain, query the pinned Nixpkgs with `nix search github:NixOS/nixpkgs/<revision> <query> --json` in a shell job. Missing headers, libraries, or tools should guide the next change.
- `platform`: runtime support the image provides by name. `gl` is software OpenGL and EGL through Mesa; `gtk` (GTK 3) and `gtk4` add schemas, GIO modules and TLS; `webkit` adds WebKitGTK for Tauri and other webviews; `qt` adds Qt 6 with its plugins; `native` adds the X11, XKB and Vulkan libraries winit, wgpu, egui, iced, GLFW, SDL, Fyne and Gio build against and load at run time; `prebuilt` puts the system libraries a manylinux wheel may assume (libstdc++, zlib, expat, GLib, X11, OpenGL) on the library path for pip and uv wheels such as numpy, pandas and pygame, and for downloaded binaries; `qt-wheel` adds what PySide6 and PyQt wheels load beyond that (XCB, XKB, fonts, D-Bus). A Qt wheel brings its own Qt, so a PySide or PyQt project takes `qt-wheel`, not `qt`; Tk is the package `python312Packages.tkinter`. Each includes what it requires. Declare a platform instead of exporting library paths or driver variables yourself.
- `libraries`: Nixpkgs attributes put on the library path, for what a prebuilt binary or wheel loads beyond its platforms: a wheel failing with `libodbc.so.2: cannot open shared object file` takes `"libraries": ["unixODBC"]`. `prebuilt` holds only what the manylinux standard lets every wheel assume.
- `env`: `"NAME": "value"` sets a variable (`$WORKSPACE` expands); `"NAME": ["dir", ...]` extends a search path, relative entries resolved against the workspace. `PATH` is always a list.
- `services`: `{"postgres": {"enable": true}}` runs a supervised service on a unix socket under `.hotline/services`, and every job gets its address (`PGHOST`, `DATABASE_URL`, `REDIS_URL`).
- `hooks`: `create` entries run once per workspace after its first successful preparation, in name order (dependency installs); `start` entries start after each preparation unless already running (watchers). Services and start hooks come back by themselves after a computer restart.
- `runs`: `NAME: {"command": [argv], "cwd": "subdir", "kind": "task" | "desktop" | "web", "env": {}, "label": "..."}`.
- `flake`: a repository flake to take the environment from instead of `packages`, `platform` and `services`; `env`, `hooks` and `runs` still apply.

Layers compose: the image's base first, then the repository's file. Lists concatenate, maps merge, and scalars are last-wins. Errors name the valid choices. A run refuses to start when the manifest file changed after the last preparation; prepare again. Changing only runs, hooks or variables reattaches the same environment without a rebuild.

A workspace keeps the base it was composed against, including its Nixpkgs pin, when the image is updated; `state manifest` reports `update_available`, and `state prepare` with `"upgrade": true` takes the new one. New workspaces start on Nixpkgs {{nixpkgs}}. The composed definition is saved in `.hotline/environment-spec.json` and the generated flake and lock in `.hotline/nix/`.

Every job starts with build parallelism decided from the container's real CPU and memory limits: `CARGO_BUILD_JOBS`, `MAKEFLAGS`, `NIX_BUILD_CORES`, `CMAKE_BUILD_PARALLEL_LEVEL`, `OMP_NUM_THREADS` (which `nproc` reports) and `GOMAXPROCS`. `state info` shows them under `parallelism`. Do not pass `-j` yourself.

Without a manifest, `state prepare` still accepts `packages` (Nixpkgs attribute names) or `flake` (`"."`, `".#dev"`, or a subdirectory) alone, and reuses the saved definition, or a root `flake.nix`, when neither is given. That path names no runs, so prefer a manifest. Repository flakes are re-evaluated on each preparation while Nix reuses built packages; an existing `flake.lock` is preserved. Shell hooks run during preparation, in the workspace; their exported variables are retained. Shell aliases/functions and per-command hook execution require an explicit `nix develop --command ...` job.

The computer is rootless by design: every job runs as the normal container user, nothing can elevate, and the image carries no `apt` or `sudo`. Get software in this order: first what the computer prepares (`state prepare`, or `hotline-computer prepare` in a shell), then Nix by hand (`nix shell nixpkgs#<name>` for one tool), then what the image already has (`state info` lists it). A `.deb` or a system-wide install belongs in an image build, not in a job. An AppImage runs without FUSE: the image sets `APPIMAGE_EXTRACT_AND_RUN=1`. Electron runs from `node_modules` as it is: the image sets `ELECTRON_DISABLE_SANDBOX=1`, since its SUID sandbox cannot work here. A native app's runtime libraries come from the manifest's `platform`, not from exported paths. Launch it as a `desktop` run (or `shell launch` for a one-off), keep `cwd` in the workspace, use an isolated app data directory where supported, and inspect startup output and actual windows. Passwords and tokens an app or CLI keeps in the Secret Service (libsecret, `secret-tool store`/`lookup`) work from the start: the keyring is unlocked at boot, never prompts, and lives in the home.

## Commands and artifacts

`exec` waits up to 60 seconds. Use `start` for builds, servers, installers, or other long work; it returns a job ID promptly. `launch` uses the same job lifecycle for GUI apps. Supply an argv array, `cwd`, and optional `env`. Give every job a `label`: a short task in plain words (`Run the unit tests`, `Build the desktop app`, `Install Python 3.12`), because that is how a person watching the desktop's jobs list and terminal tells the jobs apart; the tools keep using the job ID. Use `request_id` when a start might be retried. A reused ID with different arguments is rejected.

- `shell read` accepts a byte `cursor` and returns `next_cursor`, output, state, and EOF. Keep the cursor when following output.
- `shell wait` waits up to `wait_ms` (maximum 60000) without locking the desktop.
- `shell write` sends `text` directly to stdin; `eof:true` closes pipe input. Use `pty:true` at start for software requiring a terminal. No keyboard typing into the terminal window is needed.
- `shell cancel` stops the job's process group and retains partial output. Inspect `state`, `exit_code`, `signal`, and `error`; a launch acknowledgement is not application readiness.
- A person watching can open a terminal of their own from the desktop's menu (T). When a step needs a person, such as signing in to a service or entering a credential you must not handle, say so and wait; their commands there are not jobs and leave no retained output, so read the result from the screen or from the files they change.
- `shell show` opens or focuses the Alacritty observer; pass `job_id` to inspect just that job. Closing that window leaves the jobs running; the top bar's terminal button reopens it. The job count opens a menu of retained commands, including completed and failed jobs. Output is capped at 4 MiB per job, with truncation reported. The most recent 64 jobs are retained; at most 16 run concurrently.

`files download` accepts a URL, destination `path`, and optional `sha256`. For GitHub releases, use `repo`, `version` (tag or `latest`), and an `asset` pattern that matches exactly one file. `{arch}` expands to `arm64` or `amd64`; `{os}` expands to `linux`. Supplied checksums are verified before the final file appears. The result reports whether verification occurred.

`files extract` takes an archive `path` and a new `destination` directory. It rejects traversal, links, special files, and expanded content above 1 GiB. `files run` runs an existing or downloaded script using `bash`, `sh`, or `python3`, with optional `args`, `cwd`, and `env`. These actions return managed jobs; inspect their exit status and retained output.

## Secrets the person put in this computer

`state info` lists them under `secrets`: each has a name and a kind, and the kind says how you use it. You never see a value: nothing returns one, and wherever a value would appear in a tool's answer it reads `[redacted NAME]` instead, so do not print, echo or copy one to read it, and do not work around that. If a task needs a secret that is not listed, ask the person to store it and grant it to you; never ask them to paste one to you.

- A `variable` is an environment variable in every job you run through `shell` and `files run` (not in `state prepare`), so use it as `$NAME` in a command, or leave it for a tool that reads that variable, the way `gh` reads `GITHUB_TOKEN`. A job's own `env` entry of the same name replaces the stored one for that job.
- A `login` is a username and password for the `sites` listed with it, sometimes with a TOTP seed (`totp: true`). It never enters a job. On a sign-in form, take a `browser text` snapshot, then `fill` each field with `secret` instead of `text`: `{"action":"fill","ref":"e3","secret":"GITHUB.username"}`, then `GITHUB.password`, and `GITHUB.code` for the six-digit code a second step asks for. The computer types the value only when the page's origin is one of the login's sites; on any other page the fill is refused, and that refusal is right: do not move the value another way. A username may be typed into a plain `text` if the form insists, since `state info` shows it; a password never.
- A `passkey` is your own WebAuthn credential for its `rpId`. It signs in by itself: choose the site's "sign in with a passkey" option and the browser answers without a prompt. Nothing types it, and `fill` refuses it. If a site asks to *create* a passkey and the browser refuses with "not armed", that is the person's step: tell them, and they arm it from the desk's Settings → Secrets and add the passkey through the screen or ask you to press the button while it is armed.

## Follow the person and recover from failures

The viewer starts view-only. A person explicitly takes control before sending input or host clipboard text. If a tool reports that the person holds the computer, continue read-only inspection or wait for release. The person can paste with the viewer button or Cmd/Ctrl+V; clipboard contents are sent only for that explicit paste.

Use actual failure evidence: job stderr for builds, the form's retained value and validity for browser errors, and the remaining windows for an uncompleted close or tile. Check storage when a build reports insufficient space. Toolchain/package setup errors belong to the computer environment; external API credentials or rate limits belong to the service being tested. After a computer restart, interrupted jobs remain inspectable and can be started again with a new request ID.

For QA, capture the final screens and report what the application actually did, including remaining failures. Do not treat a successful build or process launch as a completed UI test.
