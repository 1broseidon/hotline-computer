# Cross-framework desktop application proof

2026-09-15, ARM64 OrbStack. This is a short install/build/UI check beyond Toad itself, not an exhaustive release gate. None of these workflows used Rust, Tauri, or Nix.

| Application | Stack | Installation | Basic result |
| --- | --- | --- | --- |
| VS Code 1.137.0 | Electron | Microsoft's official ARM64 `.deb` | Opened, exposed accessibility, accepted Unicode paste, saved the exact text. Required `--no-sandbox` in the current container configuration. |
| FeatherPad 1.6.1 | C++ / Qt | Debian `.deb` installed through apt | Opened, pasted Unicode and saved successfully. Accessibility capture then reproducibly crashed it with SIGSEGV. Not a clean accessibility pass. |
| Geany 2.1 | C/C++ / GTK3 / Scintilla | Official source tarball, `configure`, `make`, user-local installation | Compiled and installed, opened, exposed accessibility, pasted Unicode and saved the exact text. Required adding missing gettext. |

All saves were performed through Computer's `input` tool and checked against the resulting file through `files get`. The tested strings contain café, an em dash, a frog emoji and a newline. Screenshots were inspected visually.

## Installation boundary

The starting image was the official Computer 0.4.0 ARM64 configuration `sha256:cc9a52a599ae77257b281a40cd7305d95144a763f81ea103fa1da4ea30968fa4`. The normal UID-1000 agent cannot run system apt operations. A host-invoked root process inside the same container also failed because apt's UID/group transitions conflict with dropped capabilities. These failures are recorded in `agent-apt-attempt.json` and `apt-update.log`.

To keep this a compatibility proof, a disposable derived image installed the VS Code package, FeatherPad, build-essential, libgtk-3-dev and pkg-config. `Dockerfile.apps` is the exact recipe; `image-build.log` records dependency resolution and package installation. The derived image is `toad-computer:desktop-apps-proof`, configuration `sha256:6a8be24141a20bec5d516b1d24a59bacc6e6eca5f87c6d3c2d266ef3eb3624a5`. Building/installing the dependencies took 136.93 seconds. Apps and source compilation then ran as the normal agent, with all capabilities dropped, no-new-privileges, 2 CPUs, 4 GiB memory and 1 GiB shared memory. No Nix store was mounted.

This is proof with explicitly installed dependencies, not proof that the stock image already supplies all build headers or that the agent can perform unattended system package installation today. An agent-facing package/build environment workflow is still needed.

## Findings

### Electron

VS Code's standard sandbox launch fails immediately in a direct executable probe: namespace creation is denied and the process exits with signal 5. `vscode-sandbox-probe.json` records the exact output. `--no-sandbox` makes the tested application run, with the outer container restrictions retained; it disables Electron's own sandbox and must not be presented as equivalent isolation.

The first-run welcome dialog appeared after the window initially mapped and intercepted the first save. Dismissing that observed dialog and repeating Save succeeded. `vscode-save-retry.json` proves the persisted text. This demonstrates why a mapped window alone is not a reliable app-readiness condition. No account sign-in was performed.

### Qt accessibility

The first FeatherPad attempt crashed with signal 11 during the general capture/UI sequence. On a second fresh launch, waiting for the window and using only PNG capture allowed editing, Unicode paste and save to succeed. `qt-probe-after.json` records the saved file and still-running process. A subsequent default capture, which queries accessibility, changed that same job from running to failed/signal 11; see `qt-accessibility-probe.json`. Container cgroup counters reported no OOM events.

The exact offending AT-SPI request and whether the remedy belongs in Qt, FeatherPad or Computer are not established. This is an observed interoperability failure, not a claim that every Qt application fails. It was not concealed by marking the full application check passed.

### C/C++ source build

Geany compiled with GCC/G++, GNU Make and GTK3 development headers. The first configure/build/install job took about 75.6 seconds and failed only when installing the desktop launcher: `msgfmt` was absent, configure substituted `:`, and `geany.desktop` was never generated. The compiled executable and other installed assets already existed.

The correction used user-local apt metadata, downloaded/extracted Debian's gettext package, and invoked Make with a scoped library path and explicit MSGFMT. This required no additional privileged operation. `gettext-result.json` and `geany-install-final.json` retain the commands and successful result. The installed Geany reported version 2.1, GTK 3.24.49 and GLib 2.84.4. Its native window appeared in approximately 0.225 seconds; this single warm observation is not a benchmark.

A useful generic C/C++ GUI environment needs build systems, pkg-config, GUI headers and localization tools; a Rust/Tauri catalog entry is not an adequate product interface for this workflow.

## Follow-up tracking

All are in the Toad project with `toad-desktop`, related to BRO-37:

- [BRO-38: Qt accessibility crash](https://linear.app/1broseidon/issue/BRO-38/prevent-accessibility-capture-from-crashing-featherpads-qt-application)
- [BRO-39: Desktop packages and C/C++ GUI setup](https://linear.app/1broseidon/issue/BRO-39/provide-a-reproducible-desktop-package-and-cc-gui-setup-path)
- [BRO-40: Electron launch/isolation behavior](https://linear.app/1broseidon/issue/BRO-40/define-and-verify-electron-app-launch-behavior-under-computer)

## Evidence

- `vscode-saved.png`: Electron editor after a verified save.
- `featherpad-probe-after.png`: Qt editor after a verified save, before the accessibility crash.
- `geany-saved.png`: source-built GTK editor after a verified save.
- `app-results.json` preserves initial failed/incomplete attempts; it is superseded for final save results by `vscode-save-retry.json`, `qt-probe-after.json` and `geany-ui-result.json`.
- `trace.jsonl`: actual Computer MCP operations, including failure evidence.
- `packages.json`: installed package versions and downloaded artifact checksums. VS Code's download was checked against the SHA-256 returned by Microsoft's update metadata. Geany's source was fetched over HTTPS from its official download site; the recorded checksum is an audit fingerprint, not an independently verified signature.

Primary installation/build sources: [VS Code Linux installation](https://code.visualstudio.com/docs/setup/linux), [Geany release downloads](https://www.geany.org/download/releases/), [Geany build instructions](https://github.com/geany/geany/blob/master/README).

The isolated QA computer retains VS Code and Geany for inspection. Its identifier is in `app-container.json`; private connection files and workspace are under `/Volumes/Storage/george/toad-qa-cache/desktop-apps-2026-09-15`. The viewer was requested in Codex. User desktops were not replaced. No production source or release was changed. Only the completed Alpine review's identified, regenerable 1.049-GB Rust compilation cache was removed to keep disk space available.

## Later cleanup

On September 15, George requested removal of obsolete Docker artifacts. The historical test containers and comparison images described above were removed. Reports, recipes, screenshots, and diagnostic results remain here; live inspection now requires rebuilding. See [the Linux handoff](../README.md) for the current candidate.
