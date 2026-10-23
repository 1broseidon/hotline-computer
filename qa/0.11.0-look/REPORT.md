# 0.11 look

**Phases 1–8 landed and verified; none reverted or left unstarted.** Branch `look/0.11`; version remains 0.10.3. Nothing pushed, published, tagged or merged to main. Apple container 1.5.0 supplied the runtime at George's request.

| Phase | Passed / commit | Screenshots | What to inspect |
| --- | --- | --- | --- |
| 1 Fonts/palette | `a563f4d` | [S1](s1-empty.png), [S4](s4-watching.png) | Geist, 40px bar, dark surfaces |
| 2 Separation | `9c31437` | [S3](s3-busy.png) | 16px gutters, 1px rings, subtle shadows |
| 3 Bar | `fc19d14` | [Bar](s3-bar.png), [driving](s4-driving.png) | Identity, jobs, sealed count, amber person control |
| 4 Terminals | `b0177d9` | [S3](s3-busy.png) | Geist Mono and colored job markers |
| 5 Viewer | `740b92c` | [watching](s4-watching.png), [driving](s4-driving.png), [bad key](s4-badtoken.png), [phone](s4-phone.png) | Connection states, controls and phone layout |
| 6 Chromium | `1ec8bfd` | [S2](s2-browser.png) | Dark chrome around a normal light page |
| 7 Activity | `dd3a60e` | [activity](s7-activity.png), [with shell](s7-with-shell.png) | 60/40 split and editable activity display |
| 8 Receipt | `889cf3e` | [S5](s5-badge.png) | Sealed-fill badge below the password field |

Prerequisite `1feacba` permits native checks on macOS while retaining the Linux capture path. `0514422` adds Apple runtime recipes/evidence and updates stale contract geometry assertions. Each phase has its own check log; runtime evidence for phases 1–6 was completed once Apple container became available.

## Checks

- Final **119 Rust tests**, fmt and clippy passed on **macOS and Linux**: `phase8-check.log`, `final-linux-checks.log`.
- Live **MCP/viewer contract passed** against the final Apple-container image: `contract.log`. All eight tool descriptors and required arguments are unchanged.
- **S1–S4 passed**, including real viewer screenshots: `scene-checks.json`. Bar `[17,17,20]`; bottom rule `[34,34,39]`; gutter `[10,10,11]`; window ring `[42,42,48]`, all within tolerance. `fc-match` returned **Geist / Geist Mono**; viewer computed Geist.
- Contrast: muted/bar **5.792:1**, muted/surface **5.493:1**, ink-2/chip **8.488:1**. All exceed 4.5.
- **14 browser fixture checks passed**, zero browser errors (`viewer-checks.json`): reconnects, latency/stall, key mapping, emergency give-back, touch release, scaling, uploads/overwrite confirmation, drops, downloads and errors. The separately labeled `viewer-fixture-*.png` images use synthetic frames.
- Activity: **500-entry limit**, payload exclusion, rejected-call retention, 60/40 geometry, shell placement and restoration passed (`phase7-scenes.json`). Unit checks cover redaction before truncation and refusing a log symlink.
- Receipt: **zero X11 input rectangles**, override-redirect, no focus theft or DOM injection, click-through to Sign in, expiry and rejected-origin behavior passed (`phase8-scenes.json`). Fill **94 ms**; screenshot **1.107 s** after reply; removal **4.062 s** after reply. Activity contained the secret name, not its value.
- `scope-checks.json` verifies unchanged browser launch/security code, secret storage/redaction, passkeys, logins, leases, README security text and tool schemas. Only the specified public font route, sealed count, display metadata and SHAPE feature were added around those boundaries.
- Optional full `make acceptance` **not run**: its launcher/recovery harness requires Docker. No claim of full release acceptance or amd64 coverage.

## Measurements

| Metric | Published 0.10.3 | Final look image |
| --- | ---: | ---: |
| Compressed arm64 image | 746,149,985 bytes | 748,936,728 bytes |
| Run to healthy, cached image | 1.603 s | 1.705 s |
| Empty desktop memory | 287 MiB after capture | 258.15 MiB just after boot |

