# 0.10.0-rc.1 local verification (2026-09-23)

Image `hotline-computer:0.10.0-rc.1` (f155c24bdae3, 1.36 GB), channel `candidate`,
revision 7572986 on branch `george/manifest-0.10`. Not published.

Run the way the desk runs a computer: home, src and /nix on volumes, `--memory 4g`,
every capability dropped. Driven over MCP as an agent would.

| Step | Result |
|---|---|
| `state info` | parallelism derived from cgroup: 4 build jobs (not 24), GOMAXPROCS 24 |
| `state manifest` on a fresh clone | example, shape, platforms and services offered |
| `state prepare` with a manifest (Hotline repo, `platform: webkit`) | 24 s; 48 paths from the image's base cache; create hook `bun install` ran by itself |
| `shell run build` | `cargo build -p hotline-app` in 415 s at 4 jobs, no OOM kill |
| `shell run app` (`make dev`, kind desktop) | Hotline window rendered and took clicks, **no hand-exported GL/GTK paths** |
| `shell run lint` (undeclared) | refused, names `app, build, fmt`, says to add it to the manifest |
| edit manifest, `shell run` | refused until `state prepare`; re-prepare was a cache hit, no rebuild |
| services workspace (postgres, redis, start hook) | `psql` → 42, `redis-cli` → PONG, env list/`$WORKSPACE` expansion correct |
| `kind: web` run | got `$PORT`, returned URL, page loaded in managed Chromium |
| container restart | services and start hook came back by themselves |
| `cargo test --test contract` against the RC | pass |

Screenshots: `hotline-app-run.png`, `hotline-app-interaction.png`, `web-run.png`.

Known gaps: the nixpkgs source tarball is still fetched from GitHub on first
prepare (the substituter covers store paths, not the flake input fetch); a
manifest `flake` cannot be combined with `platform` or `services` yet; the
Hotline app logs "could not read shell PATH" at startup (its own login-shell
probe, also present on 0.9.1).
