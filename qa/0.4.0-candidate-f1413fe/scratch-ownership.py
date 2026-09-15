import subprocess,json
from pathlib import Path
r=Path('/tmp/toad-scratch-ownership');r.mkdir(exist_ok=True)
proof=[]
for legacy in [False,True]:
 volume=subprocess.check_output(['docker','volume','create','--label','toad.qa=scratch-ownership'],text=True).strip()
 base=['docker','run','--rm','--cap-drop=ALL','--security-opt','no-new-privileges','--mount','type=volume,source='+volume+',target=/home/agent/src','--entrypoint','sh']
 try:
  if legacy:
   old=subprocess.check_output(base+['toad-computer:040-candidate','-c','stat -c "%u %g %a" /home/agent/src'],text=True).strip()
   assert old.startswith('0 0 '),old
  result=subprocess.check_output(base+['toad-computer:040-scratch-probe','-c','set -e; test "$(id -u)" = 1000; mkdir -p /home/agent/src/repo; printf scratch-writable > /home/agent/src/repo/proof; test "$(cat /home/agent/src/repo/proof)" = scratch-writable; stat -c "%u %g %a" /home/agent/src'],text=True).strip()
  assert result.startswith('1000 1000 '),result
  proof.append({'legacy_empty_volume':legacy,'passed':True,'ownership':result})
 finally:subprocess.run(['docker','volume','rm',volume],check=True,stdout=subprocess.DEVNULL)
(r/'results.json').write_text(json.dumps(proof,indent=2));print(json.dumps(proof))