Single local runs, not a performance conclusion. Memory samples have different timing; the baseline was published earlier and Debian packages were rebuilt today. Image growth is **2.79 MB**. `baseline-runtime.json`, `apple-runtime.json`, `final-empty-stats.txt` and `final-image.json` retain the measurements. [Before](before-empty.png) / [after](s1-empty.png). Paint performance and cold download time were not benchmarked.

## Decisions and limits

- Official [Geist v1.8.1](https://github.com/vercel/geist-font/releases/tag/v1.8.1) TTF/WOFF2 assets plus OFL, **no fallback**, 932 KiB on disk. Archive SHA-256 `70f42c1452fb6df12ea183e20a65999157354f91d11dac18b4c713e383808ec2`.
- Work area `[16,56,1888,1008]`. Observer `[1280,56,624,1008]`; with Activity and shell: observer height 393, activity 263, shell 320, with 16px gaps. Zero gutters opts out. Existing WM had no fullscreen support to preserve.
- Cursor intent uses static verbs; optional target names are omitted. Browser-ref gestures use the last XTEST pointer position because CDP does not move that pointer. Reproduce by leaving it elsewhere before `browser click_ref`. Native input coordinates are accurate.
- Activity uses **V**, leaving **A** for About. It records explicit job labels, URL host/path without credentials/query/fragment, window titles, and focused field names when AT-SPI answers within 200 ms. Other actions use generic sentences. Atomic replacement retains 500 records. The agent can edit this file; it is not an audit log.
- Login receipts say “origin matched” only after the existing origin check succeeds. Unbound variables omit that claim. Missing AT-SPI extents or insufficient space below the field means no badge. The fixture uses `FIELDNOTE`, as the existing API requires uppercase names and returns 204 on delivery.
- Original scene PNGs use `capture mode=png` plus `files get`; image blocks are capped at 1568px. Fullscreen/keyboard-lock availability depends on the viewer browser and was not exercised by the fixture checks.
- QA corrections before passing: ambiguous missing-key locator, old edge-to-edge contract assertions, one badge clippy warning, and the fixture's secret-name case/HTTP status. No product authentication behavior changed.

## Apple runtime

The dedicated QA instance runs as **uid/gid 1000**, drops all capabilities, and sets `PR_SET_NO_NEW_PRIVS` before boot. `/proc/1/status` verifies **NoNewPrivs=1**. Memory is 4 GiB; `/dev/shm` 1 GiB; no host mounts; only **127.0.0.1:18787** is published. See `apple-isolation.txt`.

Apple 1.5 lacks Docker's cgroup PID-limit flag. `RLIMIT_NPROC=1024` is a per-uid substitute, **not equivalent isolation**. Product Docker launch settings are unchanged. The test used separate containers; existing computers were left alone. [Apple runtime documentation](https://github.com/apple/container/blob/1.5.0/docs/runtime-configuration.md).

## Reproduce

```sh
cd ~/Projects/personal/hotline-computer
git checkout look/0.11
container build -c 4 -m 6g -t hotline-computer:look-0.11 .
# Remove only the dedicated QA instance if it already exists:
container stop hotline-look-011-qa
container delete hotline-look-011-qa
python3 qa/0.11.0-look/recipes/run-apple.py
# Host Python needs Pillow and Playwright, with its Chromium installed.
PORT=18787 python3 qa/0.11.0-look/recipes/look-qa.py
PORT=18787 python3 qa/0.11.0-look/recipes/stretch-qa.py
PORT=18787 python3 qa/0.11.0-look/recipes/badge-qa.py
printf 'http://127.0.0.1:18787/#%s\n' "$(cat .token)"
```

The QA instance `hotline-look-011-qa` is left running for review. The launch recipe refuses to replace an existing name. For a fresh replay, explicitly stop/delete that QA instance first. The token stays in `.token`, never in this report. S1 requires a fresh desktop. The scene recipe runs `make contract` at its end; a separate `PORT=18787 make contract` can also be run after the scenes.

With Docker, the original requested path remains `git checkout look/0.11 && make image && make run`, then `python3 qa/0.11.0-look/recipes/look-qa.py` and `http://127.0.0.1:8787/#<token from .token>`.
