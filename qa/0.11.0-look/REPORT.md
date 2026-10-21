# 0.11 look

Phase 1 landed on `look/0.11`; no phases reverted. Phases 7–8 will not start: no Docker.

| Phase | Status | Commit | Screenshots | What to inspect |
| --- | --- | --- | --- | --- |
| Native check prerequisite | Passed | `The native checks also build on macOS.` | not run: no Docker | Linux keeps memfd capture; other Unix hosts use the existing GetImage fallback. |

| 1 Fonts and palette | Passed native gate | `The desktop and viewer use Geist and the new palette.` | S1, S4: not run: no Docker | 40px bar, public embedded font allow-list, corrected light glyph coverage. |

## Checks

- Phase 1: `make check` passed, 104 library tests; see `phase1-check.log`. The first run found two stale wallpaper color expectations, corrected on the second run. Font traversal and unknown-name tests pass. Fontconfig and computed browser font checks: not run: no Docker.
- Native preflight: `make check` passed, including 102 library tests. The first sandboxed run could not bind local test sockets; rerunning with local sockets permitted passed. See `preflight-check.log`.
- Image, running-container contract, acceptance, and every screenshot: **not run: no Docker**. Neither Docker nor an attached Hotline computer is available. Native contract tests without a running server do not verify the container.
- Cold start, image size, runtime memory and live task success: not measured without the container.

## Decisions

- Vendored official Geist v1.8.1 TTF and WOFF2 files, with OFL license. Release archive SHA-256: `70f42c1452fb6df12ea183e20a65999157354f91d11dac18b4c713e383808ec2`. No fallback.
- A small portability prerequisite lets the specified check gate run on this macOS host. Linux capture behavior is unchanged; `openpty` gets the pointer types both Unix platforms accept.
- Full-resolution scene captures will use `capture mode=png` and fetch the original file. `mode=image` rescales large images; changing that tool contract is out of scope.

## Known limits

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
