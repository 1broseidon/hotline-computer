# Computer QA evidence and Linux handoff

The current implementation is on `codex/generic-nix-environments`, introduced
by commit `caa553d`. It builds a local `0.5.0-dev` image. Nothing in this branch
has been published as an official release.

## Resume on Linux x86-64

Fetch and check out this branch. Build natively on the x86-64 machine; the
local Mac image digest below describes ARM64 and is not an AMD64 image.

```sh
git fetch origin
git switch --track origin/codex/generic-nix-environments

docker build --platform linux/amd64 --target checks \
  -t toad-computer:generic-nix-checks .
docker build --platform linux/amd64 \
  --build-arg TOAD_BUILD_CHANNEL=development \
  --build-arg TOAD_BUILD_REVISION="$(git rev-parse HEAD)" \
  -t toad-computer:0.5.0-dev .
```

The checks target runs formatting, unit tests, and Clippy. Its container
contract test needs a running desktop and is skipped during the image build.
For the same basic live proof used on the Mac, start one isolated desktop:

```sh
umask 077
proof_dir=$(mktemp -d)
openssl rand -hex 24 > "$proof_dir/token"
printf 'TOAD_COMPUTER_TOKEN=%s\n' "$(cat "$proof_dir/token")" > "$proof_dir/environment"
proof_id=$(docker run -d \
  --cap-drop=ALL --security-opt=no-new-privileges \
  --cpus=2 --pids-limit=1024 --memory=4g --shm-size=1g \
  -p 127.0.0.1::8787 --env-file "$proof_dir/environment" \
  toad-computer:0.5.0-dev)
proof_port=$(docker port "$proof_id" 8787/tcp | awk -F: '{print $NF}')
proof_url="http://127.0.0.1:$proof_port"
for attempt in $(seq 1 100); do
  if curl --fail --silent "$proof_url/health" > /dev/null; then break; fi
  sleep .1
done
curl --fail --silent --show-error "$proof_url/health"
python3 -m venv "$proof_dir/venv"
"$proof_dir/venv/bin/pip" install -r tests/requirements.txt
TOAD_COMPUTER_TOKEN="$(cat "$proof_dir/token")" \
  "$proof_dir/venv/bin/python" tests/generic_environment_smoke.py \
  --url "$proof_url" --output qa/linux-amd64
```

Keep the captured `proof_id` for inspection and shut down with
`docker stop "$proof_id"` when finished. To view locally, open
`$proof_url/#<contents of the token file>` in the browser. For a remote Linux
host, forward that loopback port over SSH; do not expose the Computer port
publicly. A fresh Linux store will download dependencies; the Mac's shared
Nix store is not transferred by Git.

This smoke exercises package lists, saved definitions, cache reuse, failed
replacement recovery, named repository flakes, exported shell hooks,
subdirectory inheritance, and visible Nix diagnostics. It intentionally
creates one failed job for a nonexistent package. Broader application QA and
release acceptance remain George's next step; no further campaign is
required merely to reproduce the basic proof.

## Evidence map

- [Generic Nix candidate](generic-nix-2026-09-15/README.md): the current
  implementation and ARM64 basic proof, including the final screenshot and
  build/check logs.
- [Cross-framework application proof](desktop-apps-2026-09-15/README.md):
  Electron/VS Code, Qt/FeatherPad, and source-built GTK/Geany on released
  Computer 0.4.0. This predates generic preparation.
- [Debian/Alpine comparison](os-review-2026-09-15/README.md): the bounded
  research and compatibility comparison that motivated keeping Debian.
- [Published 0.4.0 evidence](0.4.0-release/README.md): historical release proof.
- The acceptance and candidate directories retain earlier reports, recipes,
  screenshots and diagnostic results, including failed attempts. They use
  the APIs of their recorded versions; use current `tests/` for new runs.

Raw upstream page caches and downloaded tarballs stay local and are excluded
from Git. Reports link their primary sources and record artifact checksums.
Historical scripts with Mac-specific paths are retained as evidence, not as
portable launchers; the commands above are the Linux entry point.

## State at handoff

The implementation and evidence are committed; no release is cut. BRO-39
remains In Progress for operator acceptance and system-package customization.
BRO-38 tracks the Qt accessibility crash, and BRO-40 tracks Electron sandbox
launch behavior. Safe image updates preserving a user's home/customization
remain separate work; saved Nix definitions alone are not that migration.

The Mac review desktop is stopped, with its container and active Nix store
retained. Older test containers, unused images/build cache, and orphaned QA
volumes were removed at George's request. These reports preserve the test results, not backups of the removed
containers; historical live container IDs and viewer URLs are no longer usable.
Only the final review container remains on the Mac, stopped. Keep local Mac
runs to two containers where possible, with at most three in parallel.
