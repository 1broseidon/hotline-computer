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
printf 'HOTLINE_COMPUTER_TOKEN=%s\n' "$(openssl rand -hex 24)" > "$credentials"
container=''
scratch=''
home=''
store=''
cleanup() {
  if [ -n "$container" ]; then
    docker logs "$container" > "$output/container.log" 2>&1 || true
    if [ "${HOTLINE_ACCEPTANCE_KEEP:-0}" != 1 ]; then docker rm -f "$container" >/dev/null || true; fi
  fi
  for volume in "$scratch" "$home" "$store"; do
    if [ -n "$volume" ] && [ "${HOTLINE_ACCEPTANCE_KEEP:-0}" != 1 ]; then docker volume rm "$volume" >/dev/null || true; fi
  done
  rm -rf "$output/venv"
  rm -f "$credentials"
  if [ "${HOTLINE_ACCEPTANCE_KEEP:-0}" != 1 ]; then rm -f "$output/token"; fi
}
trap cleanup EXIT INT TERM
# The desk gives a computer a scratch volume, a home volume and the shared
# store volume; the home and the store are what a recreated container keeps.
scratch=$(docker volume create)
home=$(docker volume create)
store=$(docker volume create)
printf '%s\n' "$scratch" > "$output/scratch-volume.txt"
printf '%s\n' "$home" > "$output/home-volume.txt"
printf '%s\n' "$store" > "$output/store-volume.txt"
container=$(docker run -d --cap-drop=ALL --security-opt no-new-privileges --pids-limit 1024 --memory 4g --shm-size 1g -p "127.0.0.1:${HOTLINE_ACCEPTANCE_PORT:-}:8787" --env-file "$credentials" --mount "type=volume,source=$home,target=/home/agent" --mount "type=volume,source=$scratch,target=/home/agent/src" --mount "type=volume,source=$store,target=/nix" "$image")
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
docker run --rm --network "container:$container" --env-file "$credentials" -e HOTLINE_COMPUTER_URL=http://127.0.0.1:8787 --entrypoint /acceptance/contract "$checks" --nocapture 2>&1 | tee "$output/contract.log"
python3 -m venv "$output/venv"
"$output/venv/bin/pip" install --disable-pip-version-check -r tests/requirements.txt
"$output/venv/bin/python" tests/acceptance.py --url "$url" --token-file "$output/token" --output "$output" --suite full
"$output/venv/bin/python" tests/recovery.py --container "$container" --url "$url" --token-file "$output/token" --output "$output"
