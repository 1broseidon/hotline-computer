# Toad Computer 0.4.0 release acceptance

Computer 0.4.0 is published and accepted. The companion Toad integration is merged in PR #12 as 9b30bc60b208418634dd53cc645c941754e4f96b, with the exact tested d9e0d7e source tree built and installed locally. Main now defaults to Computer 0.4.0, but the public desktop binary is still 0.11.1 from before the merge. A new packaged desktop release is still required to distribute this integration.

- Release: https://github.com/1broseidon/toad-computer/releases/tag/v0.4.0
- Source: 3f4ec78f729256ab789d546dd585d904b63dd0c4, annotated tag v0.4.0; Computer PR #2 merged.
- Image: ghcr.io/1broseidon/toad-computer:0.4.0
- Combined registry digest: sha256:acd49c6019dc35445d79170fb9201d91461efe056a989c51a54e1f3fc844ac82
- ARM64 configuration: sha256:cc9a52a599ae77257b281a40cd7305d95144a763f81ea103fa1da4ea30968fa4
- x86_64 configuration: sha256:3df830f1f25fc09c5e7c46d0cb3dcb307456bb50e7b5ce0d9a2d2ff0cf276ca5
- Released guide SHA-256: 863b962bbf771c151e00cad4c43ffe5a8c4b2c2367d0b1cd1de49b7fafe3966e
- Toad integration: d9e0d7e on codex/computer-040-integration, rebased on Toad 0.11.1; make check passed.
- Installed host app: /Users/george/Applications/Toad Computer QA.app, isolated from the user's regular Toad data.

## Completed gates

Release CI https://github.com/1broseidon/toad-computer/actions/runs/34927810331 passed native builds/checks, all 16 scenarios on each architecture, MCP/viewer contracts, hard-restart recovery, published-versus-accepted image configuration checks, and cold registry pulls on empty native runners.

The official ARM64 image then passed all 16 scenarios through the rebuilt production Toad adapter on OrbStack, plus its actual create/stop/resume provisioner, scratch clone, guide matching, and hard-restart recovery. This run reused the existing Nix store and external-drive native target cache; it is explicitly a warm local integration run. Its native Toad build/screens took 92.529 seconds. No source/library repairs were required.

The suite includes simple and complex browser flows, verified Ketch installation/scraping, pinned Click repository tests, Python/Go/Node/Rust/Rust-Tauri profiles, download/script failures, managed jobs, cancellation/descendant cleanup, bounded output, native Toad screens, native accessibility/window identity, the application tray, observer restart/reopen and legacy Xterm titles. Existing takeover behavior and host clipboard ownership are covered by the live contract. Actual native host clipboard with Unicode was additionally verified against the same viewer code in candidate 9e020fa.

A real Codex-backed teammate in the installed Toad app independently completed browser, CLI and native Toad workflows using the running guide. That model session ran 9e020fa and exposed scratch ownership; the final image fixes it and the exact published image now passes the scratch mount contract and Toad-driven clone. See the candidate evidence link below; do not equate this earlier model session with the final scripted adapter run.

## Measurements

| Measurement | ARM64 native CI | x86_64 native CI |
|---|---:|---:|
| Compressed registry layers | 508,774,691 bytes | 510,678,739 bytes |
| Empty-daemon cold pull | 17.499 s | 16.278 s |
| Downloaded image start to health | 0.287 s | 0.341 s |
| Native Toad build/screens | 547.210 s | 594.544 s |
| Job acknowledgement p95 | 3.170 ms | 3.550 ms |
| Hard restart to health | 0.200 s | 0.241 s |

The local Toad adapter's 70 job starts measured median 44.93 ms, p95 47.51 ms and maximum 68.90 ms. Its 12 observer reopens measured median 89.91 ms and maximum 115.24 ms. All cached environment activations were below one second. Official-image local hard restart took 0.780 s, preserving output and prepared environments. The local image pull was measured on a nonempty Docker daemon: 16.715 s transfer and 1.078 s start-to-health; it is not labeled a cold pull.

## Observations and limits

- The independent ACP model session measured three no-op round trips of 3.643, 3.773 and 7.245 s despite 15, 2 and 4 ms execution. This remains aggregate client/transport/queue overhead; the precise cause is unproven. Toad hands the Computer HTTP endpoint directly to ACP, without its OAuth proxy. Do not claim all agent interactions take milliseconds.
- Pinned upstream Toad 0.11.0 emits a GTK thaw/update warning, but visible screens, controls and accessibility passed. Its Computer settings correctly report no nested container runtime and the historical 0.3.0 default; Linux help mentioning macOS products is a baseline copy issue. The host integration's default is 0.4.0.
- Tray failure totals include intentional negative tests and interrupted jobs. They are retained as truthful status rather than reset for screenshots.
- The earlier model attempt exhausted disk when an extra duplicate Nix-store probe ran concurrently. Its report is preserved separately; it does not count as acceptance. Final tests use one shared store and the external drive's build cache.
- The first local recovery invocation used a nested output path and failed before touching the container. The corrected invocation passed. Both logs are retained.

## Evidence and handoff

native-ci/ contains both release architectures, cold-pull/ contains empty-runner measurements, host-pull/ contains local image identity/startup, and toad-adapter/ contains the final 16-scenario traces/screens and recovery results. live-view/ contains the native app relaunched after recovery and left running for manual testing. Tokens and host credentials are excluded.

The earlier real-agent report and actual host clipboard evidence are retained in ../0.4.0-candidate-f1413fe/. The full task notes remain in Linear BRO-25 through BRO-37. BRO-26 through BRO-34 and BRO-36 are Done. BRO-25, BRO-35 and BRO-37 were reopened on 2026-09-15: their prior closure overstated distribution, because the packaged desktop release is still outstanding. The source upload and default change are merged: https://github.com/1broseidon/toad/pull/12. No automatic PR checks matched these files; the exact merged tree passed the recorded local make check, app build and official-image acceptance.

The earlier automatic approval rejection was resolved by George’s explicit approval on 2026-09-15. Branch codex/computer-040-integration was pushed and PR #12 merged. The review patch remains at /tmp/toad-computer-integration-review.patch; /tmp/toad-integration-pr.md records the capability review and acceptance evidence.


## Existing-computer update behavior

Existing running or stopped containers are reused without comparing their image to the configured version. Image pulls occur only when a container must be created and the selected image is absent locally. Selection is teammate override, room override, then the desktop pin. There is no background latest-image polling or hot replacement. Recreating an old computer uses the new selection; Docker/Podman workspace and scratch volumes persist, but disposable container-layer files do not. Stopping a session allows idle cleanup after 30 minutes (stop) and seven days (remove), while live sessions are excluded. The packaged desktop release and documented existing-computer transition remain rollout work.
