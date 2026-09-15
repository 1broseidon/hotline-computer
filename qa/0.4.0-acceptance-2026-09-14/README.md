# Toad Computer 0.4.0 acceptance QA

2026-09-14. Related work: BRO-23 and BRO-4 (George identified them as related; this report does not assume their issue descriptions).

**Recommendation: use Alacritty as the first Linux terminal observer, backed by a job runner owned by Toad Computer.** Both Alacritty and Ghostty worked once Mesa software rendering was configured. Alacritty used less memory and started faster in the tested container; its explicit Unix socket and headless daemon fit the existing Rust/X11 toolset well. Keep terminal selection replaceable at the small process-launch boundary.

The acceptance run reached all three requested task categories. Browser workflows exposed correctness bugs. Ketch installed and scraped successfully. Toad built and displayed its native screens after substantial environment assistance. This is **not an unmodified-image end-to-end pass**, and it is not a release approval.

## Environment and provenance

- Computer source: `1broseidon/toad-computer`, main `51c5586`, version 0.4.0; image `sha256:d834a67d90b036d25404beb7323d1a8b7b0eb9a81009f950c0bb887c52f3895f`.
- Dedicated local OrbStack container: `toad-acceptance-040`, Linux ARM64, Alpine 3.22.5, uid 1000. Separate from George's own test computer and teammate computers.
- Container memory limit 8 GB; OrbStack VM reported about 5.85 GiB available. No hardware GPU exposed. X11 desktop 1920 × 1080; window-manager work area 1920 × 1016.
- Nixpkgs pinned at `ef34387ddd751e1ab8857adf4676492d32eb24ec`. Nix 2.20.6, Alacritty 0.17.0, Ghostty 1.3.1, Mesa 26.2.2.
- Toad source: `1broseidon/toad` main `f3c17b78d85b4cc235455862d9c9241579157b39`, version 0.11.0. The latest release inspected, desktop-v0.10.2, supplied x86_64 Linux assets; this ARM64 computer took the source-build path.
- Toad runtime data isolated at `/home/agent/qa/toad-data`. No provider credentials supplied and no agent conversations sent.
- Computer actions ran through its public MCP tools. Explicit host-assisted environment changes are listed below. Product sources were not changed; no release was published.

## Acceptance results

| Task | Result | Evidence / limits |
|---|---|---|
| Simple browser form | Passed | Required validation, text/email/textarea, select, checkbox, submit. [Screenshot](evidence/01-simple-form.png). |
| Complex browser wizard | Completed with workarounds | Three steps, invalid-email recovery, conditional organization, date, upload, multiple selections, back/review, asynchronous submit. Native date fill and multi-select required DOM workarounds. [Wrong date](evidence/02-complex-review.png), [correct final result](evidence/03-complex-submitted.png). |
| Independent public browser form | Passed, with readonly false-success finding | Selenium's public web form submitted and returned “Received!”. [Result](evidence/05-public-form-received.png). |
| Install Ketch CLI | Passed | Official v0.16.2 Linux ARM64 archive, SHA-256 verified: `61e0a7ef16350534c586d7e2755a490333334fc40bdeac01c092bf77bf030b8c`. Runs directly on Alpine. |
| Ketch functional checks | Scraping passed; search not validated | Version/help/config JSON, HTTP scrape, local HTML extraction, forced browser scrape, JS-only rendered fixture passed. Brave lacked a key (exit 5); explicit DDG fallback rate-limited (exit 4). Browser path needed `/usr/bin/chromium`. |
| Clone/build Toad | Passed after environment assistance | Frozen Bun install, UI production build, `cargo build --locked -p toad-desktop --features tauri/custom-protocol`; successful Rust build reported 5m11s after dependency preparation. [Build log](evidence/toad-build.log). |
| Toad native screen QA | Passed smoke checks | Empty room, General, Providers, Computer, Updates, New teammate form, cancellation/navigation. [Empty room](evidence/10-toad-after-focus.png), [settings](evidence/12-toad-settings-settled.png), [Computer](evidence/14-toad-computer.png), [new teammate](evidence/15-toad-new-teammate.png). No authenticated agent run, nested computer, or release updater exercised. |
| Terminal observers | Both passed configured integration trial | Tool-driven argv/cwd/env, IPC window creation, close/reopen while job ran, replay of retained transcript, intentional exit 42 displayed. No terminal keyboard typing. |

