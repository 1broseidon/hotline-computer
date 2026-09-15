import json,os,pathlib,secrets,subprocess,time,urllib.request
ROOT=pathlib.Path(__file__).resolve().parents[2]
OUT=pathlib.Path(__file__).resolve().parent
WORK=pathlib.Path('/Volumes/Storage/george/toad-qa-cache/generic-nix-2026-09-15')
WORK.mkdir(parents=True,exist_ok=True)
def docker(*args):
    return subprocess.check_output(['docker',*args],text=True).strip()
token=secrets.token_hex(24)
(WORK/'token').write_text(token);(WORK/'token').chmod(0o600)
cid=docker('run','-d','--name','toad-computer-generic-nix-review-final','--cap-drop=ALL','--security-opt=no-new-privileges','--cpus=2','--pids-limit=1024','--memory=4g','--shm-size=1g','-p','127.0.0.1::8787','-e','TOAD_COMPUTER_TOKEN='+token,'-v','toad-nix-glibc:/nix','-v',str(WORK)+':/home/agent/workspace','toad-computer:0.5.0-dev')
(WORK/'container').write_text(cid)
port=docker('port',cid,'8787/tcp').rsplit(':',1)[1]
url='http://127.0.0.1:'+port
(WORK/'url').write_text(url)
(OUT/'container.json').write_text(json.dumps({'id':cid,'image':docker('inspect',cid,'--format','{{.Image}}'),'url':url},indent=2))
for _ in range(100):
    try: urllib.request.urlopen(url+'/health',timeout=.2);break
    except Exception: time.sleep(.1)
else: raise RuntimeError('desktop did not start')
print('Review desktop ready at '+url,flush=True)
env={**os.environ,'TOAD_COMPUTER_TOKEN':token,'UV_CACHE_DIR':'/Volumes/Storage/george/toad-qa-cache/uv'}
subprocess.run(['uv','run','--no-project','--with','Pillow==11.3.0','python',str(ROOT/'tests/generic_environment_smoke.py'),'--url',url,'--output',str(OUT)],env=env,check=True)
