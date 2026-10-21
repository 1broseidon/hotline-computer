# hotline-computer: launch punch list

Reviewed at 7d75305 (0.10.3). Work top to bottom. One item per PR, each with a test or a QA screenshot proving the "done when". Line numbers are against 7d75305.

The pitch is "your agent's own computer, on your machine, without your passwords." A security crowd will try to break exactly that, so P0 is the claims holding, P1 is a stranger getting it running, P2 is how it looks on camera.

---

## P0: the security claims have to be true

### 1. Agent can reach Chromium DevTools on 127.0.0.1:9222
`src/browser.rs:157` starts Chromium with `--remote-debugging-port=9222`. Shell jobs run as the same uid in the same network namespace, so `curl 127.0.0.1:9222/json` from a job is a full CDP session on the person's browser. From there: `WebAuthn.getCredentials` dumps passkey private keys, `Runtime.evaluate` reads any typed login on any origin, and base64 output walks past redaction. This defeats secrets, origin-bound logins and passkeys in one move. README:334-336 even says 9222 is reachable inside the container.
- Needs a design call from George before building. Options, cheapest first: CDP over a pipe (`--remote-debugging-pipe`) owned by the supervisor; run Chromium as a second uid with jobs blocked from its socket; Chromium in its own netns or sidecar container with the supervisor brokering CDP.
- Done when: a contract test runs `curl -s 127.0.0.1:9222/json/version` (and any port Chromium listens on) as a shell job and it fails, and every browser tool still passes.

### 2. Bearer token readable from /proc/1/environ
`jobs.rs:796` strips `HOTLINE_COMPUTER_TOKEN` from job env, but PID 1 runs as uid 1000 with the token in its environment, so `tr '\0' '\n' </proc/1/environ` from a job gets it (confirm in a running container). With it the agent can arm and approve its own passkey (`PUT /passkeys/registration`, `POST .../answer`), replace `/secrets`, and seize `/ws`.
- Fix: read the token once at startup, then take it out of PID 1's env (or pass it by file/fd and delete the file), and run jobs under a separate uid or mount `/proc` with `hidepid=2`. Also keep `--token` out of argv in docs for `serve`.
- Done when: a contract test reads `/proc/*/environ` and `/proc/*/cmdline` from a job and finds no token.

### 3. Default run is wide open
`Dockerfile:75` and `src/lib.rs:47-51`: binds `0.0.0.0:8787`, no token means no auth on every route (`serve.rs:161-166`, `viewer.rs:57`, `viewer.rs:93-96`). `docker run -p 8787:8787` publishes on every host interface: anyone on the LAN gets shell via `/mcp`, the live screen and control via `/ws`, and read/write of the home (keyring, Chromium profile, `~/.bashrc`) via `/files`. `seize` (`lease.rs:223`) overrides the person, so "the button waits" is only enforced in the page.
- Fix: no token set means generate one at boot and print the viewer URL and an MCP config line to the log (Jupyter style). Serve unauthenticated only with `HOTLINE_COMPUTER_INSECURE=1`, with a loud warning.
- Done when: `docker run` with no env returns 401 on `/mcp`, `/ws`, `/files`, and `docker logs` shows a working `http://127.0.0.1:8787/#<token>`.

### 4. DNS rebinding and cross-site requests against loopback
`serve.rs:119` calls `.disable_allowed_hosts()`; `/ws` and `POST /files` don't check `Origin`. A malicious page in the person's own browser can reach `127.0.0.1:8787`.
- Fix: allow-list Host (`localhost`, `127.0.0.1`, `[::1]`, plus `HOTLINE_COMPUTER_ALLOWED_HOSTS`); require same-origin `Origin` on `/ws` and the upload route.
- Done when: tests show a foreign `Host` and a foreign `Origin` are refused with a token present.

### 5. Upload has no size limit
`viewer::upload` takes a raw body with no cap. Disk fill DoS.
- Done when: uploads over a configured limit (default 2 GiB?) get 413 and leave nothing staged.

### 6. Say what redaction is and isn't
Redaction only rewrites text blocks (`tools/mod.rs:151`); `capture` images pass untouched, and `base64` or splitting beats it by design (`secrets.rs:30-33`). That's fine if stated. Not fine if the pitch implies it's an exfiltration boundary.
- Fix: README security section says redaction is context hygiene; the boundary is that secrets are never readable by tool or file. After items 1 and 2 land, that sentence becomes true.

