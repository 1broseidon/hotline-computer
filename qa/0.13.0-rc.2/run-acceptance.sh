#!/usr/bin/env bash
# The fresh-image gate (tests/run-image-acceptance.sh) on Apple container:
# the contract and split test binaries from the checks image, then
# tests/acceptance.py --suite full. Created as the desk creates a computer
# (4 CPUs, 16 GB, --cap-drop=ALL; split adds --user 0 + SETUID/SETGID).
# No volumes: Apple container hands a fresh volume over root-owned, so the
# home is the container's own layer. usage: run-acceptance.sh split|legacy IMAGE CHECKS OUT
set -uo pipefail
mode=${1:?split|legacy}; image=${2:?image}; checks=${3:?checks image}; out=${4:?output dir}
repo=/Users/george/Projects/worktrees/hotline-computer/secrets-rc2
venv=/Users/george/Projects/scratch/secrets-rc-2026-11-18/venv
name=hl-secrets-rc2-acc-$mode
mkdir -p "$out"; out=$(cd "$out" && pwd)
token=$(openssl rand -hex 24); printf '%s' "$token" > "$out/token"; chmod 600 "$out/token"
port=$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1])')
flags=(); [ "$mode" = split ] && flags=(--user 0 --cap-add CAP_SETUID --cap-add CAP_SETGID)
cleanup() {
  container logs "$name" > "$out/container.log" 2>&1 || true
  if [ "${KEEP:-0}" != 1 ]; then container stop "$name" >/dev/null 2>&1; container rm "$name" >/dev/null 2>&1; fi
  rm -f "$out/token"
}
trap cleanup EXIT INT TERM
echo "# containers before (none touched):"; container list --all
HOTLINE_COMPUTER_TOKEN=$token container run -d --name "$name" --cap-drop=ALL ${flags[@]+"${flags[@]}"} -c 4 -m 16g \
  -p "127.0.0.1:$port:8787" -e HOTLINE_COMPUTER_TOKEN "$image" >/dev/null || exit 2
for _ in $(seq 1 120); do curl -sf "http://127.0.0.1:$port/health" >/dev/null && break; sleep 1; done
curl -sS "http://127.0.0.1:$port/health" > "$out/health.json"; echo; cat "$out/health.json"; echo
curl -sS -H "Authorization: Bearer $token" "http://127.0.0.1:$port/capabilities?nonce=acc" > "$out/capabilities.json"; cat "$out/capabilities.json"; echo
ip=$(container inspect "$name" | python3 -c 'import json,sys; d=json.load(sys.stdin)[0]; print(d["status"]["networks"][0]["ipv4Address"].split("/")[0])')
echo "# $name ($mode) at 127.0.0.1:$port, $ip"
rc=0
echo "## contract"
container run --rm --name "$name-contract" -e HOTLINE_COMPUTER_TOKEN="$token" -e HOTLINE_COMPUTER_URL="http://$ip:8787" \
  --entrypoint /acceptance/contract "$checks" --nocapture > "$out/contract.log" 2>&1 || rc=1
grep -E "^test result|FAILED|panicked" "$out/contract.log" | head -20
echo "## split checks"
container run --rm --name "$name-split" -e HOTLINE_COMPUTER_TOKEN="$token" -e HOTLINE_COMPUTER_URL="http://$ip:8787" -e HOTLINE_COMPUTER_MODE="$mode" \
  --entrypoint /acceptance/split "$checks" --nocapture > "$out/split.log" 2>&1 || rc=1
grep -E "^test result|FAILED|panicked" "$out/split.log" | head -20
echo "## acceptance.py --suite full"
(cd "$repo" && "$venv/bin/python" tests/acceptance.py --url "http://127.0.0.1:$port" --token-file "$out/token" --output "$out" --suite full) || rc=1
exit $rc
