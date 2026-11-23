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

With a manifest, the job builds only the environment. Its create hooks run
after it, once each, then its services and start hooks start, and the
workspace is ready only when all of them are. `state manifest` answers with
`readiness`: the environment, each hook (`ran`, `done before`, `running`, or
`failed` with its job and the end of its output), each service (`up` or not)
and `ready`. A cached preparation waits up to 40 seconds for the hooks and
services and answers the same way. Before building, preparation checks that
each script a hook runs is in the workspace, and refuses with the hook's name
when one is not; it looks only inside the workspace and follows no link out
of it. A create hook that failed runs again on the next preparation, and one
that succeeded never does.

## Storage

The workspace owns `.hotline/environment-spec.json`, and `.hotline/nix/`
holds the generated flake and lock. A package definition keeps its Nixpkgs
revision across Computer upgrades. Shared caches live under
`~/.cache/hotline/environments`, keyed by recipe and architecture rather
than by Computer version.

The image's terminal configuration, base Bash configuration and Chromium policy
are installed under `/etc`. The interactive terminal also writes
`~/.hotline/bashrc` each time it opens. Mount the home directory as a volume to
retain prepared environments, caches, job records and output, shell history, and
the browser profile when the container is recreated.

## Flakes

A repository flake is evaluated again on each preparation, reusing Nix's
store. An existing lock is never updated silently; first use may create one.
Shell hooks run during preparation and their exported variables are kept.
Aliases, functions and hooks that must run for every command need an
explicit `nix develop --command …`. To change a package environment, edit its
generated flake and prepare with `"flake":".hotline/nix"`.

## Software

Jobs run as an unprivileged user and cannot elevate. The image includes neither
`apt` nor `sudo`. Use software in this order: packages prepared for the
workspace, manual Nix commands such as `nix shell nixpkgs#<name>`, then tools
already included in the image. Installing a `.deb` or other system-wide software
requires rebuilding the image.

The image sets `APPIMAGE_EXTRACT_AND_RUN=1`, so AppImages extract themselves and
run without FUSE. It also includes the libraries on the AppImage exclude list,
which AppImages expect the system to provide.

## Artifacts

`files download` takes a URL or a GitHub release asset, with an optional
SHA-256 check; an asset pattern must match exactly one file. `extract` needs
a new destination and refuses traversal, links, special files and archives
that expand past 1 GiB. `run` runs bash, sh and python3 scripts. All three
are managed jobs, with the same status, progress and cancellation as shell
commands.
