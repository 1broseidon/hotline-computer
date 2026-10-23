# 0.11 look: polish pass

The pass is commit 1c1ea2c on `look/0.11`. Later commits on the branch build on it: tile-aware panes, corner caps and a movable shell (385c9d5), and the harness's name and mark (e139fe3).

## What changed

- **Windows:** 10px corners cut with SHAPE, plus an antialiased ring in the wallpaper. Chromium keeps its own shape: overriding it stops its toolbar from repainting.
- **Sealed-fill receipt:** it sits beside the field, inside the browser window, and falls back to above the field. It is never placed over the next control. Its corners are drawn over a grab of the screen beneath, so they antialias against the real page.
- **Agent cursor:** browser `click_ref`, `fill`, `hover`, `check` and `select` move the real pointer onto the element first. The viewer's label reads "Tad · clicking Sign in" and hangs below the cursor tip.
- **Activity:** reads (captures, snapshots, listings) are no longer logged. Clicks name their element, and keys, saves and page actions read as sentences.
- **Observer:** inline scripts show as one line, and paths show as `~/…`.
- **Bar:** vector icons for the observer, the shell and Activity, and no more "· 0 running".
- **Empty desktop:** a quiet watermark plus "Tad's computer".
- **Chromium 141 fix:** the new-tab page's hidden `chrome://newtab-footer/` frame was being driven as the tab. `NewTabPageLocation` is now `about:blank`, and internal frames are filtered out of the tab list.

The pass was checked with `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` and 120 tests. The `shots/` were taken on Ubuntu with Playwright's Chromium 141 and Alacritty 0.15. The same scene on the Debian image (Chromium 155, Apple container, ARM64) matches them: corners, receipt and bar are pixel-identical up to text. Chromium 155 draws its own tab-search button at the left of the tab strip, and Debian's font draws a different check glyph in Activity.

## Drive it yourself

```sh
cd ~/Projects/personal/hotline-computer
container build -c 4 -m 6g -t hotline-computer:look-0.11 .
container stop hotline-look-011-qa; container delete hotline-look-011-qa
python3 qa/0.11.0-look/recipes/run-apple.py          # fresh token in .token
open "http://127.0.0.1:18787/#$(cat .token)"          # watch, then Take control
```

Optional: rebuild the demo scene and shoot it. Run these against a fresh instance, because the scene starts a server on port 8000 inside the computer.

```sh
python3 qa/0.11.0-look/polish/recipes/scene.py mine   # desktop shots
python3 qa/0.11.0-look/polish/recipes/viewer.py mine  # viewer shots (needs Playwright)
```

Shots land in `qa/0.11.0-look/polish/local/`. The scene stores a throwaway login, `FIELDNOTE`, for `http://localhost:8000` and puts a stand-in `pnpm` in `~/.local/bin` inside the computer. Both live only in that QA instance.

## Layout and identity checks

Each runs against a fresh instance from `run-apple.py` (port 18787, `.token`) unless `HC_URL` and `HC_TOKEN_FILE` say otherwise, and writes to `polish/local/`.

- `tile-check.py`: a browser reopened beside the observer takes the left pane, from the menu or by the agent; the work area comes back when the column closes; a maximized app fills the pane, not the screen; a plain installed app opens in the pane; `windows tile` keeps every app left.
- `caps-check.py`: an app stacked over the browser, and the browser over the app, then both expanded, shot for zooming in on the corners.
- `shell-check.py`: the shell's bar toggle moves it between its slot and the pane, `windows maximize` does the same, auto-tile leaves an expanded shell alone, and closing or reopening the observer reflows both. It reads the toggle's place from `_HOTLINE_BAR_LAYOUT` rather than guessing pixels.
- `harness-check.py`: connects as eleven harness client names, with no holder header, and shoots the bar through each one's own session.

Run each on a fresh instance: they assume the desktop starts empty. Real Claude Code 2.1.306 and Codex 0.163.0 were also connected to the image and named themselves `claude-code` (in each request's `_meta`) and `codex-mcp-client` (at the handshake); Cursor's CLI says `Cursor`.