### 7. Resource limits are only docs
pids, memory and shm limits live in `Makefile:21` and README:364. A bare `docker run` has none: one fork bomb kills the demo.
- Fix: put the flags in the quickstart (item 8) and add `RLIMIT_NPROC` on spawned jobs as a backstop. Add a Dockerfile `HEALTHCHECK` on `/health`.

---

## P1: a stranger gets it running in two minutes

### 8. Quickstart with the published image
The README never names `ghcr.io/1broseidon/hotline-computer`. The first command is `make image` at line 352; the only `docker run` uses a local tag and `cat /etc/timezone`, which fails on macOS.
- Fix: within the first 20 lines:
  ```sh
  TOKEN=$(openssl rand -hex 24)
  docker run -d --name computer \
    --cap-drop=ALL --security-opt no-new-privileges \
    --pids-limit 1024 --memory 4g --shm-size 1g \
    -p 127.0.0.1:8787:8787 -e HOTLINE_COMPUTER_TOKEN="$TOKEN" \
    -v computer-home:/home/agent \
    ghcr.io/1broseidon/hotline-computer:0.10.3
  open "http://127.0.0.1:8787/#$TOKEN"
  ```
- Done when: copy-paste works on a clean Mac (Docker Desktop and OrbStack) and a clean Linux box. Note Podman's 2 GB default VM and the uid 1000 bind-mount issue.

### 9. Connect your agent
No instructions anywhere for Claude Code, Cursor or Codex. Add tested snippets for each, then "first call: `state info`, then `state guide`." Mention the server already speaks the 2026-07-28 tool-list fields Claude Code requires.
- Done when: each snippet verified against a live container.

### 10. README rewrite for launch
414 lines, insider language up top ("the image is a contract, not a binary", "desk", "teammate" never defined), ~100 lines of desk-internal protocol, a stale 0.5 dev note (233-236), no screenshot.
- New order: pitch + GIF, quickstart, connect your agent, security model (rootless, caps dropped, Chromium `--no-sandbox` with the container as the sandbox, unencrypted keyring, what redaction does), the eight tools, limitations (no GPU, X11 only, Tk/Fyne have no a11y tree, first `prepare` pulls Nix), then Reference and Hotline integration.
- Requirements line: Docker or Podman, 4 GB RAM, ~500 MB pull. Re-measure boot time on 0.10.3 before quoting it.
- Label `qa/README.md` as historical (it still says `toad-computer`, "nothing published").

---

## P1: viewer blockers (src/viewer.html)

### 11. Bad or missing token looks like a dead page
No `#token` or a wrong one: black screen, "Reconnecting…" forever, retrying every second (`viewer.html:153`, `:180`, `:418`). Same as a stopped container. First thing an HN visitor will hit.
- Fix: say "Connecting…" until the socket has ever opened. If it closes before opening, probe `/files?token=` (401 means bad key: show "This link is missing its key. Open it from Hotline or add #<token>", stop retrying) and `/health` (fails: "The computer isn't running"). Otherwise back off 1, 2, 4… 15s. Hide the empty divider (`:18`).

### 12. Desktop is scaled soft and the bar covers it
1920x1080 in a 1440x900 window renders at ~0.75: the bar's 12px text becomes ~9px and blurry. `fit()` (`:166-170`) uses full height, so the control bar sits over the bottom 23px while watching.
- Fix: fit to `innerHeight - 64` (or park the bar in the letterbox); add a 1:1 toggle using `devicePixelRatio`. For recording, match `HOTLINE_COMPUTER_SCREEN` to the viewport.

### 13. Mac keyboard is broken while driving
Cmd goes over as Meta, which X treats as Super (`:96`, `xtest.rs:394`): Cmd+C/V/A do nothing. Ctrl+W closes the viewer tab. Tab and Escape are swallowed with no way out (`:399-408`).
- Fix: map Meta to Control on Mac (with opt-out); add a Fullscreen button with `navigator.keyboard.lock()`; reserve one documented combo that gives control back.

---

## P2: design and polish

### 14. Files panel
- Toast and panel both sit at `bottom:70px`, panel on top (`:45`, `:61`): upload status and errors are hidden while the panel is open. Show progress in the panel, or z-index the toast above.
- Dropping a file anywhere but the panel navigates the tab away. Cancel window-level `dragover`/`drop` and open the panel on drag-in.
- Uploads overwrite silently: page never sends `create_only` (`:334`). Send it, confirm "Replace notes.md?" on 409.
- No progress for big files. XHR with `upload.onprogress`.
- Rows are clickable `<li>`s (`:312`): make them buttons, show the `modified` date the API already returns, map raw errors ("unauthorized") to plain copy (`:290`).