## Alacritty versus Ghostty

The final benchmark alternated the candidates for six launches each, excluded one warmup per candidate, then reported medians across five successful trials. Both used font size 12, their default scrollback policies, the same desktop, Nix software-GL configuration, and the same 4,900,000-byte / 50,000-line payload. The harness starts a Python child directly through `-e` and records its first instruction. Startup therefore means **process launch to child execution**, not first visible pixel. The output metric measures writer completion through the PTY, not rendered frames per second. PSS is proportional memory of the terminal process, including its renderer threads, excluding the workload child.

| Measurement | Alacritty 0.17.0 | Ghostty 1.3.1 |
|---|---:|---:|
| Successful measured trials | 5/5 | 5/5 |
| Median launch to child | 75 ms | 247 ms |
| Idle PSS | 131 MiB | 254 MiB |
| Idle RSS | 133 MiB | 258 MiB |
| 4.9 MB writer completion | 148 ms | 308 ms |
| PSS after output | 180 MiB | 266 MiB |
| Terminal CPU during output + 350 ms settle | 0.48 CPU seconds | 1.76 CPU seconds |
| IPC initial attach, one sample | 93 ms | 192 ms |
| IPC reopen, one sample | 105 ms | 182 ms |

[Raw benchmark measurements](evidence/terminal-benchmark-results.json), [summary](evidence/terminal-benchmark-summary.json), [harness](recipes/terminal-benchmark.py), [integration results](evidence/terminal-integration-result.json).

These are warm-cache microbenchmarks on one ARM64 VM with software rendering. They do not establish GPU performance, macOS performance, cold-install latency, long-term durability, or identical scrollback memory behavior. A first harness run had a marker-file publication race; it was corrected with atomic writes and the full comparison repeated. That race was a QA harness defect, not a Ghostty failure.

### Integration behavior actually tested

Alacritty ran with `--daemon --socket <path>`. Its `msg create-window` accepted a title, working directory, command, and argument vector. Ghostty ran as an explicitly named GTK singleton with `initial-window=false` and `quit-after-last-window-closed=false`; `+new-window --class=...` accepted working directory, title, and `-e` arguments through D-Bus. Both received an explicitly supplied environment variable via `env`, which the observer verified. Both daemon PIDs remained alive after their last windows closed, and the independent producer remained alive. Both reopened onto the saved log and displayed its deliberate exit-42 result.

[Ghostty reopened transcript](evidence/17-terminal-reopened.png), [Alacritty reopened transcript](evidence/18-alacritty-reopened.png).

Ghostty's normal close confirmation required a visible confirmation click when a command was running. The observer trial set `confirm-close-surface=false`, because closing an observer must only detach that observer. [Default close dialog](evidence/20-ghostty-close-button.png). This is expected terminal behavior, not a crash. The computer's `windows close` currently reports that the request was sent without proving that the window disappeared.

Both failed their first launch without usable EGL/OpenGL. Explicit Mesa/GLVND library and vendor paths plus `LIBGL_ALWAYS_SOFTWARE=1` resolved both. The extra Mesa trial closure downloaded 56.97 MiB and unpacked 270.01 MiB; that is an incremental dependency cost, not either terminal's complete installed footprint.

For this scope Alacritty provides the smaller measured runtime and simpler explicit socket addressing. Ghostty's GTK integration, tabs, splits, and shell-integration features may be useful for a human-focused terminal, but they did not improve the tested observer workflow. Its Linux window API is viable; macOS AppleScript command sending should not be confused with a Linux API for writing to an existing job's stdin. Neither tested window API replaces a managed job/PTY service.

