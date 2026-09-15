# Computer 0.4.0 continued release acceptance

Final candidate source: f1413fe. Local image: sha256:1166cc75291cb405e33c9505cbd55dd940addbdccaf4deba7ccfc27fcc48115e.
Toad integration: d9e0d7e, rebased onto upstream 0.11.1; make check passed and the isolated native QA app was rebuilt and installed.

## Completed evidence

- Native ARM64 and x86_64 CI both passed all 16 task scenarios and restart recovery for 9e020fa. Run: https://github.com/1broseidon/toad-computer/actions/runs/34925147396.
- Native build/screens took 605.094 s on ARM64 and 491.066 s on x86_64. Job acknowledgement p95 was 5.997 ms / 2.302 ms respectively.
- A real Codex-backed teammate in the installed Toad app read the running guide, completed Selenium and the multi-step wizard, verified and tested Ketch, built pinned Toad, and inspected Room, Settings, Computer, and Updates. Its dev build took 5m 37s; observer closure preserved the running job. See live-agent-9e020fa/REPORT.md and its full-resolution screenshots.
- Live-agent discovery exposed the root-owned scratch mount. f1413fe seeds /home/agent/src as the non-root agent; fresh volumes and existing empty root-owned volumes both passed without a privileged repair. The CI contract now mounts this actual scratch volume and writes into it.
- Missing browser fill text now fails without clearing a field. The pinned rust-tauri profile supplies prebuilt Tauri CLI 2.11.4. Three focused MCP/viewer contract runs passed after checked X11 resize delivery.

- The rebuilt Toad 0.11.1 production provisioner created and resumed final f1413fe successfully. Its adapter cloned Ketch into /home/agent/src without repair, activated prebuilt Tauri CLI 2.11.4, and matched the generic client guide exactly. Job acknowledgement p95 was 48.286 ms across 70 starts. Evidence: final-toad-adapter/.

## Measurements and limits

The live ACP agent measured no-op command round trips of 3.643, 3.773 and 7.245 s, compared with command durations of 15, 2 and 4 ms. This aggregate client/transport/queue overhead is separate from the backend acknowledgement distributions; its precise cause is not established here. Do not describe full agent round trips as sub-250 ms. Code inspection of Toad d9e0d7e confirms that ACP receives the Computer HTTP URL and bearer directly; the OAuth proxy is not in this path. The observation does not yet isolate client scheduling, transport, or approval overhead.

The final native CI screenshots include a GTK `gdk_window_thaw_toplevel_updates` diagnostic on both architectures; Room and Settings rendering, accessibility, native controls, and window operations still passed. This is retained as an application/runtime observation rather than hidden from the evidence.

The pinned Toad 0.11.0 app correctly reports no nested Docker/Podman runtime and its historical 0.3.0 image default. Its Linux Docker help mentions macOS products; that is a baseline application-copy issue. Browser save-password UI was dismissed during QA. Personal Recoil/Cymbal tools were absent from the container; no host fallback was used.

An earlier live run failed when a concurrent duplicate-store probe exhausted local disk. Its reports are retained under earlier-storage-failure. The probe was removed, regenerable caches were reclaimed, and the successful independent run above started fresh. The failed attempt is not counted as acceptance.

## Remaining release gates

Final f1413fe native CI passed all 16 scenarios and recovery on both architectures (run 34926255056); evidence is in native-ci-f1413fe/. Native build/screens took 530.494 s on ARM64 and 534.065 s on x86_64. Computer PR #2 merged as 3f4ec78, and annotated tag v0.4.0 points to that identical source tree. The release image workflow 34927810331 is building the tagged source; registry cold pulls and startup remain pending on empty native runners. No GitHub Release has been created yet.

Public upload of the separate Toad integration branch was rejected by automatic approval review because the earlier confirmation covered only the Computer repository. Explicit approval has been requested; that branch remains local. Official default rollout and BRO-26/BRO-35/BRO-37 remain open until their remaining criteria pass.


## Final release result

Computer v0.4.0 is published. Both native release suites, cold pulls, and all 16 local Toad-adapter scenarios on the official image passed. The final evidence is in ../0.4.0-release/README.md. BRO-26 is Done; the separate Toad upload/default rollout remains pending approval in BRO-25/BRO-35/BRO-37.


## Rollout completed — 2026-09-15

Toad PR #12 merged as 9b30bc6 with the exact tested d9e0d7e tree. Computer 0.4.0 is the default on main. BRO-25 through BRO-37 are all Done. Final evidence and measured limitations are in ../0.4.0-release/README.md.
