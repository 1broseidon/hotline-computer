# Toad Computer acceptance results

Outcome: requested browser, artifact, local scrape, build, and native screen checks passed on the attached development computer. This is not certification of an official stable release or nested container execution.

## Identity
- Computer: 0.4.0, development revision 9e020fa, Linux aarch64, Mesa software rendering.
- Running guide: 0.4.0 / 9e020fa. SHA-256 fc245d4838b134aca28e6eff9e1683d7aa744c3a3e2ec0b0cb93218677917f0b.
- Nixpkgs: ef34387ddd751e1ab8857adf4676492d32eb24ec; rust-tauri profile.
- Native Toad: 0.11.0 at f3c17b78d85b4cc235455862d9c9241579157b39.
- Rust 1.98.1; Cargo 1.98.0; Tauri CLI 2.11.4; Bun 1.4.2.
- Computer clock printed September 15 03:41 UTC; user task date was September 14. Synthetic form date explicitly remained 2026-09-14.

## Verified results
1. Selenium web form: synthetic text/password/textarea, dropdown, datalist, date, attachment, checkbox and radio submitted; target displayed “Form submitted” and “Received!”. Other controls retained their defaults.
2. Local wizard: Team / Toad QA / 2026-09-14 / Linux and macOS / attachment.txt (53 bytes). Back to Contact preserved contact and project values including attachment. Review displayed all values. Synthetic confirmation checked; submission displayed “Application QA-040 accepted” with matching data. Additional local simple form submitted successfully.
3. Ketch: artifact tools downloaded Linux ARM64 v0.16.2, verified SHA-256 61e0a7ef16350534c586d7e2755a490333334fc40bdeac01c092bf77bf030b8c against GitHub release API digest and checksums.txt, then extracted it. Binary reports ketch v0.16.2. Local HTTP scrape returned expected title, unique marker, and Linux/macOS list; assertions passed without API credentials.
4. Native Toad: frozen Bun install and UI typecheck passed. Documented make dev completed Rust compilation and launched target/debug/toad-desktop. Debug symbols disabled and build parallelism limited to 2 through environment only. TOAD_DATA_DIR verified in running process as /home/agent/qa/toad/.toad-dev. No tracked source changes. Room empty state, Settings General, Computer runtime help, and Updates screens inspected visually and through window-scoped accessibility. Updates displays Toad 0.11.0 and development-build disabling. Computer displays Automatic, Docker/Podman not installed, Apple container macOS only, and default image ghcr.io/1broseidon/toad-computer:0.3.0.
5. Observer closed/reopened while native build job 000001a0a3280067-d8c73acd remained running with PID 484 and unchanged start time. Browser forms and full-resolution screenshots worked during compilation. Final app/observer tiling succeeded.

## Timing
- Repository clone: 1.362 s process time.
- Prepared rust-tauri environment: 22.494 s, including Nix metadata preparation.
- Bun dependency install: 1.93 s; Vite ready: 218 ms.
- Cargo dev build: 5m 37s reported by Cargo, including index/dependency download and compilation; these phases were not individually timed.
- Ketch 8,059,545-byte download with checksum: 0.795 s managed job; extraction: 0.263 s.
- Ketch local scrape command: 128 ms.
- Three no-op shell samples: round trips 3643 / 3773 / 7245 ms, versus process durations 15 / 2 / 4 ms. Difference is aggregate tool/transport/queue overhead, not download or compile time.

## Friction and limits
- Guide example /home/agent/src/toad clone failed permission denied. /home/agent/qa/toad worked.
- Recoil and cymbal are absent inside this computer. No host fallback used; no source editing was needed.
- Chromium's save-password prompt lingered after synthetic Selenium submission; dismissed with native input and clean local-result screenshot saved.
- Native accessibility exposes useful controls but omits much paragraph text; screenshots verify that copy. Observer correctly reports accessibility unavailable rather than borrowing another window's tree.
- The Computer screen's Docker advice says “Install Docker Desktop or OrbStack” even on Linux; potentially misleading platform copy.
- No nested Docker/Podman runtime installed; no nested computer launch attempted. No credentials, external teammate calls, publication, installer packages, or full repository regression suite.
- Environment preparation adds untracked .toad/ metadata; tracked diff is empty.
- App remains open under its managed development job for inspection. Local fixture server was stopped after verification.

## Evidence
All files are in /home/agent/qa/live-agent-evidence.
- computer-guide.json: exact info and running guide.
- selenium-submitted.png; local-results-clean.png; wizard-submitted.png.
- build-observer-before-close.png; build-observer-closed.png; build-observer-reopened.png.
- ketch-release.json; ketch-checksums.txt; ketch-verification.txt; ketch-scrape.json; ketch-local-fixture.png.
- toad-build.log: complete retained build output (295141 bytes).
- toad-provenance.json; verified-results.json; tool-timing.json; native-computer-tree.json.
- toad-room.png; toad-settings.png; toad-computer.png; toad-computer-runtime-detail.png; toad-development-version.png; toad-built-with-observer.png.
Screenshots saved at full desktop resolution, 1920×1080. Fixture and installed Ketch are under /home/agent/qa/fixture and /home/agent/qa/ketch-v0.16.2.
