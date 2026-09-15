import json,subprocess,pathlib,time
root=pathlib.Path(__file__).resolve().parent
seed=pathlib.Path('/Volumes/Storage/george/toad-qa-cache/os-review-2026-09-15/seed')
env=json.loads((seed/'native-environment.json').read_text())['env']
results=[]
for label,image in [('debian','ghcr.io/1broseidon/toad-computer:0.4.0'),('alpine','toad-computer:os-review-alpine-stack')]:
 args=['docker','run','--rm','--user','1000:1000','--cpus','2','--memory','2g','--cap-drop=ALL','--security-opt=no-new-privileges','-v','toad-nix-glibc:/nix:ro','-v',str(seed)+':/seed:ro','--entrypoint','/bin/sh','-e','HOME=/home/agent']
 for k,v in env.items(): args+=['-e',k+'='+v]
 args += [image,'-c','mkdir -p /tmp/os-review-ui; cp -R /seed/ui/. /tmp/os-review-ui/; cd /tmp/os-review-ui; node --version; bun --version; bun run build']
 start=time.monotonic();p=subprocess.run(args,text=True,capture_output=True,timeout=90)
 result={'label':label,'exit':p.returncode,'seconds':time.monotonic()-start,'stdout':p.stdout,'stderr':p.stderr}
 results.append(result);(root/(label+'-frontend.log')).write_text(p.stdout+p.stderr)
 print(json.dumps(result),flush=True)
(root/'stack-proof-results.json').write_text(json.dumps(results,indent=2))
