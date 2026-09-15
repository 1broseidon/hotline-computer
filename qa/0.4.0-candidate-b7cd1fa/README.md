# Toad Computer 0.4.0 local candidate acceptance

Local ARM64 candidate accepted for the tested workflows; cross-architecture release acceptance is pending.

Computer source: `b7cd1fa`, branch `codex/desktop-040-release`.
Toad integration source: `cc1a19f`, branch `codex/computer-040-integration`.
Image: `sha256:ee28283f4cfd4c958bbee3db5c3896d8e0c63b3061a9aae549fb38dcfe88e717`.
Build channel: `development`; embedded revision: `b7cd1fa`.
Host runtime: OrbStack. Desktop: 1920×1080, 4 GiB, 1024 PID limit; native tests use two build jobs.

The actual Toad provisioner created the desktop from an empty mounted Nix store, stopped it, and woke it. Both clients queried the running image's info and identical bundled skill/hash. The generic suite began with no prepared environments or native build cache. The Toad adapter repeated every workflow on the reused desktop. This verifies the real MCP integration; it is not an independent model-session evaluation.

## Results

- All 16 generic-client cases and 16 Toad-adapter cases passed.
- Browser fixtures include a three-step conditional wizard, upload, back/review, asynchronous submission, and the public Selenium form.
- Python, Go, Node, Rust and Rust/Tauri prepare/run passed, including concurrent prepare, cached reuse, external C headers, Click repository tests, and verified Ketch 0.16.2 installation/scraping.
- Pinned Toad `f3c17b78d85b4cc235455862d9c9241579157b39` built inside the desktop. Room, Settings and Computer screens passed accessibility and pixel checks. Browsing/capture continued during the build.
- Job churn, retained output, PTY input, missing commands, nonzero exits, timeouts, process-group cancellation and zombie checks passed.
- Alacritty daemon restart, per-job inspection and startup-failure recovery passed. A separate fault-injection desktop verified automatic retry and the 640×480 top bar.
- Real GTK controls, duplicate-window ambiguity, tray registration/menu activation/removal, close confirmation, minimum sizes and legacy Xterm titles passed.
- Live MCP/viewer contract passed in 11.87 seconds, covering view-only behavior, takeover races, clipboard ownership and reconnect.
- Hard container restart passed in 5.698 seconds, preserving partial output, marking the old job interrupted, and reusing the prepared environment.
- Computer formatting, Clippy and all 27 unit tests passed. Toad `make check` passed; the isolated macOS QA app was built and installed.

## Warm distributions

Values are median / p95 / maximum, with every sample retained in JSON.

| Client | Job acknowledgement (70 samples) | Observer reopen (12 samples) | Slowest cached activation (25 samples) |
|---|---|---|---|
| generic | 3.4 / 5.4 / 9.0 ms | 33.5 / 101.3 / 101.3 ms | 4.2 ms |
| toad | 44.2 / 47.1 / 47.6 ms | 88.8 / 229.8 / 229.8 ms | 91.6 ms |

## Findings fixed during QA

Nix now exposes its actual stderr when preparation fails. X11 status updates wait for server acknowledgement and preserve ordering. Native window tiling raises the selected pair and respects minimum sizes. Accessibility avoids ambiguous windows and supports native values/states. Observer startup failures remain separate from independent jobs. Cancelled or interrupted artifact jobs clean staging files. Toad recovers the running desktop's token and viewer after host-app restart.

The restart follow-up measured 3.356, 0.256 and 0.177 seconds. The slow sample spent 2.768 seconds in Docker start, 0.026 seconds locating the port, and 0.561 seconds awaiting health. Keep the original 5.698-second recovery sample; do not substitute the best warm run. The compressed Docker export is 491,813,583 bytes (469.0 MiB), compared with 1,351,255,885 bytes uncompressed.

The pinned upstream Toad app emits a GTK thaw/update warning during window changes; its visible controls and screens remain responsive. The terminal's error count includes deliberate negative tests and cancellations, not desktop failures.

## Remaining release work

- Native x86_64 acceptance, then both-architecture CI on the source intended for tagging.
- Registry compressed transfer/cold-pull measurements; `image-size.json` reports a compressed Docker export, not a cold registry pull.
- An independent Toad model-session evaluation of the bundled guide has not run; the current evidence exercises its production MCP adapter and shared preamble.
- Publish/tag and official-default rollout only after those checks. BRO-25, BRO-26, BRO-35 and BRO-37 stay open until their remaining acceptance is met.
- Public source push requires George's explicit confirmation: automatic approval review rejected uploading the broad source/test change to the public repository. No release/image has been published.

## Evidence

`generic/` and `toad/` retain commands/tool traces, result manifests, output, all timing samples and full-resolution screenshots. `host-native/` records the actual native Toad viewer clipboard checks. `observer-recovery/`, `a11y-edges/`, and `recovery/` contain targeted recovery evidence. Tokens and host credentials are excluded.
