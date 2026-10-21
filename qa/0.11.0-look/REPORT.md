# 0.11 look

Phases **1–6 landed** on `look/0.11`; none reverted. **7–8 not started yet**. Apple container is now available at the operator's request, so the stretch phases can proceed. Nothing pushed, published, tagged or merged; version remains 0.10.3.

| Phase | Status / commit | Screenshots | What to inspect |
| --- | --- | --- | --- |
| Native prerequisite | Passed · `1feacba` | verified with Apple container | macOS check support; Linux capture unchanged |
| 1 Fonts/palette | Passed · `a563f4d` | S1/S4: verified with Apple container | Geist, 40px bar, glyph edges, embedded font allow-list |
| 2 Window separation | Passed · `9c31437` | S3: verified with Apple container | 16px gutters, matching tiling, rings, shadows |
| 3 Bar | Passed · `fc19d14` | S3/S4: verified with Apple container | Agent identity/task, sealed count, jobs, amber person control |
| 4 Terminals | Passed · `b0177d9` | S3: verified with Apple container | Geist Mono, line spacing, colored job markers |
| 5 Viewer | Passed · `740b92c` | S4: verified with Apple container | Real viewer screenshots plus browser fixtures |
| 6 Chromium | Passed · `1ec8bfd` | S2: verified with Apple container | Dark chrome and a normal light page |
| 7–8 Stretch | Not started | S5: not run yet | Activity window and sealed-fill badge deferred |

## Evidence

- `make check` passed per phase. Library test counts: preflight **102**, then **104 / 107 / 111 / 112 / 114 / 114**. Logs: `preflight-check.log`, `phase1-check.log` through `phase6-check.log`. Native contract tests without a running server do not verify the container.
- All **14 browser fixture checks passed**, zero browser errors: `viewer-checks.json`. Covers connection states, no auto-retake, latency/stall recovery, Cmd mapping, emergency give-back, touch cancellation, scaling, errors, uploads with explicit overwrite confirmation, global drop and downloads. Rust tests cover ping without lease renewal and intent passthrough.
- Contrast passed: muted/bar **5.792:1**, muted/surface **5.493:1**, ink-2/chip **8.488:1**, all above 4.5. See `static-checks.json`.
- `scope-checks.json`: tool schemas/required args, versions, browser launch, leases, passkeys, logins, stored-secret/redaction code and README security wording preserved. Bearer gate only gains the specified public font prefix; unknown/traversal font names are refused.
- Apple container 1.5.0 built the image and the Linux checks stage: **114 tests, fmt and clippy passed**. The complete S1–S4 recipe and live `make contract` passed (`scene-checks.json`, `contract.log`). S1 bar/rule pixels were `[17,17,20]` / `[34,34,39]`; S3 gutter/ring were `[10,10,11]` / `[42,42,48]`. `fc-match` returned Geist / Geist Mono.
- Published 0.10.3 versus this image: compressed arm64 image **746,149,985 → 748,829,279 bytes**; run-to-health **1.603 → 1.563 s** (single warm-cache runs, not a performance conclusion). Empty desktop measured **258.13 MiB** just after boot; baseline **287 MiB** after capture, so memory samples are not directly comparable. See runtime JSON and stats files.
- Two QA corrections: scope the missing-key locator to the placeholder; update stale contract geometry assertions to `[16,56,1248,1008]` and `[1280,56,624,1008]`. The full recipe then passed. Optional full acceptance remains unrun.

These are **labeled fake-screen browser fixtures, not S4/container evidence**: [watching](viewer-fixture-watching.png), [driving](viewer-fixture-driving.png), [bad key](viewer-fixture-badtoken.png), [phone](viewer-fixture-phone.png). Computed Geist font and fitted viewport were verified in these fixtures.

## Decisions for review

- Official [Geist v1.8.1](https://github.com/vercel/geist-font/releases/tag/v1.8.1) static TTF/WOFF2 files plus OFL; no fallback. Assets occupy 932 KiB on disk. Archive SHA-256: `70f42c1452fb6df12ea183e20a65999157354f91d11dac18b4c713e383808ec2`.
- Added a small Unix portability prerequisite to run the native gate here. Local socket tests required sandbox escalation; no checks were disabled.
- Work area is `[16,56,1888,1008]` at 1920×1080. Observer is `[1280,56,624,1008]`; shell split is 672px / 16px / 320px. Zero gutters opt out; oversized outer gutters clamp on small screens. All rings paint after shadows. Existing WM had no fullscreen support to preserve.
- Identity is display metadata observed after bearer authentication, redacted with the existing redactor. `/secrets` only adds count publication after successful replacement. No authority changes.
- Cursor intents use only the five static verbs, redacted before broadcast. Optional target names are omitted to avoid extra action-path reads and accidental disclosure. On narrow/touch screens, 1:1 and Fullscreen remain available in Settings.
- Original scene PNGs use `capture mode=png` plus `files get`: `mode=image` resizes to 1568px. The MCP contract stays unchanged.
- Early checks exposed stale color assertions and two clippy issues; corrected before phase commits. Browser checks passed on the third pass after fixing a hidden touch focus indicator and case-sensitive `macOS` detection.

## Known limits

- Browser-ref intent position uses the last XTEST pointer position. Reproduce by leaving that pointer elsewhere, then calling `browser click_ref`; CDP does not move the X pointer. Native `input` gestures use actual coordinates.
- Container fonts, window placement, X11 input and dark Chromium chrome are verified by the real scenes/contract. Paint performance has not been benchmarked. Fullscreen/keyboard-lock support depends on the host browser and was not exercised by the fixtures.

## Apple runtime decision

The operator asked to use Apple container. The QA recipe uses a separate uid-1000 instance, drops all capabilities, applies `PR_SET_NO_NEW_PRIVS` before boot, and verifies `NoNewPrivs=1`. Memory is 4 GiB and `/dev/shm` 1 GiB; no host mounts; only 127.0.0.1:18787 published. Apple 1.5 has no cgroup PID-limit flag: `RLIMIT_NPROC=1024` is a per-uid substitute, not equivalent protection. Product Docker launch settings stay unchanged. Existing computers were left alone. [Apple runtime configuration](https://github.com/apple/container/blob/1.5.0/docs/runtime-configuration.md).

## See it

```sh
cd ~/Projects/personal/hotline-computer
git checkout look/0.11 && make image && make run
make contract
# In a host Python environment with Pillow and Playwright installed:
python3 qa/0.11.0-look/recipes/look-qa.py
printf 'http://127.0.0.1:8787/#%s\n' "$(cat .token)"
```

Open the printed viewer URL. The token stays in `.token`, never this report. Recipes, logs and fixture images are committed alongside it.

On this Mac, use `container build -c 4 -m 6g -t hotline-computer:look-0.11 .`, then `python3 qa/0.11.0-look/recipes/run-apple.py`. Run `PORT=18787 make contract` and `PORT=18787 python3 qa/0.11.0-look/recipes/look-qa.py`; open `http://127.0.0.1:18787/#<token from .token>`. The recipe refuses to replace an existing QA instance.
