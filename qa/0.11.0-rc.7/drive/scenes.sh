#!/bin/zsh
# The whole QA walk on a fresh container, shots into /tmp/hc7/shots.
set -e
cd /tmp/hc7; export HC_URL=http://127.0.0.1:8798 HC_TOKEN=$(cat token)
rm -f shots/*.png
./setup.sh >/dev/null
./put-set.sh ask >/dev/null; node mcp.mjs browser '{"action":"reload"}' >/dev/null; sleep 2; SETTLE=200 ./shot.sh 01-new-passkey
sleep 10; ./put-set.sh ask ask >/dev/null; sleep 1.5; SETTLE=200 ./shot.sh 02-new-login
sleep 10; SETTLE=200 ./shot.sh 02b-after-arrival
node mcp.mjs browser '{"action":"text"}' | grep -q '\[e5\] \[button\] Sign in with a passkey'
node mcp.mjs browser '{"action":"click_ref","ref":"e5"}' >/dev/null; sleep 1.5; ./uses.sh > uses-passkey.json; SETTLE=200 ./shot.sh 03-passkey-waiting
./uses.sh approve; sleep 1.2; SETTLE=200 ./shot.sh 04-passkey-approved
sleep 7; node mcp.mjs browser '{"action":"click_ref","ref":"e5"}' >/dev/null; ./uses.sh deny; sleep 1.2; SETTLE=200 ./shot.sh 05-passkey-declined
sleep 7; node mcp.mjs browser '{"action":"fill","ref":"e2","secret":"GITHUB_LOGIN.username"}'
s=$(date +%s); node mcp.mjs browser '{"action":"fill","ref":"e3","secret":"GITHUB_LOGIN.password"}' > fill-waiting.txt || true; echo "fill answered in $(( $(date +%s) - s ))s: $(cat fill-waiting.txt)"
sleep 1; ./uses.sh > uses-login.json; SETTLE=200 ./shot.sh 06-fill-waiting
./uses.sh approve; sleep 0.5; node mcp.mjs browser '{"action":"fill","ref":"e3","secret":"GITHUB_LOGIN.password"}'; sleep 1; SETTLE=200 ./shot.sh 07-fill-approved
sleep 7; node mcp.mjs browser '{"action":"click_ref","ref":"e5"}' >/dev/null; ./uses.sh allow; sleep 1.2; SETTLE=200 ./shot.sh 08-passkey-allowed
sleep 6; node mcp.mjs browser '{"action":"click_ref","ref":"e5"}' >/dev/null; sleep 0.3; echo "held during allowance: $(./uses.sh)"; sleep 1.2; SETTLE=200 ./shot.sh 09-within-allowance
node mcp.mjs input '{"action":"click","x":901,"y":119}' >/dev/null; sleep 1; SETTLE=200 ./shot.sh 10-record-within
node mcp.mjs input '{"action":"click","x":901,"y":119}' >/dev/null; sleep 50
node mcp.mjs browser '{"action":"fill","ref":"e3","secret":"GITHUB_LOGIN.password"}' || true; ./uses.sh deny; sleep 1; SETTLE=200 ./shot.sh 11-fill-declined
node mcp.mjs browser '{"action":"fill","ref":"e3","secret":"GITHUB_LOGIN.password"}' > fill-denied.txt || true; echo "retry after deny: $(cat fill-denied.txt)"
sleep 7; ./put-set.sh ask auto >/dev/null; node mcp.mjs browser '{"action":"fill","ref":"e3","text":""}' >/dev/null; sleep 11
node mcp.mjs browser '{"action":"fill","ref":"e3","secret":"GITHUB_LOGIN.password"}'; echo "held for an automatic login: $(./uses.sh)"; sleep 1; SETTLE=200 ./shot.sh 12-auto-fill
node mcp.mjs input '{"action":"click","x":901,"y":119}' >/dev/null; sleep 1; SETTLE=200 ./shot.sh 13-auto-record
node mcp.mjs input '{"action":"click","x":901,"y":119}' >/dev/null
python3 - <<'PY' > /tmp/hc7/set.json
import json
c=json.load(open('/tmp/hc7/credential.json')); c['use']='ask'
print(json.dumps({'PROOF_PASSKEY_B':c,'GITHUB_LOGIN':{'kind':'login','sites':['http://localhost:8123'],'username':'rex@hotline.dev','password':'correct horse battery staple'}}))
PY
curl -s -o /dev/null -X PUT -H "Authorization: Bearer $HC_TOKEN" -H 'content-type: application/json' --data @/tmp/hc7/set.json $HC_URL/secrets; rm -f /tmp/hc7/set.json
sleep 11; node mcp.mjs browser '{"action":"click_ref","ref":"e5"}' >/dev/null; sleep 1; u=$(./uses.sh); echo "$u" > uses-lapse.json
exp=$(echo "$u" | python3 -c "import json,sys;print(json.load(sys.stdin)['uses'][0]['expiresAt'])"); now=$(python3 -c "import time;print(int(time.time()*1000))")
sleep $(( (exp-now)/1000 + 1 )); SETTLE=100 ./shot.sh 14-expired; echo "after lapse: $(./uses.sh) page: $(node mcp.mjs browser '{"action":"eval","js":"out.textContent"}')"
