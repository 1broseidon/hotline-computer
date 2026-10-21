# hotline-computer 0.11 look: overnight build spec

You are implementing a visual refresh of hotline-computer unattended. George QAs in the morning from your report and screenshots. Read this whole file before writing code.

Repo state at writing: `main` at 7d75305 (0.10.3). The design target is a mockup George has on a canvas; this file carries every value you need, so you don't need to see it.

---

## 0. Ground rules

**Branch and git**
- Create `look/0.11` from `main`. Commit at the end of each phase (and smaller commits inside a phase are fine). Commit messages follow the repo's style: a plain declarative sentence about what the computer now does.
- Never push, never merge to `main`, never tag, never bump the version in `Cargo.toml`, never publish an image.

**Out of bounds. Do not change:**
- Auth, tokens, `/secrets`, `/passkeys/*`, `/logins/*`, Chromium launch security flags, the DevTools port, redaction. Those are tracked in `LAUNCH-PUNCHLIST.md` P0 and need George's decisions.
- The MCP tool contract: still exactly 8 tools with the same required arguments. Additive optional output fields are fine.
- `README.md` security wording. You may update README sections that describe the bar, the viewer and the desktop layout so they stay true.

**Loop for every phase**
1. Implement.
2. `make check` (fmt, clippy `-D warnings`, tests). Fix until green.
3. `make image && make run && make contract`.
4. Run the QA script (section 9) for the scenes that phase touches. Save screenshots.
5. Append the phase's section to the report (section 10). Commit.

**If a phase won't go green after 3 honest attempts:** `git revert` that phase's commits, note why in the report, move to the next phase. Phases 1 to 6 are independent enough to skip one. Phases 7 and 8 are stretch: only start them if 1 to 6 are done.

**If Docker isn't available:** do phases that are verifiable with `make check` alone, mark every screenshot step "not run: no Docker", and stop after phase 6.

**Comments:** this codebase explains *why* in plain prose comments. Match it. Remove comments your change makes false.

---

## 1. Design tokens (source of truth)

| Token | Hex | Use |
|---|---|---|
| ground | `#0b0b0d` | wallpaper, gutters |
| bar | `#111114` | top bar fill |
| bar-rule | `#222227` | 1px line under the bar |
| surface | `#17171b` | menus, popups, panels, viewer sheets |
| raised | `#1f1f25` | active pill, hovered row |
| raised-edge | `#2e2e35` | 1px inset edge on the active pill |
| chip | `#19191e` | neutral chip fill |
| chip-edge | `#26262c` | neutral chip border |
| rule | `#2a2a30` | borders, window ring |
| ink | `#ececef` | primary text |
| ink-2 | `#b4b4ba` | secondary text |
| pill-ink | `#a2a2a9` | inactive window pill text |
| muted | `#8e8e95` | tertiary text. Must stay at least 4.5:1 on bar and surface |
| agent | `#8fb8ff` | everything the agent is doing: running dot, agent cursor, "agent at work" |
| you | `#f0c27a` | everything the person is doing: "you're driving", Take control |
| ok | `#86d69c` | success, sealed, origin matched |
| fail | `#ff8f80` | failed jobs, errors |

Tinted chip fills are the accent blended at 16% over `bar`. Add a `blend(fg, bg, alpha)` helper rather than hardcoding; for reference agent-soft is about `#252c3a`, you-soft about `#352d24`.

**Type:** Geist for UI, Geist Mono for terminal and code. Sizes in px at 1:1: bar labels 12.5 regular, agent name 13 semibold, clock 13 with tabular figures, menus 13, viewer UI 13 to 14.

**Shape:** chips and pills radius 8, height 26 (chips) and 28 (pills). Bar height 40. Gutter 16. One shadow style.

**Semantics change from 0.10:** person-in-control moves from blue (`YOU 0x7fc4f0`) to amber; failed moves from amber (`WARN`) to coral; the agent gets periwinkle. Update every place that used the old meaning, in `desktop.rs`, `viewer.html` and `assets/alacritty.toml`.

---

## 2. Phase 1: fonts and palette

