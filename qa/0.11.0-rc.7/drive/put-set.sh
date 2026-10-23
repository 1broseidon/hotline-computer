#!/bin/zsh
# put-set.sh <passkey-use> [login-use]  — PUT /secrets with PROOF_PASSKEY and, if given, GITHUB_LOGIN for localhost:8123.
cd /tmp/hc7; export HC_URL=http://127.0.0.1:8798 HC_TOKEN=$(cat token)
python3 - "$@" <<'PY' > /tmp/hc7/set.json
import json,sys
c=json.load(open('/tmp/hc7/credential.json')); c['use']=sys.argv[1]
s={'PROOF_PASSKEY':c}
if len(sys.argv)>2:
  s['GITHUB_LOGIN']={'kind':'login','sites':['http://localhost:8123'],'username':'rex@hotline.dev','password':'correct horse battery staple','use':sys.argv[2]}
print(json.dumps(s))
PY
curl -s -o /dev/null -w "secrets %{http_code}\n" -X PUT -H "Authorization: Bearer $HC_TOKEN" -H 'content-type: application/json' --data @/tmp/hc7/set.json $HC_URL/secrets; rm -f /tmp/hc7/set.json
