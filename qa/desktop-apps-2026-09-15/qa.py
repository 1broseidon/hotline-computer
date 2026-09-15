import importlib.util,json,pathlib,secrets,subprocess,time,urllib.request,sys
ROOT=pathlib.Path(__file__).resolve().parents[2]
OUT=pathlib.Path(__file__).resolve().parent
WORK=pathlib.Path('/Volumes/Storage/george/toad-qa-cache/desktop-apps-2026-09-15')
WORK.mkdir(parents=True,exist_ok=True)
spec=importlib.util.spec_from_file_location('acceptance',ROOT/'tests/acceptance.py')
a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a)
def docker(*args,check=True):
 p=subprocess.run(['docker',*args],capture_output=True,text=True)
 if check and p.returncode: raise RuntimeError(p.stderr)
 return p.stdout.strip()
def client():
 return a.Computer((WORK/'url').read_text().strip(),(WORK/'token').read_text().strip(),OUT)
def record(name,value): (OUT/name).write_text(json.dumps(value,indent=2))
if __name__=='__main__':
 token=secrets.token_hex(24);(WORK/'token').write_text(token);(WORK/'token').chmod(0o600)
 cid=docker('run','-d','--cap-drop=ALL','--security-opt=no-new-privileges','--cpus=2','--pids-limit=1024','--memory=4g','--shm-size=1g','-p','127.0.0.1::8787','-e','TOAD_COMPUTER_TOKEN='+token,'-v',str(WORK)+':/home/agent/workspace','ghcr.io/1broseidon/toad-computer:0.4.0')
 (WORK/'container').write_text(cid);record('container.json',{'id':cid,'image':docker('inspect',cid,'--format','{{.Image}}')})
 port=docker('port',cid,'8787/tcp').rsplit(':',1)[1];url='http://127.0.0.1:'+port;(WORK/'url').write_text(url)
 for _ in range(200):
  try: urllib.request.urlopen(url+'/health',timeout=.2);break
  except Exception:time.sleep(.05)
 c=client();record('info.json',c.call('state',{'action':'info'}))
 c.call('files',{'action':'put','path':'/home/agent/workspace/hello.txt','content':'Desktop app QA: original text\n'})
 job=c.call('shell',{'action':'start','command':'apt-get','args':['update'],'label':'Check agent package installation'})
 try:c.done(job,20)
 except Exception:record('agent-apt-attempt.json',c.call('shell',{'action':'read','job_id':job['id']}))
 metadata=json.load(urllib.request.urlopen('https://update.code.visualstudio.com/api/update/linux-deb-arm64/stable/latest',timeout=30));record('vscode-download.json',metadata)
 jobs={}
 jobs['vscode']=c.call('files',{'action':'download','url':metadata['url'],'path':'/home/agent/workspace/code.deb','sha256':metadata['sha256hash']})
 jobs['geany']=c.call('files',{'action':'download','url':'https://download.geany.org/geany-2.1.tar.gz','path':'/home/agent/workspace/geany-2.1.tar.gz'})
 record('download-jobs.json',jobs)
 print('Isolated desktop ready; downloads started. VS Code '+metadata['productVersion'],flush=True)