**Fonts**
- Vendor static TTFs into `assets/fonts/`: `Geist-Regular.ttf`, `Geist-Medium.ttf`, `Geist-SemiBold.ttf`, `GeistMono-Regular.ttf`, `GeistMono-Medium.ttf`, plus `OFL.txt`. Source: the `vercel/geist-font` GitHub release. Also produce `.woff2` copies for the viewer if a converter is available (`fonttools` / `woff2_compress`); otherwise serve the TTFs.
- If you can't download them, fall back to Debian packages `fonts-inter` and `fonts-jetbrains-mono` in the runtime `apt-get install` list, point everything at those, and say so at the top of the report.
- Dockerfile: `COPY assets/fonts/*.ttf /usr/share/fonts/truetype/hotline/` and run `fc-cache -f` in that layer. Add `assets/fonts-hotline.conf` installed to `/etc/fonts/conf.d/10-hotline-defaults.conf` that makes `sans-serif` prefer Geist and `monospace` prefer Geist Mono. Keep `05-hotline-system-fonts.conf` working (Nix apps read `/etc/fonts/conf.d`).
- `src/desktop.rs`: replace the DejaVu `SANS`/`MONO` paths. Add a semibold face to `Fonts` for the agent name. Keep DejaVu installed (other things may depend on it); it's just no longer the bar's face.

**Palette**
- `src/desktop.rs:54-67`: map the constants to section 1. Add `AGENT`, `FAIL`, `CHIP`, `CHIP_EDGE`, `SURFACE`, `BAR_RULE`.
- `src/viewer.html`: CSS custom properties to the same tokens. Rename the agent pointer's variable from `--you` to `--agent` (it currently uses `--you` for the agent: `viewer.html:40`). Load the fonts with `@font-face` from a new static route (below).
- New route `GET /assets/fonts/{name}` in `src/serve.rs` serving only the vendored font files by an allow-listed name, with a long `Cache-Control`. Add the path prefix to the open list in `authenticate()` alongside `/`; it serves nothing but fonts. Unit-test that a name outside the allow-list is 404 and `..` is refused.

**Text rendering**
- `src/paint.rs:161`: light text on dark renders thin because coverage is blended linearly. When ink is lighter than the background, use `coverage.powf(1.0 / 1.4)` before blending. Add a unit test on one glyph edge pixel.
- `src/paint.rs:84`: remove the dead `(descent * 0.0)` term.

**Bar height**
- `BAR_HEIGHT` 36 to 40 (`desktop.rs:56`). Update `src/tools/windows.rs:270-275` test expectations and anything else that assumed 36. `_HOTLINE_BAR_LAYOUT` and `_NET_WORKAREA` must reflect the new value.

**Done when:** `docker exec hotline-computer-next fc-match sans-serif` names Geist (or Inter on fallback); scene S1 shows the bar in Geist on `#111114`; the viewer page computes its body font as Geist.

---

## 3. Phase 2: gutters, window ring, painted shadow

The WM is non-reparenting (`desktop.rs` `manage` / `apply_geometry`): it only positions client windows. Keep it that way. No frames, no compositor.

**Gutters**
- New `GUTTER: u16 = 16`, overridable by env `HOTLINE_COMPUTER_GUTTER` (0 restores edge to edge; agents that want every pixel can opt out).
- The work rect becomes `(GUTTER, BAR_HEIGHT + GUTTER, width - 2*GUTTER, height - BAR_HEIGHT - 2*GUTTER)`. Add a `work_rect()` helper and use it everywhere `work_height()` and the `(0, BAR_HEIGHT, width, ...)` tuples are used today: maximized windows (`apply_geometry`), `column()` (observer and shell), the terminal split in `manage`, dialog centering, `windows tile`, `_NET_WORKAREA` in `announce()`.
- The split between the app and the right column is a `GUTTER`-wide gap, not abutting windows. The shell under the observer is separated by `GUTTER` instead of `STACK_GAP`.
- A window that asks for `_NET_WM_STATE_FULLSCREEN` still covers the whole screen, gutters and bar included, if the WM supports fullscreen today; don't regress it.

