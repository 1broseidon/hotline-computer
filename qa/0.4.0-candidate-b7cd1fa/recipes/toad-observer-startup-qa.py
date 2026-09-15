import sys,json,subprocess,time,secrets,urllib.request
from pathlib import Path
sys.path.insert(0,'/Users/george/Projects/personal/toad-computer/tests')
from acceptance import Computer
out=Path('/tmp/toad-candidate-b7cd1fa/observer-recovery');out.mkdir(exist_ok=True)
token=secrets.token_hex(24);envfile=out/'credentials';envfile.write_text('TOAD_COMPUTER_TOKEN='+token+'\n');envfile.chmod(0o600)
owned=None
try:
 owned=subprocess.check_output(['docker','run','-d','--cap-drop=ALL','--security-opt','no-new-privileges','--pids-limit','256','--memory','1g','--shm-size','128m','-p','127.0.0.1::8787','--env-file',str(envfile),'-e','TOAD_COMPUTER_SCREEN=640x480','toad-computer:040-candidate'],text=True).strip()
 port=subprocess.check_output(['docker','port',owned,'8787/tcp'],text=True).strip().rsplit(':',1)[1];url='http://127.0.0.1:'+port
 for _ in range(50):
  try:urllib.request.urlopen(url+'/health',timeout=1);break
  except OSError:time.sleep(.1)
 c=Computer(url,token,out)
 config='/home/agent/.config/alacritty/alacritty.toml'
 subprocess.run(['docker','exec','--user','0',owned,'chmod','a-x','/usr/bin/alacritty'],check=True)
 job=c.call('shell',{'action':'start','command':'true','label':'Observer startup failure fixture'})
 c.done(job)
 assert job.get('observer_error'),job
 failure=job['observer_error']
 subprocess.run(['docker','exec','--user','0',owned,'chmod','755','/usr/bin/alacritty'],check=True)
 recovered=c.call('shell',{'action':'start','command':'sleep','args':['10'],'label':'Automatic observer recovery'})
 assert not recovered.get('observer_error'),recovered
 windows=c.call('windows',{'action':'list'});terminal=next(w for w in windows if 'toadterminal' in w['class'].lower())
 assert terminal['bounds'][1]>=48,terminal
 c.screenshot('15-observer-recovered-640x480.png')
 assert c.call('shell',{'action':'status','job_id':recovered['id']})['state']=='running'
 c.call('shell',{'action':'cancel','job_id':recovered['id']})
 (out/'results.json').write_text(json.dumps({'passed':True,'failure_reported':failure,'automatic_retry_opened_window':True,'screen':[640,480],'work_area_preserved':True},indent=2))
 print('Observer failure and automatic retry passed')
finally:
 if owned:
  with (out/'container.log').open('w') as log:subprocess.run(['docker','logs',owned],stdout=log,stderr=subprocess.STDOUT)
  subprocess.run(['docker','rm','-f',owned],stdout=subprocess.DEVNULL)
 envfile.unlink(missing_ok=True)
