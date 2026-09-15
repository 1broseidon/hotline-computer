---
name: toad-computer
description: Operate a Toad Computer through its MCP tools for browser work, installing and testing GitHub tools, and building or testing native desktop apps in its container.
---

Read `state {"action":"info"}` and `state {"action":"guide"}` when connecting to a computer. Use the guide returned by that running release. A desktop's configured image tag may differ from the computer currently attached to it.

This guide ships with Toad Computer {{version}}, channel `{{channel}}`, revision `{{revision}}`. A development build is not an official release; compare this identity with the requested image before reporting release acceptance. The computer exposes eight MCP tools: `capture`, `input`, `browser`, `shell`, `files`, `windows`, `wait`, and `state`. A host may prefix those tool names. `state info` lists installed executable paths, including the managed `/usr/bin/chromium`; use these paths instead of downloading a second browser.

## Start with the task's shortest path

- Browser tasks: `browser navigate`, then `browser text`. Use returned refs with `fill`, `select`, `check`, and `click_ref`. Take a fresh snapshot after navigation or a page change. `fill` requires `text` (for example, `{"action":"fill","ref":"e3","text":"QA"}`); an explicit empty string clears a field. Date/time inputs use native formats; `select` accepts `values` for multiple choices. An action result includes the retained value and HTML validity. Verify the submitted result, not just the field entry.
- CLI installation or native app QA: use `shell` and the environment catalog below. The desktop is a Linux glibc container; its CPU architecture is reported by `state info`. Do not install host macOS artifacts into it.
- Native screens: `capture` supplies a screenshot and window-scoped accessibility nodes. Use `input` for mouse/keyboard work and `windows` to focus or arrange apps. To place an app beside its observer, call `windows tile` with `primary_id` and `observer_id` from `windows list`; the result verifies work-area geometry and respects app minimum sizes. When accessibility is unavailable, use screenshot coordinates; never assume another window's tree belongs to this app.

## Build and run a repository

A typical Rust/Tauri task uses this sequence (replace the repository and directory with the user's target):

1. `shell {"command":"git","args":["clone","https://github.com/1broseidon/toad.git","/home/agent/src/toad"]}`
2. `state {"action":"prepare","name":"rust-tauri","workspace":"/home/agent/src/toad"}`
3. If preparation returns `ready:false`, use `shell wait` and `shell read` with the returned job ID until it succeeds. Nix download/build progress is retained in that job.
4. Read the repository's build instructions with `files get` or a shell command. Run its actual build using `shell start` with `cwd` inside the prepared workspace. Commands inherit the catalog environment. Use direct argv or `bash -c` for compound commands; a login shell (`bash -lc`) can reset the prepared PATH. No `nix develop` wrapper is needed.
5. Launch the app with `shell launch`, keeping `cwd` in the workspace and using an isolated app data directory where supported. Read its startup output, list windows, and capture the screens under test.

`state catalog` lists `python`, `go`, `node`, `rust`, and `rust-tauri`. The `rust-tauri` profile includes the prebuilt `cargo tauri` CLI and native GTK/WebKit dependencies. Preparation is cached across workspaces. The catalog pins Nixpkgs {{nixpkgs}} and rejects stale environment metadata after a computer upgrade; rerun `state prepare` then. Repositories with their own Nix flake may use that flake through a managed shell job instead.

## Commands and artifacts

`exec` waits up to 60 seconds. Use `start` for builds, servers, installers, or other long work; it returns a job ID promptly. `launch` uses the same job lifecycle for GUI apps. Supply an argv array, `cwd`, and optional `env`. Use `request_id` when a start might be retried. A reused ID with different arguments is rejected.

- `shell read` accepts a byte `cursor` and returns `next_cursor`, output, state, and EOF. Keep the cursor when following output.
- `shell wait` waits up to `wait_ms` (maximum 60000) without locking the desktop.
- `shell write` sends `text` directly to stdin; `eof:true` closes pipe input. Use `pty:true` at start for software requiring a terminal. No keyboard typing into the terminal window is needed.
- `shell cancel` stops the job's process group and retains partial output. Inspect `state`, `exit_code`, `signal`, and `error`; a launch acknowledgement is not application readiness.
- `shell show` opens or focuses the Alacritty observer; pass `job_id` to inspect just that job. Closing that window leaves the jobs running; the top bar's terminal button reopens it. The job count opens a menu of retained commands, including completed and failed jobs. Output is capped at 4 MiB per job, with truncation reported. The most recent 64 jobs are retained; at most 16 run concurrently.

`files download` accepts a URL, destination `path`, and optional `sha256`. For GitHub releases, use `repo`, `version` (tag or `latest`), and an `asset` pattern that matches exactly one file. `{arch}` expands to `arm64` or `amd64`; `{os}` expands to `linux`. Supplied checksums are verified before the final file appears. The result reports whether verification occurred.

`files extract` takes an archive `path` and a new `destination` directory. It rejects traversal, links, special files, and expanded content above 1 GiB. `files run` runs an existing or downloaded script using `bash`, `sh`, or `python3`, with optional `args`, `cwd`, and `env`. These actions return managed jobs; inspect their exit status and retained output.

## Follow the person and recover from failures

The viewer starts view-only. A person explicitly takes control before sending input or host clipboard text. If a tool reports that the person holds the computer, continue read-only inspection or wait for release. The person can paste with the viewer button or Cmd/Ctrl+V; clipboard contents are sent only for that explicit paste.

Use actual failure evidence: job stderr for builds, the form's retained value and validity for browser errors, and the remaining windows for an uncompleted close or tile. Check storage when a build reports insufficient space. Toolchain/package setup errors belong to the computer environment; external API credentials or rate limits belong to the service being tested. After a computer restart, interrupted jobs remain inspectable and can be started again with a new request ID.

For QA, capture the final screens and report what the application actually did, including remaining failures. Do not treat a successful build or process launch as a completed UI test.