**Ring and shadow, painted into the wallpaper**
- Extend `paint_wallpaper()` (`desktop.rs:603`) to take the current list of visible managed client rects and paint, in this order, onto the ground: the mark (unchanged, centred), then for each rect a soft shadow (offset y 10, blur radius 24, black at 45% max alpha, built as an alpha mask with three box-blur passes in `paint.rs`), then a 1px `rule` ring hugging the outside of the rect.
- Repaint on any layout change: map, unmap, destroy, `apply_geometry`, column restack. Debounce to at most one repaint per 50 ms, and one `clear_area` per repaint, so a burst of window changes is one screen update for the stream.
- Skip override-redirect windows, the bar, tray icons, popups, and fullscreen windows.
- On a near-black ground the shadow is subtle by design; the ring and the lighter surfaces carry the separation.

**Done when:** scene S3 shows a 16px ground gap around and between windows, a 1px `#2a2a30` ring around each, and `windows tile` results and contract tests pass with the new geometry.

---

## 4. Phase 3: what the bar says

Layout left to right: mark button (existing menu) · agent identity · 1px divider · window pills · flexible space · sealed chip · jobs chip · lease chip · clock.

The desktop thread reads state from root-window properties set by the server process (see `job_counts()` and `read_lease()`). Add new properties the same way, set by the server and watched with `PROPERTY_CHANGE` on the root:
- `_HOTLINE_AGENT` (UTF8 JSON): `{"name": "...", "task": "...", "last_action_ms": 0}`.
  - `name`: env `HOTLINE_COMPUTER_AGENT` if set, else the most recent non-`anonymous` `X-Computer-Holder` seen on `/mcp`, else `Agent`.
  - `task`: label of the most recently started job that is still running, else empty.
  - `last_action_ms`: wall clock of the last tool call that moves something (input, browser actions other than snapshots/text, windows, shell start). Update it in `tools::call`.
- `_HOTLINE_SEALED` (CARDINAL): number of stored secrets. Set it in `secrets::replace`.

**Agent identity:** 22px circle filled agent-soft with the name's initial in agent colour, 11.5 semibold; then the name 13 semibold `ink`; then the task 12.5 `muted`, ellipsized at 40 characters. No task: show nothing after the name.

**Window pills:** height 28, radius 8, padding 0 11, gap 8 between icon (16px app icon, existing `icon_of`) and title. Active: `raised` fill plus 1px inset `raised-edge`, text `ink`. Inactive: no fill, text `pill-ink`, hover `raised`. Keep `PILL_MAX`.

**Sealed chip:** a small padlock drawn as vectors in `paint.rs` (rounded rect body plus an arc shackle, 13px, `ink-2`) and `N sealed`. Hidden when N is 0.

**Jobs chip** (neutral chip: `chip` fill, 1px `chip-edge`):
- `no jobs` with a muted dot, then `· M done` in muted.
- `N running` in `ink` with an agent dot and a 3px agent-soft halo, then `· M done` in muted.
- `N failed` in fail colour with a fail dot, then `· M running` in muted, when there are fresh failures (existing rule: only failures nobody retried).

**Lease chip** (tinted, no border):
- `agent at work`: agent dot, agent text, agent-soft fill, when jobs are running or `last_action_ms` is within 10 s.
- `agent idle`: muted dot, neutral chip, otherwise.
- `you're driving`: you dot, you text, you-soft fill, when a person holds the lease (existing `Holding::Person`).
- Remove the old `agent in control` / `person in control` strings.

**Clock:** 13 `ink`, tabular figures, 8px right padding.

**Popups and menus** (`draw_popup`, `open_popup`): stay square X windows, but `surface` fill, 1px `rule` border, 6px padding, 32px rows, `raised` hover, 13px text. Menu row text: "the agent's jobs" instead of "the teammate's jobs" (`desktop.rs:1110`). Show "scroll for more" (`desktop.rs:1161`) only when rows actually overflow. Fix the comments at `desktop.rs:351` and `:1252` that say the menu keys are "A to D"; they're B, T, O, A.