Primary references: [Alacritty IPC](https://alacritty.org/cmd-alacritty-msg.html), [Alacritty license](https://github.com/alacritty/alacritty/blob/master/LICENSE-APACHE), [Ghostty Linux new-window implementation, v1.3.1](https://github.com/ghostty-org/ghostty/blob/v1.3.1/src/cli/new_window.zig), [Ghostty configuration](https://ghostty.org/docs/config/reference), [Ghostty MIT license](https://github.com/ghostty-org/ghostty/blob/v1.3.1/LICENSE).

## Confirmed friction and proposed fixes

Priorities below are QA recommendations, not changes already implemented.

| Priority / owner | Observed problem | Proposed correction |
|---|---|---|
| P1 Computer browser | Native `input[type=date]` fill reported success for `2026-09-15` but produced `60915-02-02`. Readonly fill also reported success without changing the value. | Make fill type-aware, reject readonly/disabled controls, dispatch appropriate events, and read back the actual value. Cover native dates and controlled forms. Entry point: `src/browser.rs` fill. |
| P1 Computer accessibility | Capture attached Chromium's accessibility tree to the native window titled `Toad`. `titles_match` accepts substrings, so `Toad Computer … Chromium` matches `Toad`. | Prefer application/window identity (PID/bus identity/window handle), require unambiguous matching, and return unavailable rather than unrelated controls. `src/a11y.rs:19`, `:143`. |
| P1 Computer jobs | Shell timeout kills the process group and loses partial output. Detached launch discards stdout/stderr unless manually redirected, reports only a PID, and never waits for the child; finished launches became zombies. | Managed job IDs, incremental retained output, status/wait/cancel, early startup failure, process-group ownership, and reaping. `src/tools/shell.rs`. |
| P1 Environment | Stock image lacks Bash, Git, Nix bootstrap, and language runtimes; mixed Alpine/Nix libc detection broke a normal JS native dependency. | Ship a supported bootstrap and a consistent userland for named environment recipes. Cache prepared environments and perform preflight checks. Avoid teaching every agent ad hoc libc surgery. |
| P2 Computer browser | Multi-select API accepts only one value; text output includes hidden controls and omits useful value/checked/selected/readonly/invalid state. | Accept values arrays, expose control state, prioritize visible actionable elements. Preserve a way to inspect hidden state deliberately. |
| P2 Computer windows | Xterm missing from list despite being alive. It has `WM_NAME` but no `_NET_WM_NAME`; empty successful lookup bypasses the existing fallback. | Fall back for absent/empty `_NET_WM_NAME`, not just errors. `src/x11.rs:171`. |
| P2 Computer input | Immediate screenshots were stale. `settle_ms` only applies between batch steps, not after the final step; single clicks ignore it. | Add explicit post-action settling/readiness and capture after it. Prefer bounded state/frame conditions over arbitrary sleeps. `src/tools/input.rs:241`. |
| P2 Computer layout | Tile uses full 1080-pixel screen height despite 1016-pixel work area; crowded five-window layout did not keep Ghostty in the requested tile. It returns a count without verifying geometry. | Honor work area and window size constraints, choose explicit primary/observer windows, verify final geometry. Investigate GTK placement race separately; do not label all of it a Ghostty defect. `src/tools/windows.rs:52`. |
| P2 Desktop shell | Only a Chromium dock icon; fullscreen raw build output looked like a broken computer. | Top desktop bar with Toad mark at top-left, running-app tray, terminal job status, and reopen/focus controls. Label job name, command, phase, elapsed time, and exit status. |
| P2 Toad integration | The built Toad's Computer settings default to image 0.3.0 while this QA computer runs 0.4.0. | Discover the actual running computer version/capabilities and load its matched skill. Do not infer runtime knowledge solely from a configured default image tag. |
| P2 Ketch | `browser=chrome` failed because this image has `chromium`; browser status printed an error with exit 0. | Discover the installed browser or consume a computer-provided browser executable capability. Make failed status machine-detectable. Search failures remain external configuration/service limitations. |

### Host-assisted environment workarounds

1. Rootless nix-portable needed Bash; unpacked Bash/readline/ncurses APKs into the agent's home and used an explicit musl-loader wrapper. A globally exported bootstrap `LD_LIBRARY_PATH` poisoned glibc children; scoped loader use fixed it.
2. Rootless proot could clone, but cold Nix imports were slow and exceeded the shell timeout. A native `/nix` store required host Docker assistance to create a writable real directory. A failed store migration produced malformed metadata; that was a QA setup error and was preserved separately before fresh metadata initialization.
3. Rolldown detected musl from Alpine's `/usr/bin/ldd`, although Node ran against Nix glibc. Within this isolated QA container, host assistance moved that script to `/usr/bin/ldd.alpine`. The actual musl loader remained intact. This unblocked the UI build; it is a diagnostic workaround, not a recommended catalog implementation.
4. Toad's first native launch panicked because dynamic appindicator lookup could not find its library. The runtime recipe supplies its library path. GTK subsequently disabled hardware acceleration but rendered the tested screens successfully.

## Proposed product shape

The following is the implementation proposal arising from this run; it does not describe existing tools.

**Computer owns jobs.** A launch request supplies `argv`, `cwd`, explicit environment, environment recipe, and a label. Return a stable job ID immediately, then provide bounded reads by cursor, wait, stdin/PTY write, and cancel. Persist transcript and structured state independently of the terminal window. Reap children, distinguish spawn failure from nonzero exit, retain partial output on timeout, and report signal/exit status. A full container restart needs explicit recovery semantics; window reattachment alone does not provide process resurrection.

**Alacritty observes jobs.** Start a managed daemon and open a purpose-built observer by socket, title, and job ID. Tools execute commands directly. A visible window renders the retained command/output stream; closing it leaves the job alone. The tested Python observer is proof of this separation, not a production PTY implementation. Interactive stdin should go through the same ownership and takeover rules as other computer input, without synthesizing terminal keyboard events. The desktop bar and Toad Desktop consume the same job state for running/completed/failed indicators.

**Computer ships release-matched workflow knowledge.** Bundle a small skill and environment catalog in the release image. Advertise actual server version, image/build identity where available, skill revision, catalog revision, and supported capabilities. MCP discovery should expose the guide to any agent; Toad Desktop should inject/reference that same guide through its existing computer grant/session setup. Pin it to the running server, including an existing reused container, rather than the UI's intended/default image. Dev overrides must identify themselves explicitly.

**Catalog entries produce usable workspaces.** Start with Python, Go, Node, Rust, and Rust/Tauri. Each entry pins its Nix inputs and declares build tools, runtime libraries, graphics policy, executable paths, and smoke checks. A prepare operation selects an entry and returns a ready environment handle with progress and cache status. A script-download operation should return the resolved URL, artifact path and hash, followed by direct execution as a managed job. Skill text should teach selection and recovery; deterministic tools should perform setup, downloads, execution, and verification.

## Next acceptance gate

Re-run these cases from a fresh image after fixing the browser false-success cases, accessibility identity, and job lifecycle. Require no host edits for the CLI and Rust/Tauri recipes. Verify the top bar, reopenable terminal, and takeover gating on the shared job service. Then run a longer churn test and at least one x86_64 environment before extrapolating the terminal recommendation across architectures.

The QA scripts under `recipes/` preserve this experiment, including absolute paths to the realized Nix store and run-specific directories. Use a fresh QA directory/container for another run; these are evidence recipes, not a portable shipped catalog yet. The sanitized MCP trace omits inline image payloads and known local viewer credentials; screenshots and selected logs are retained separately.