### 15. Error toasts never leave
`say(msg, false)` never fades and can't be dismissed (`:142`, `:188`). `sticky` is set but never read (`:106`, `:141`). Fade after ~6s or on next input, click to dismiss, `role="alert"`, delete `sticky`.

### 16. Phone and touch
At 390px the bar is 495-631px wide and clips Take control and Files. A touch drag sends button-down then never button-up (`pointercancel` unhandled), leaving the remote mouse held. No `touch-action:none`, so the page pans and zooms.
- Fix: cap bar width, hide `.sub` and `.hint` under 600px and on `hover:none`; `touch-action:none`, `setPointerCapture`, treat `pointercancel` as release. Or ship phones as view-only and say so.

### 17. One product, one look
- Two typefaces: desktop bar is DejaVu Sans (`desktop.rs:71-72`), viewer is system-ui. Embed one sans (Inter or similar) with `include_bytes!` for the bar and serve it to the viewer via `@font-face`.
- Desktop popups are square with a 1px border (`desktop.rs:990-1005`) next to 6px pills and a 14px viewer panel. Pick one radius (8px) everywhere.
- Viewer icons are text characters `+ ↑ ×` (`:70`). Replace with inline SVG at 1.5 stroke.
- No favicon, no mark in the viewer. Use `assets/hotline-mark.svg` as a data-URI favicon and in the status pill.
- Light-on-dark bar text renders thin from gamma-space blending (`paint.rs:161`). Boost coverage for light ink; remove the dead `(descent * 0.0)` at `paint.rs:84`.
- Before the first frame the canvas is black and 1x1. Show the mark and "Waiting for the screen…".

### 18. Accessibility
- `--muted #7d7d82` on `#1c1c1f` is 4.15:1, under AA (`viewer.html:10`, `desktop.rs:63`). Raise to ~`#8e8e93` in both.
- No visible focus on the canvas or Take control (`:15`, `:25-28`). Keys only reach the machine when the canvas has focus, so show it.
- Canvas needs `role="img"` and a label; `aria-live="polite"` on status; `aria-label`s on `+ ↑ ×`; `aria-expanded` on Files.
- Respect `prefers-reduced-motion` for the pointer slide and click ring.

### 19. Liveness and latency
Idle desktop sends nothing, so a stalled socket looks like an idle agent. Send `{"t":"tick"}` every 5s, echo it, show "Live · 32 ms" while driving, amber dot after 10s silent (`viewer.rs:304-316`).

### 20. Copy
- "agent at work" shows when idle (`desktop.rs:845`, `viewer.html:162`): say "agent idle" when no jobs run.
- Menu says "the teammate's jobs" (`desktop.rs:1110`), everything else says "agent". Use "agent".
- Paste error "Paste into the application you are testing" (`viewer.rs:456`) assumes QA. Use "The observer is read-only. Click the app you want to paste into."
- "N failed" is amber on the desktop bar, grey in the viewer (`:159-163`). Amber in both.
- Move the always-on "Ctrl+Alt+V pastes from your computer" hint into the Paste tooltip plus a one-time toast.
- Jobs popup always says "scroll for more" (`desktop.rs:1161`), even when there's nothing more.

### 21. Dead code and stale comments
- `viewer.html:179` re-take-control-on-reconnect can never run; delete it.
- `desktop.rs:1252` and `:351` say menu keys are "A to D"; they're B, T, O, A.
- GHCR shows an `unknown/unknown` platform (build provenance). Set `provenance: false` in `image.yml` or accept it.

---

## Keep and show off
- The agent's pointer overlay that slides and rings on click (`viewer.html:35-42`, `:121-135`). Hero shot.
- View-only until Take control, then "The screen is yours" / "The screen is the agent's".
- One palette across viewer, bar, Alacritty and prompt.
- Token lives in the URL fragment and never leaves the browser; idle desktop sends zero bytes.
- The rootless `apt`/`sudo` messages in `bashrc`.
- Origin-binding logic itself is solid (https-only, no userinfo, dot-anchored subdomains, live URL re-checked at fill). Once item 1 lands, it's a real claim.