`_HOTLINE_BAR_LAYOUT` gains `agent` and `sealed` rects. Update `README.md` "The desktop" section to describe the new bar.

**Done when:** scene S3's bar crop shows identity, pills, `1 failed · 2 running` in coral, `agent at work`, and the clock; scene S4 (driving) shows `you're driving` in amber.

---

## 5. Phase 4: observer and terminals

`assets/alacritty.toml`:
- `font`: Geist Mono, size 10.5, `offset = { x = 0, y = 3 }` for air between lines. Bold uses GeistMono-Medium.
- `padding = { x = 20, y = 18 }`.
- Colours: background `#111114`, foreground `#c9c9ce`; normal green `#86d69c`, blue `#8fb8ff`, yellow `#f0c27a`, red `#ff8f80`, black `#19191e`, white `#ececef`; bright black `#8e8e95`. Cursor stays painted in the background colour.
- Update the file's header comment ("teammate's jobs" to "agent's jobs").

`src/observer.rs`: job header lines use green `✓` for done, blue `●` for running, red `✗` for failed or cancelled; the label in bright white; the id, time and directory line in bright black. Keep the existing tests passing (they assert text, not colour); add one asserting the running marker.

`assets/bashrc`: prompt blue to `#8fb8ff` if it hardcodes the old blue.

**Done when:** scene S3's observer shows coloured markers in Geist Mono on `#111114`.

---

## 6. Phase 5: the viewer (`src/viewer.html`, `src/viewer.rs`)

Line numbers are against 7d75305. These are the UI items from `LAUNCH-PUNCHLIST.md` 11 to 21 plus the look.

**Look**
- Tokens and Geist from phase 1. Control bar: radius 14, `surface` at 92% opacity, 1px `raised-edge`, shadow `0 18px 40px rgba(0,0,0,.5)`, height 48.
- Watching: green live dot, "Watching {agent name}", muted "· N jobs · live NN ms", a quiet Files button, and **Take control** as a solid you-coloured button with `#0b0b0d` text.
- Driving: border at you colour 45%, "You're driving" in you colour, muted "· agent paused", Paste, **Give it back** on `#26262c`.
- Replace the text glyph icons `+ ↑ ×` (`:70`) with inline stroke SVGs (1.5 stroke, 16px), each with `aria-label`.
- Favicon: `assets/hotline-mark.svg` inlined as a data URI. Put the mark (16px, `ink`) at the start of the status pill.

**Agent cursor with intent**
- `src/xtest.rs` `Gesture` gains optional `intent: Option<String>`. Set it from the tool layer: `clicking`, `double-clicking`, `typing`, `scrolling`, `dragging`, plus a short target when one is cheap and safe: the AT-SPI element name or browser ref's accessible name, max 32 chars. Never typed text, never JS, never a command line. Run it through `secrets::redact` before broadcast. `viewer.rs` `pointer_message` passes it through; extend the existing test at `viewer.rs:990-1007`.
- Page: agent pointer filled agent colour with a 1.6px `#0b0b0d` outline; the click ring in agent colour (right-click stays amber). Next to the pointer, a pill (height 28, radius 14, agent fill, `#0b0b0d` text 12.5 medium) with a small initial circle and "{name} · {intent}". It fades 2 s after the last gesture.
- Agent name reaches the page in the existing `state` message: add `agent` to `json!({"t":"state", ...})` (`viewer.rs:382`) from `_HOTLINE_AGENT` or the same source.

**Connection states** (punch list 11)
- "Connecting…" until the socket has opened once (today `render()` overwrites it with "Reconnecting…", `:153`).
- Closed before ever opening: `fetch('/files?token=…')`. 401: show "This link is missing its key. Open it from Hotline, or add #<token> to the address." and stop retrying. `/health` failing: "The computer isn't running." Otherwise retry with backoff 1, 2, 4, 8, 15 s.
- Connected but no frame yet: centred mark and "Waiting for the screen…" instead of a 1×1 black canvas.
- Hide the status divider when there's nothing after it (`:18`).

