#!/usr/bin/env bash
# Both native CI runners and local releases use this exact fresh-image gate.
set -euo pipefail
image=${1:?usage: run-image-acceptance.sh IMAGE CHECKS_IMAGE OUTPUT_DIRECTORY}
checks=${2:?checks image is required}
output=${3:?output directory is required}
mkdir -p "$output"
output=$(cd "$output" && pwd)
credentials=$(mktemp)
chmod 600 "$credentials"
printf 'TOAD_COMPUTER_TOKEN=%s\n' "$(openssl rand -hex 24)" > "$credentials"
container=''
cleanup() {
  if [ -n "$container" ]; then
    docker logs "$container" > "$output/container.log" 2>&1 || true
    if [ "${TOAD_ACCEPTANCE_KEEP:-0}" != 1 ]; then docker rm -f "$container" >/dev/null || true; fi
  fi
  rm -f "$credentials"
  if [ "${TOAD_ACCEPTANCE_KEEP:-0}" != 1 ]; then rm -f "$output/token"; fi
}
trap cleanup EXIT INT TERM
container=$(docker run -d --cap-drop=ALL --security-opt no-new-privileges --pids-limit 1024 --memory 4g --shm-size 1g -p "127.0.0.1:${TOAD_ACCEPTANCE_PORT:-}:8787" --env-file "$credentials" "$image")
printf '%s\n' "$container" > "$output/container-id.txt"
docker inspect --format '{{.Image}}' "$container" > "$output/image-id.txt"
port=$(docker port "$container" 8787/tcp | sed 's/.*://')
url="http://127.0.0.1:$port"
printf '%s\n' "$url" > "$output/url.txt"
cut -d= -f2- "$credentials" > "$output/token"
chmod 600 "$output/token"
for attempt in $(seq 1 100); do
  if curl --silent --fail "$url/health" > /dev/null; then break; fi
  sleep .1
done
curl --silent --show-error --fail "$url/health" > "$output/health.json"
printf 'Running MCP and viewer contract against %s\n' "$url"
docker run --rm --network "container:$container" --env-file "$credentials" -e TOAD_COMPUTER_URL=http://127.0.0.1:8787 --entrypoint /acceptance/contract "$checks" --nocapture 2>&1 | tee "$output/contract.log"
python3 tests/acceptance.py --url "$url" --token-file "$output/token" --output "$output" --suite full
