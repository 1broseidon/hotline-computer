# 0.11 look

Phases 1–5 landed on `look/0.11`; no phases reverted. Phases 7–8 will not start: no Docker.

| Phase | Status | Commit | Screenshots | What to inspect |
| --- | --- | --- | --- | --- |
| Native check prerequisite | Passed | `The native checks also build on macOS.` | not run: no Docker | Linux keeps memfd capture; other Unix hosts use the existing GetImage fallback. |

| 1 Fonts and palette | Passed native gate | `The desktop and viewer use Geist and the new palette.` | S1, S4: not run: no Docker | 40px bar, public embedded font allow-list, corrected light glyph coverage. |

| 2 Window separation | Passed native gate | `Windows share a guttered work area with painted separation.` | S3: not run: no Docker | Work area `[16,56,1888,1008]`; aligned WM/tool tiling; one-pixel rings and three-pass shadows. |

| 3 Bar metadata and layout | Passed native gate | `The bar names the agent and distinguishes work from person control.` | S3, S4: not run: no Docker | Identity, active pills, sealed count, colored job/lease chips, square hover menus. |

| 4 Observer and terminals | Passed native gate | `Terminals use Geist Mono and color each job by its state.` | S3: not run: no Docker | 10.5pt medium/regular faces, more line space, colored markers and periwinkle prompt. |

| 5 Viewer | Passed native and browser gates | `The viewer shows agent intent and handles reconnects and handoffs clearly.` | S4: not run: no Docker; separate `viewer-fixture-*.png` | Watching/driving, bad link and phone layout on a labeled fake frame. |

## Checks

- Phase 5: `make check` passed, 114 library tests, including ping without lease renewal and intent passthrough. All **14 browser fixture checks** passed on the third pass, with zero browser errors; see `viewer-checks.json` and `viewer-check.log`. Earlier runs caught a hidden touch focus indicator and case-sensitive macOS detection; both were fixed. Fixtures cover files/overwrite/download/drop, touch cancellation, Cmd mapping, emergency give-back, fitting, 1:1, six-second errors, liveness, reconnect, missing key and waiting/offline states. These do **not** verify X11 injection or real-container scenes.
- Phase 4: `make check` passed, 112 library tests; see `phase4-check.log`. Updated the old prompt-color expectation after the first run. Running-marker assertion and Alacritty TOML parsing passed.
- Phase 3: `make check` passed, 111 library tests; see `phase3-check.log`. Corrected test-module placement after the first clippy run. Tests cover name precedence, authenticated metadata updates, read/action distinction and ten-second expiry.
- Static QA passed: muted/bar **5.792:1**, muted/surface **5.493:1**, ink-2/chip **8.488:1**. See `static-checks.json`. Recipe syntax checked; live scenes are not run.
- Phase 2: `make check` passed, 107 library tests; see `phase2-check.log`. Removed one obsolete painter after the first clippy run. Tests cover 16px and zero gutters, small screens, minimum-size rejection, bounded/offset shadow alpha.
- Phase 1: `make check` passed, 104 library tests; see `phase1-check.log`. The first run found two stale wallpaper color expectations, corrected on the second run. Font traversal and unknown-name tests pass. Fontconfig and computed browser font checks: not run: no Docker.
- Native preflight: `make check` passed, including 102 library tests. The first sandboxed run could not bind local test sockets; rerunning with local sockets permitted passed. See `preflight-check.log`.
- Image, running-container contract, acceptance, and every screenshot: **not run: no Docker**. Neither Docker nor an attached Hotline computer is available. Native contract tests without a running server do not verify the container.
- Cold start, image size, runtime memory and live task success: not measured without the container.

## Decisions

- Vendored official Geist v1.8.1 TTF and WOFF2 files, with OFL license. Release archive SHA-256: `70f42c1452fb6df12ea183e20a65999157354f91d11dac18b4c713e383808ec2`. No fallback.
- Intent labels use only the five static verbs, redacted before broadcast. No accessible target names are included: obtaining and validating them would add reads to the action path. Browser CDP intents use the last XTEST pointer location; CDP actions do not themselves move the X pointer.
- Narrow/touch layouts put 1:1 and Fullscreen in Settings to keep primary controls visible. Cmd-as-Ctrl defaults on for macOS; no auto-retake on reconnect. Upload replacement requires an explicit confirmation after a create-only collision.
- Display metadata: holder names are observed after the existing bearer check; the same identity state feeds the bar and viewer. The only `/secrets` addition publishes the stored count after a successful replacement. No values or names go into that count. Existing redaction sanitizes identity/task labels. Successful mutating calls mark recent activity; read-only window lists and browser tab lists do not.
- Geometry: split the usable width after reserving the inner gap. Default observer is `[1280,56,624,1008]`; shell split is 672px / 16px / 320px. Tiny-screen outer gutters are clamped to leave usable pixels. Existing WM did not implement fullscreen; no new fullscreen behavior was added. Rings are painted after all shadows so adjacent shadows cannot dim them.
- A small portability prerequisite lets the specified check gate run on this macOS host. Linux capture behavior is unchanged; `openpty` gets the pointer types both Unix platforms accept.
- Full-resolution scene captures will use `capture mode=png` and fetch the original file. `mode=image` rescales large images; changing that tool contract is out of scope.

## Known limits

- Browser-ref cursor position: click a ref after leaving the X pointer elsewhere; its intent label uses that last pointer position. Native `input` gestures carry their actual coordinates.
- All visual and container checks need to be run with Docker before accepting the look. The report does not claim screenshot parity.

## Run it

```sh
cd ~/Projects/personal/hotline-computer
git checkout look/0.11 && make image && make run
make contract
python3 qa/0.11.0-look/recipes/look-qa.py
printf 'http://127.0.0.1:8787/#%s\n' "$(cat .token)"
```

Open the printed viewer URL. The bearer stays in the local `.token`, never this report.