**Liveness and latency** (punch list 19)
- Page sends `{"t":"ping","id":n}` every 5 s; server answers `{"t":"pong","id":n}` immediately (add a `Ping` variant to the input enum at `viewer.rs:387`; it must not count as input or touch the lease). Show "live NN ms" from the round trip. No pong for 10 s: amber dot and "stalled".

**Scaling** (punch list 12)
- `fit()` (`:166-170`) fits to `innerHeight - 72` so the control bar never covers the desktop while watching.
- A 1:1 toggle in the control bar sets the canvas CSS size to `canvas.width / devicePixelRatio` and lets the page scroll.

**Keyboard** (punch list 13)
- On macOS (`navigator.platform` / `userAgentData`), send Meta as Control, with a "Cmd as Ctrl" toggle in a small settings popover, default on.
- Fullscreen button: `requestFullscreen()` then `navigator.keyboard.lock()` where supported.
- Ctrl+Alt+Shift gives control back. Document it in the Give it back tooltip.

**Toasts** (punch list 15)
- Errors fade after 6 s or on the next input, click to dismiss, `role="alert"`. Delete the unused `sticky` (`:106`, `:141`).
- Toast `z-index` above the Files panel (`:45`, `:61`).

**Files panel** (punch list 14)
- Cancel `dragover`/`drop` on `window`; a file dragged anywhere opens the panel and lands there.
- Send `create_only=true` (`:334`); on 409 ask "Replace {name}?" and resend without it only on yes.
- Upload with `XMLHttpRequest` and `upload.onprogress`; a thin agent-coloured progress bar per row.
- Rows become `<button>`s with a download icon on hover/focus and the `modified` date the API already returns. Map 401 and other raw server errors to plain sentences (`:290`).

**Touch** (punch list 16)
- Canvas `touch-action: none`; `setPointerCapture` on down; treat `pointercancel` as button release.
- Under 600px wide or `(hover: none)`: hide the job sub-text and the paste hint; cap the bar at `calc(100vw - 24px)`.

**Accessibility** (punch list 18)
- Visible `:focus-visible` ring (2px agent colour, 2px offset) on every button and on the canvas. A small "keys → computer" indicator while the canvas has focus and the person is driving.
- Canvas `role="img"` with `aria-label="Agent's desktop"`; `aria-live="polite"` on status; `aria-expanded` on Files.
- `prefers-reduced-motion`: no pointer slide, no ring animation.

**Copy** (punch list 20)
- Paste error at `viewer.rs:456`: "The observer is read-only. Click the app you want to paste into."
- Failed counts in the viewer use fail colour, matching the bar.
- Move "Ctrl+Alt+V pastes from your computer" into the Paste button's tooltip and a one-time toast on first take-control.

**Dead code** (punch list 21)
- Remove the re-take-control-on-reconnect branch at `:179`.

**Done when:** viewer screenshots for S4 (watching, driving, bad token, phone width) match the descriptions above, and `cargo test` covers the ping/pong and intent passthrough.

---

## 7. Phase 6: Chromium chrome

- `assets/chromium-policy.json`: add `"BrowserThemeColor": "#17171b"`. Update its `_comment`.
- Do **not** add `--force-dark-mode` or any `WebContentsForceDark` feature: those change `prefers-color-scheme` and page rendering for sites under test.

**Done when:** scene S2 shows a dark tab strip and toolbar around a normal light page.

---

## 8. Stretch phases (only if 1 to 6 are committed)

### Phase 7: Activity window
- The server appends one line per completed tool call to `~/.hotline/activity.jsonl`: `ts`, `tool`, `action`, short `target` (URL host and path, window title, job label, element name), `ok`. Redact with `secrets::redact`. Never typed text, JS bodies, file contents or full command lines. Keep the newest 500 lines.
- `hotline-computer activity` renders it as a TUI the way `observe` renders jobs: time in bright black, a marker (✓ green, ● blue for the latest when the agent is at work, ✗ red), then the sentence ("Opened app.example.dev/login", "Typed into Email", "Filled Password from sealed secret fieldnote", "Clicked Sign in"). Footer line in bright black: "Display only, not an audit log."
- Alacritty class `hotlineactivity`. The WM places it in the right column below the observer, splitting the column 60/40 when both are open; the person's shell still takes the bottom third when open. Menu gets a row for it; pick an unused key and update `MENU_KEYS`.
- Be honest in the README: the file lives in the agent's home and the agent can edit it.

