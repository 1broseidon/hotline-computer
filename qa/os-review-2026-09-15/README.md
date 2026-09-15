# Debian versus Alpine: initial research and basic proof

2026-09-15. Scope was explicitly limited to initial research and basic flows, not another release acceptance campaign. Recommendation: keep Debian as the general-purpose Computer default. Alpine remains smaller and viable for browser/CLI work, but the comparable stack saves 17.3% unpacked and the initial candidate reproduces native workflow compatibility failures. This establishes a practical compatibility tradeoff, not a universal performance ranking.

## Compared images

| Image | Unpacked bytes | Decimal GB | Role |
| --- | ---: | ---: | --- |
| Released Alpine Computer 0.3.0 | 527,077,179 | 0.527 | Historical size only; lacks the new bundled stack |
| Alpine 3.22 with Computer 0.4.0 | 1,097,416,771 | 1.097 | Built for size reference; old community support window |
| Alpine 3.24.1 with Computer 0.4.0 | 1,116,999,124 | 1.117 | Current supported Alpine candidate used for basic proof |
| Released Debian Computer 0.4.0 | 1,351,279,781 | 1.351 | Control |

The current Alpine candidate saves 234,280,657 bytes, 17.34% versus Debian. Debian is 20.97% larger than this Alpine candidate. The early package-only Alpine measurement was 1,076,555,842 bytes before the Computer binary and final initialization.

The candidates compile unchanged 0.4.0 source as musl binaries and include distro-native Chromium, Xvfb, Mesa software graphics, Alacritty, Nix, GTK, WebKitGTK, AppIndicator, Bash, Git, Python and artifact utilities. Package versions differ by distribution: this compares their supported packaged stacks, not libc in isolation. Both current images use Chromium 152.0.7977.82; Alpine has Mesa 26.1.6 and WebKitGTK 2.48.7, Debian Mesa 25.0.7 and WebKitGTK 2.52.6.

`image-sizes.txt`, candidate Dockerfiles, build logs and `alpine-stack-packages.txt` retain exact inputs and identities. No production Dockerfile or release was changed.

Registry transfer sizes are a different measurement: existing ARM64 releases are 231,747,886 bytes for 0.3.0 and 508,774,691 bytes for 0.4.0. See `registry-sizes.json` and per-architecture manifests. The local Alpine candidate was not published, and its registry transfer/cold-pull performance was not measured. Do not compare unpacked Alpine bytes with compressed Debian bytes.

## Basic proof

| Check | Debian 0.4.0 | Current Alpine candidate |
| --- | --- | --- |
| Simple form and three-step wizard | Pass | Pass |
| Shell execution and Alacritty observer | Pass | Pass |
| Software OpenGL / llvmpipe | Pass | Pass |
| Verified Ketch download and local scrape | Pass | Pass |
| Cached Python, Node, Rust and Go environments | Pass | Pass |
| Pinned Toad frontend production build | Pass | Fail: native binding / libc mismatch |
| Same prebuilt Nix Toad binary: launch and basic screens | Pass | No native window within 30 seconds |

The frontend failure was reproduced directly and through Computer's managed shell. Its Rolldown loader reads `/usr/bin/ldd` before consulting the running Node process. Alpine therefore selects the musl binding even while Node is using Nix glibc; the error includes `libc.so: invalid ELF header`. Reinstalling with `bun install --frozen-lockfile` did not repair it. Forcing Rolldown's GNU binding via the generic `NAPI_RS_NATIVE_LIBRARY_PATH` advanced past that failure but broke another native consumer (Tailwind: `Y is not a constructor`). That global override is not a general environment solution. Raw output: `alpine-frontend.log`, `alpine-libc-repro.log`, and `compatibility/alpine/results.json`.

This does not show that Alpine cannot run Toad. The previous exploratory Alpine session did so with manual environment repairs. It shows that the current common catalog is insufficient on the initial Alpine candidate and would require additional mixed-libc integration work. The separate native-window timeout was not root-caused in this short review; do not attribute it to Rolldown or claim a permanent incompatibility.

