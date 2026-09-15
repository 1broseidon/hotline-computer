# Proposed 0.4.0 release plan

Based on the 2026-09-14 acceptance run. This is planned work, not implemented behavior. The release must complete representative tasks from a fresh image without host assistance or undocumented agent workarounds.

## Implementation order

1. **Make the image ready for real development.** Adopt a pinned minimal glibc base to remove the observed Alpine/Nix libc ambiguity. Include Bash, Git, download/archive utilities, CA certificates, fonts, a supported native Nix installation, and a correctly owned store. Supply and test software EGL/OpenGL through Mesa. Verify Alacritty, Chromium, and the Tauri/GTK runtime without manual library-path repair. Retain non-root execution and explicit process ownership. Measure image size, cold pull, and startup alongside task completion time.
2. **Give execution a durable owner.** Consolidate shell exec/launch behind one job implementation with argv, cwd, explicit environment, stable job ID, incremental retained output, status, wait, stdin/PTY write, and cancel. Reap children; surface spawn failure and exit status; retain partial output on timeout. Waiting for a job must not hold the desktop input lock. Bound transcript retention. Preserve job records across service restart and mark interrupted jobs honestly; container restart does not imply automatic process resumption.
3. **Make work visible.** Manage Alacritty as an observer of the job service through socket IPC. Show command, directory, label, progress, and exit status. Window closure or renderer failure must not kill a job. Extend the existing desktop shell with a top bar, Toad mark at top-left, running-window controls, terminal activity indicator, and application tray compatibility for apps such as Toad. Reopen jobs from that bar. Tile using actual work area and minimum window sizes, and verify requested focus/close/geometry changes.
4. **Make common environments one operation.** Ship a pinned catalog for Python, Go, Node, Rust, and Rust/Tauri. Entries include build tools, runtime libraries, executable paths, and smoke checks. Preparation returns an environment handle and visible progress. Reuse realized Nix environments without reevaluating/downloading them for each command; cache common dependencies without baking every possible toolchain into the base. Make download/extract/run structured operations that report resolved artifact, hash, destination, and job status. Test paths with spaces, argument quoting, environment isolation, and failed downloads.
5. **Make perception and actions trustworthy.** Fix native date fill, readonly/disabled handling, and read-back verification; support multi-select arrays and field states. Bind accessibility trees to the correct application/window identity. Fix legacy window-title fallback and settle after the final action before capture. Recheck takeover across UI actions, new job mutations, and PTY input; explicitly define how existing background jobs behave during human control.
6. **Ship knowledge with the running computer.** Bundle the skill and catalog with the release and expose their revisions and capabilities through MCP discovery. Toad Desktop uses the guide from the actual running computer, including reused containers and explicit development overrides. Cover browser forms, GitHub CLI installation, repository preparation, desktop launch, screen QA, and recovery. Validate with both a Toad-driven agent and a generic MCP agent.

Start with the base image and job service. The observer and catalog depend on those. Finish skill examples against the implemented tool contract, then run the release gate against the final image digest.

## Release gate

- Fresh ARM64 and x86_64 images complete simple/complex forms, verified CLI installation, and repository-to-desktop screen QA without host edits, hand-installed runtime libraries, or agent-authored DOM workarounds for the covered controls.
- Catalog smoke checks cover all five entries. End-to-end cases include Ketch and Toad, plus small pinned Python, Go, Node, and Rust fixtures so each runtime is exercised.
- Both cold-cache and warm-cache runs retain timings, commands, tool-call counts, output, and screenshots. Distinguish orchestration overhead from downloads and compilation.
- Proposed warm-path targets on reference hardware: job acknowledgement under 250 ms; cached environment activation under one second; terminal reopen under 300 ms. Record distributions rather than one best run; these are targets, not current guarantees.
- Exercise missing commands, failed installers, nonzero exits, timeouts, cancellation of descendants, viewer reconnect, observer process restart, human takeover, and container restart. No lost completed-job logs within the retention policy, false success, orphaned descendants, or accumulated zombies.
- Run a churn test with repeated jobs/window reopenings and a concurrent build while browsing. UI control and capture must remain responsive under output load.
- Run the repository's required checks, publish candidates for local validation, verify the release-matched skill through both clients, and only then tag/publish 0.4.0 and update Toad Desktop's official image default.

## Scope boundary

All six workstreams are part of the proposed 0.4.0 readiness bar. Defer multiple terminal backends, a general-purpose desktop environment, automatic support for arbitrary language stacks, GPU passthrough tuning, and transparent resumption of processes across container replacement. None is needed to make the tested task loop reliable and fast.
