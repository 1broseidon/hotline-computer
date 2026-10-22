#!/bin/zsh
# Serves the sign-in page, makes a passkey through the arming as the desk would,
# and stores it with "use":"ask". Leaves the browser on the sign-in page.
cd /tmp/hc7; export HC_URL=http://127.0.0.1:8798 HC_TOKEN=$(cat token); A="Authorization: Bearer $HC_TOKEN"
node mcp.mjs files "$(cat put-login.json)" >/dev/null
node mcp.mjs shell '{"action":"start","command":"python3","args":["-m","http.server","8123","--bind","127.0.0.1","--directory","/home/agent/src/proof"],"label":"Serve the sign-in page"}' >/dev/null
for i in $(seq 1 40); do node mcp.mjs browser '{"action":"navigate","url":"http://localhost:8123/login.html"}' 2>/dev/null | grep -q navigated && break; sleep 0.5; done
curl -s -X PUT -H "$A" -H 'content-type: application/json' -d '{"rpId":"localhost"}' $HC_URL/passkeys/registration >/dev/null
node mcp.mjs browser '{"action":"eval","js":"window.making = make(); \"parked\""}' >/dev/null
for i in $(seq 1 30); do s=$(curl -s -H "$A" $HC_URL/passkeys/registration); echo "$s" | grep -q '"asked"' && break; sleep 0.3; done
id=$(echo "$s" | python3 -c "import json,sys;print(json.load(sys.stdin)['ask']['id'])")
curl -s -X POST -H "$A" -H 'content-type: application/json' -d "{\"id\":\"$id\",\"approved\":true}" $HC_URL/passkeys/registration/answer >/dev/null
for i in $(seq 1 20); do r=$(curl -s -H "$A" $HC_URL/passkeys/registration); echo "$r" | grep -q registered && break; sleep 0.3; done
echo "$r" | python3 -c "
import json,sys; c=json.load(sys.stdin)['credential']
json.dump(c, open('/tmp/hc7/credential.json','w'))"
chmod 600 /tmp/hc7/credential.json
curl -s -o /dev/null -X DELETE -H "$A" $HC_URL/passkeys/registration
node mcp.mjs browser '{"action":"eval","js":"window.making"}'