Nix toolchains and the native executable were reused from the accepted QA run, with the existing Nix volume mounted read-only. New homes, UI copies and app data were isolated. The four-toolchain check proves warm activation and execution, not fresh Nix downloads or fresh Rust compilation. The frontend was rebuilt; the native Rust executable was not rebuilt. No existing desktops or their data were modified.

## Speed and efficiency observations

This was one basic pass per current image on ARM64 OrbStack, with the same 2-CPU, 4-GiB and 1-GiB shared-memory limits. The VM has 10 CPUs and approximately 6 GiB RAM; existing desktops remained running and some review checks overlapped. These are observational timings, not a controlled benchmark or a tail-latency claim.

| Observation | Debian | Alpine |
| --- | ---: | ---: |
| Already-downloaded image start to health | 0.233 s | 0.199 s |
| Simple browser form | 0.881 s | 1.026 s |
| Three-step wizard | 2.279 s | 2.312 s |
| Median of five shell acknowledgements | 5.44 ms | 3.96 ms |

Both were responsive. There is no evidence here that Debian is generally faster, nor a decisive Alpine runtime advantage. Raw cgroup samples are retained but are too limited and affected by measurement processes to support memory or idle-CPU claims. Initial benchmark harness attempts incorrectly parsed the plaintext health response as JSON; `benchmark-harness-error.json` preserves those harness failures and the corrected runs are in `benchmark-results.json`.

The package inventory shows the size is dominated by applications and graphics. Debian's declared installed sizes include Chromium plus common assets (~369 MiB), LLVM (~118 MiB), WebKitGTK (~95 MiB), Mesa Gallium (~33 MiB) and JavaScriptCore (~29 MiB). Package metadata is not identical to Docker's image byte accounting; do not sum it as an exact attribution of the image delta. The small Alpine base does not make this shared functional stack small.

## Maintenance evidence and decision

Alpine says main packages receive roughly two years of support, while community packages are supported until the next stable release. Chromium and Nix are community packages. Alpine 3.22 now lists main-only support; the current 3.24 branch restores community coverage. This means staying current with the browser/Nix stack requires regular base-branch upgrades, or owning a separate packaging/backport policy. Sources: [Alpine release policy](https://alpinelinux.org/releases/), [Chromium package](https://pkgs.alpinelinux.org/package/v3.24/community/aarch64/chromium), [Nix package](https://pkgs.alpinelinux.org/package/v3.24/community/aarch64/nix).

Debian Trixie has full support through August 9, 2028, followed by LTS with a reduced architecture set. Its stable security policy aims to preserve API/ABI compatibility when backporting fixes. That provides a longer base-maintenance window; it does not mean our frozen image updates itself or every desktop package has identical support guarantees. Sources: [Trixie lifecycle](https://www.debian.org/releases/trixie/), [security policy](https://www.debian.org/security/faq).

For the requested mix of browser automation, downloaded CLI tools, arbitrary repository setup and native app QA, avoiding recurring mixed-libc repairs and reducing base-upgrade churn is worth the measured 234 MB unpacked premium. Keep Debian, retain the Nix catalog, and treat image footprint as an explicit budget. Any future slimming should measure the complete usable stack and preserve these basic workflows; selectively removing libraries only to download them again through Nix can move rather than eliminate cost.

This is sufficient evidence for a default-base decision at the requested depth, not exhaustive proof across repositories, architectures or custom environments. A browser-only product could reasonably weight Alpine's size saving more heavily.

## Visual evidence

[Alpine browser wizard completed](benchmark/alpine-current-0/02c-browser-wizard-submitted.png), [Debian browser wizard completed](benchmark/debian-040-0/02c-browser-wizard-submitted.png), [Debian native Toad settings](compatibility/debian/08-toad-computer-settings.png).

All review containers were removed after capturing evidence. Candidate images and their build recipes remain local for inspection. No images were published and no migration was performed. Upstream research used ketch; an Alpine wiki page returned 403 through both ketch and the web fallback and was not used as evidence.

## Later cleanup

On September 15, George requested removal of obsolete Docker artifacts. The historical test containers and comparison images described above were removed. Reports, recipes, screenshots, and diagnostic results remain here; live inspection now requires rebuilding. See [the Linux handoff](../README.md) for the current candidate.
