import json,subprocess,pathlib,time
root=pathlib.Path(__file__).resolve().parent
seed=pathlib.Path('/Volumes/Storage/george/toad-qa-cache/os-review-2026-09-15/seed')
env=json.loads((seed/'native-environment.json').read_text())['env']
args=['docker','run','--rm','--user','1000:1000','--cpus','2','--memory','2g','--cap-drop=ALL','--security-opt=no-new-privileges','-v','toad-nix-glibc:/nix:ro','-v',str(seed)+':/seed:ro','--entrypoint','/bin/sh','-e','HOME=/home/agent']
for k,v in env.items(): args+=['-e',k+'='+v]
script='''mkdir -p /tmp/os-review-ui; cp -R /seed/ui/. /tmp/os-review-ui/; cd /tmp/os-review-ui
bun install --frozen-lockfile
printf '\nAFTER REINSTALL\n'
bun run build
printf 'UNMODIFIED_EXIT=%s\n' "$?"
printf '\nFORCE CORRECT GLIBC BINDING\n'
NAPI_RS_NATIVE_LIBRARY_PATH=/tmp/os-review-ui/node_modules/@rolldown/binding-linux-arm64-gnu/rolldown-binding.linux-arm64-gnu.node bun run build
printf 'EXPLICIT_GNU_EXIT=%s\n' "$?"
'''
args += ['toad-computer:os-review-alpine-stack','-c',script]
p=subprocess.run(args,text=True,capture_output=True,timeout=90)
(root/'alpine-libc-repro.log').write_text(p.stdout+p.stderr)
print('exit',p.returncode)
print('\n'.join(line for line in p.stdout.splitlines() if 'EXIT=' in line or 'REINSTALL' in line or 'BINDING' in line))
