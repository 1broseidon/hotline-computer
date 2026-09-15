#!/usr/bin/env bash
set -euo pipefail
printf "\n=== APPLICATION BUILD: Toad from GitHub ===\nComputer service is healthy. Errors below belong to this build task.\n\n"
set -x
cd /home/agent/src/toad
printf 'Revision: '; git rev-parse HEAD
cd ui
bun install --frozen-lockfile
bun run build
cd ..
cargo build --locked -p toad-desktop --features tauri/custom-protocol
printf '\nBUILD_COMPLETE\n'
