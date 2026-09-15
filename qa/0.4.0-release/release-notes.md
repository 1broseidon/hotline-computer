Toad Computer 0.4.0 supports complete repository-to-desktop workflows in a non-root Linux image: prepare an environment, install a verified tool, run a build, and inspect the resulting app while browser and viewer work continue.

- A glibc image with Nix, Mesa software graphics, GTK/WebKit runtime support, and writable persistent scratch storage.
- Cached Python, Go, Node, Rust, and Rust/Tauri environment profiles, including a prebuilt Tauri CLI.
- Managed shell jobs with retained output, direct stdin/PTY support, cancellation, restart recovery, and verified download/extract/run actions.
- A reopenable Alacritty observer and top bar for applications, tray icons, and job status.
- Browser form actions that verify retained values, window operations that verify geometry, and accessibility trees matched to the correct native window.
- Explicit host clipboard paste while the human viewer holds control.
- A bundled skill and catalog identified by the actual running version, source revision, and guide checksum.

Use `ghcr.io/1broseidon/toad-computer:0.4.0`. Existing computers retain their running version until recreated; `state info` and `state guide` identify the attached computer.

Acceptance covers native ARM64 and x86_64, browser forms, verified Ketch installation and scraping, multiple repository/environment fixtures, native Toad builds and screens, takeover, clipboard, job/observer recovery, and cold registry pulls. A real Toad-driven Codex session also completed the browser, CLI, and native-app workflows. Release image publication requires the published image configuration to match the accepted image.


Validation: [native ARM64/x86_64 release gates](https://github.com/1broseidon/toad-computer/actions/runs/34927810331) passed all 16 scenarios per architecture, recovery, image-identity checks, and cold pulls. The published image also passed all 16 scenarios through Toad's production adapter locally, plus hard-restart recovery.

Cold registry pulls: 17.50 s ARM64 / 16.28 s x86_64. Downloaded-image start-to-health: 0.287 s / 0.341 s. Compressed layers: 508,774,691 / 510,678,739 bytes. The combined image digest is `sha256:acd49c6019dc35445d79170fb9201d91461efe056a989c51a54e1f3fc844ac82`.

The local Toad adapter measured 47.5 ms job acknowledgement p95 and 115.2 ms maximum observer reopen. A separate live ACP session showed some 3.6–7.2 s tool round trips despite millisecond command execution; that client-side/transport/queue overhead remains a measured limitation, not a backend timing claim.

The companion Toad guide/default integration is merged in [Toad PR #12](https://github.com/1broseidon/toad/pull/12). New desktop builds use Computer 0.4.0 by default; existing Toad installations can select the explicit image above.
