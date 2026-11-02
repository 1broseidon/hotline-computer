---
title: Workspaces
description: Toolchains with Nix, kept per project.
cluster: Using it
section: workspaces
reviewed: 2026-11-02
---

**The image has a desktop, a browser and Nix, and no language toolchains: a
project names what it needs, and the computer prepares it in that project's
workspace.** The agent's `state guide` has the full manifest format; this is
how it works underneath.

## Preparing

Start a session with `state info` and `state guide`. The guide and its
SHA-256 come from the running binary, so every client gets instructions that
match its image. Then prepare the packages the repository needs:

```json
{"action":"prepare","packages":["nodejs","pnpm"],"workspace":"/home/agent/src/project"}
```

Or use the repository's own flake, including a named dev shell:

```json
{"action":"prepare","flake":".#dev","workspace":"/home/agent/src/project"}
```

Package names are Nixpkgs attribute paths. Without `packages` or `flake`, the
saved definition is reused, or a root `flake.nix` is found on first use.
There are no language or framework presets. `state catalog` lists the inputs
and the default Nixpkgs pin.

Preparation returns a managed job, or `ready:true` when the packages are
cached. Once it succeeds, jobs whose `cwd` is inside the workspace get the
exported environment, and a job's own `env` wins. A failed preparation keeps
the previous environment.

## Storage

The workspace owns `.hotline/environment-spec.json`, and `.hotline/nix/`
holds the generated flake and lock. A package definition keeps its Nixpkgs
revision across Computer upgrades. Shared caches live under
`~/.cache/hotline/environments`, keyed by recipe and architecture rather
than by Computer version.

The image keeps nothing of its own in the home: its terminal configuration,
bash rc and Chromium policy live under `/etc`. So a home on a volume carries
the prepared environments, their cache, the jobs and their output, the shell
history and the browser profile across a recreate, for an upgrade or a
restart.

## Flakes

A repository flake is evaluated again on each preparation, reusing Nix's
store. An existing lock is never updated silently; first use may create one.
Shell hooks run during preparation and their exported variables are kept.
Aliases, functions and hooks that must run for every command need an
explicit `nix develop --command …`. To change a package environment, edit its
generated flake and prepare with `"flake":".hotline/nix"`.

## Software

Nothing runs as root, nothing can elevate, and the image has no `apt` or
`sudo`. Software comes in this order: what the computer prepares, then Nix by
hand (`nix shell nixpkgs#<name>` for one tool), then what the image already
has. A `.deb` or a system-wide install belongs in an image build.

An AppImage runs without FUSE, which a container cannot offer: the image sets
`APPIMAGE_EXTRACT_AND_RUN=1` and carries every library on the AppImage
exclude list, so an AppImage built anywhere finds what it expects.

## Artifacts

`files download` takes a URL or a GitHub release asset, with an optional
SHA-256 check; an asset pattern must match exactly one file. `extract` needs
a new destination and refuses traversal, links, special files and archives
that expand past 1 GiB. `run` runs bash, sh and python3 scripts. All three
are managed jobs, with the same status, progress and cancellation as shell
commands.
