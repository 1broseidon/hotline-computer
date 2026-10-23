#!/bin/zsh
# shot.sh <name> [x y w h] — full-res PNG from the container into /tmp/hc7/shots/<name>.png
export HC_URL=http://127.0.0.1:8798 HC_TOKEN=$(cat /tmp/hc7/token)
mkdir -p /tmp/hc7/shots
if [ -n "$2" ]; then region=",\"region\":[$2,$3,$4,$5]"; else region=""; fi
node /tmp/hc7/mcp.mjs capture "{\"mode\":\"png\",\"path\":\"/home/agent/shots-$1.png\",\"settle_ms\":${SETTLE:-300}$region}" >/dev/null
curl -s -o /tmp/hc7/shots/$1.png "$HC_URL/files/download?token=$HC_TOKEN&path=/home/agent/shots-$1.png"
file /tmp/hc7/shots/$1.png | cut -d: -f2
