#!/bin/zsh
# uses.sh            — GET /secrets/uses
# uses.sh <answer>   — answer the oldest held use (approve|allow|deny)
export HC_URL=http://127.0.0.1:8798 HC_TOKEN=$(cat /tmp/hc7/token)
if [ -z "$1" ]; then curl -s -H "Authorization: Bearer $HC_TOKEN" $HC_URL/secrets/uses; echo; exit; fi
for i in $(seq 1 50); do u=$(curl -s -H "Authorization: Bearer $HC_TOKEN" $HC_URL/secrets/uses); echo "$u" | grep -q '"id"' && break; sleep 0.2; done
id=$(echo "$u" | python3 -c "import json,sys;print(json.load(sys.stdin)['uses'][0]['id'])")
curl -s -w " %{http_code}\n" -X POST -H "Authorization: Bearer $HC_TOKEN" -H 'content-type: application/json' -d "{\"id\":\"$id\",\"answer\":\"$1\"}" $HC_URL/secrets/uses/answer
