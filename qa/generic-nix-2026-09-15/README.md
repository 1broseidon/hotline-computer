# Generic Nix preparation: local development candidate

Built from branch `codex/generic-nix-environments`; local image `toad-computer:0.5.0-dev`. No release publication or production image replacement.

## Changed behavior

- `state prepare` accepts project-selected `packages`, a local repository `flake` with optional named dev shell, or the saved definition when neither is supplied.
- Definitions and locks live with the workspace; package caches use recipe and architecture, without the Computer version. Existing pins survive dependency additions.
- Flakes execute their hooks during preparation and retain exported variables. Managed shell jobs inherit the environment by working directory.
- The bundled skill reads repository requirements before selecting dependencies. The fixed language/framework presets and base-image WebKit/AppIndicator packages are removed. The Toad-specific flake remains only a test fixture.

## Scope of validation

The normal Rust formatting, unit tests, and Clippy checks ran through the Docker `checks` stage. The network/container contract test is skipped there by design; the small `tests/generic_environment_smoke.py` run separately exercises the actual MCP preparation and shell handlers. Full release acceptance and cross-framework application QA are intentionally deferred to George's review.

Logs: [checks.log](checks.log), [image-build.log](image-build.log), and [smoke.log](smoke.log). The live smoke results, trace, and screenshot are alongside this report.

## Host incident

The initial build exhausted the host disk, and OrbStack stopped. Restarted OrbStack and all seven containers observed running before the incident. Data remains on disk; processes inside those desktops were interrupted. Moved the 554 MiB Cargo registry source cache to the external drive and left a symlink at its original path. Removed disposable Alpine comparison images, an unused 0.4 candidate image, and rebuildable build caches to recover space. No project directories or container data volumes were deleted.

## Basic smoke outcome

The initial smoke passed package execution, cache reuse, saved definitions, failed-replacement recovery, a named repository flake, subdirectory inheritance, and re-evaluation of a changed shell hook without changing its lock. Its observer screenshot exposed a logging defect: Tokio's `output()` captured Nix diagnostics instead of forwarding the configured streams. Preparation now uses `spawn().wait_with_output()` to honor stream inheritance; the same smoke adds assertions for Nix errors and hook output.

The skill revision uses a repository-first imperative before the examples (attention placement, established), and labels each example by its applicable input (reduces copying a framework-specific setup, a design bet). Existing browser/native input, job lifecycle, and takeover guidance is preserved. This assumes a client loads `state guide` from the attached Computer and uses its current tool schema.

## Final handoff

- Commit: `caa553d` on `codex/generic-nix-environments`.
- Image: `toad-computer:0.5.0-dev`, configuration digest `sha256:b21c506459cc21a4680af5a5798e91c0f7efd83424e2036b66c7567f312b5e95`.
- Final formatting, 32 unit tests, and Clippy passed. The corrected image passed the same live MCP smoke, including explicit assertions for Nix diagnostics and hook output.
- `summary.json`, `trace.jsonl`, and `generic-environments.png` contain the basic proof. The single failed job in the tray is the deliberately nonexistent package used to prove failure recovery.
- Final review container: `35986da576d0aade60a21f9dc313177e6be174cf34d11e7661a0a1f00d188baf`; viewer port 32775. Its URL and private token are in `/Volumes/Storage/george/toad-qa-cache/generic-nix-2026-09-15/`. The earlier smoke container was stopped with its data retained.
- BRO-39 remains In Progress for George's acceptance and the separate system-package customization work. No release published and no existing Computer image configuration changed.

## Linux handoff and shutdown

The final review container was stopped on September 15 at George's request. No containers are left running on the Mac. Older QA containers and unused image/build caches were removed; only the final review container and its Nix store were retained. The previously stopped first smoke container has since been removed. See [the Linux handoff](../README.md) for portable x86-64 build and basic proof commands.