### Phase 8: sealed-fill badge
- After a `browser fill` with a `secret` succeeds, read the focused element's screen extents over AT-SPI (`Component.GetExtents` in screen coordinates). Show an override-redirect window just below the field for 4 s, drawn with `paint.rs`: `surface` fill, radius 8 (corners antialiased against what's beneath is not possible; draw it square with a 1px `rule` border), green padlock, "Typed from sealed secret {name}", green "· origin matched".
- It must not take input: add x11rb's `shape` feature and set an empty input region on the window.
- If extents aren't available, show nothing. Never put the badge in the page DOM.

---

## 9. Self-QA

Create `qa/0.11.0-look/recipes/look-qa.py`, adapted from `qa/0.4.0-acceptance-2026-09-14/recipes/mcp-client.py`: URL from `PORT` (default 8787), token from `.token`, holder `look-qa`, artifacts into `qa/0.11.0-look/`.

Create `qa/0.11.0-look/recipes/login.html`: a plain light sign-in page (white card on `#f5f5f2`, Email and Password fields, a dark Sign in button) for scenes to use. Serve it inside the computer with a shell job `python3 -m http.server 8000` in its folder, so its origin is `http://localhost:8000`.

**Scenes.** Capture each with `capture` in image mode at full resolution (`original` PNG). Name files exactly:
- **S1 `s1-empty.png`**: fresh desktop, no windows.
- **S2 `s2-browser.png`**: browser open on `http://localhost:8000/login.html`.
- **S3 `s3-busy.png`** and **`s3-bar.png`** (region `[0,0,1920,40]`): browser plus observer open; jobs: one labelled `pnpm test` that succeeds, one labelled `pnpm dev` running `sleep 600`, one labelled `lint` that exits 3. Then `windows tile`.
- **S4 viewer**: if the host has Playwright or a headless Chrome, screenshot `http://127.0.0.1:8787/#<token>` at 1440×900 as `s4-watching.png`, after clicking Take control as `s4-driving.png`, with no fragment as `s4-badtoken.png`, and at 390×844 as `s4-phone.png`. If not available, write "S4 not run" in the report.
- **S5 `s5-badge.png`** (phase 8 only): `PUT /secrets` with a test login bound to `http://localhost:8000`, `browser fill` the password by secret name, capture within 2 s.

**Checks the script asserts** (decode PNGs with Pillow on the host; install it if missing):
- S1 pixel at (960, 20) within ±3 of `#111114`; pixel at (960, 39) within ±3 of `#222227`.
- S3 pixel at (8, 540) within ±3 of `#0b0b0d` (gutter), and the pixel one left of the left window's edge within ±6 of `#2a2a30` (ring). Read window rects from `windows list`.
- WCAG contrast computed from section 1: `muted` on `bar`, `muted` on `surface`, `ink-2` on `chip` all ≥ 4.5.
- `fc-match sans-serif` and `fc-match monospace` inside the container name Geist / Geist Mono (or the fallback).
- `make contract` green.

At the very end, if time allows, run `make acceptance` once and record the result. Don't block the report on it.

---

## 10. The morning report

Write `qa/0.11.0-look/REPORT.md`, keep it short, and commit it with the screenshots:

1. One line: which phases landed, which were reverted, which weren't started.
2. A table: phase · status · commits · screenshots · what to look at.
3. Check results from section 9, pass or fail with the measured values.
4. Anything you decided that George should confirm (font fallback used, intent wording, geometry trade-offs).
5. Known issues, each with how to reproduce.
6. Exact commands to see it: `git checkout look/0.11 && make image && make run`, then the viewer URL with the token from `.token`.

Do not claim something works unless a check or a screenshot in this folder shows it.
