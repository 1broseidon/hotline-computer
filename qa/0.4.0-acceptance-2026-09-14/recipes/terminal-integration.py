import json,os,pathlib,subprocess,sys,time
ROOT=pathlib.Path('/home/agent/qa/terminal-integration');ROOT.mkdir(exist_ok=True)
PYTHON=sys.executable

def write(path,data):
 tmp=path.with_suffix(path.suffix+'.tmp');tmp.write_text(json.dumps(data,indent=2));tmp.replace(path)

def observer(label):
 write(ROOT/(label+'.ready'),{'pid':os.getpid(),'cwd':os.getcwd(),'env':os.environ.get('TOAD_QA_CONTEXT')})
 print('TOAD JOB OBSERVER | '+label+'\nCommands are executed by the job runner. This window displays its log.\n',flush=True)
 pos=0
 while True:
  with (ROOT/'job.log').open() as f:f.seek(pos);text=f.read();pos=f.tell()
  if text:sys.stdout.write(text);sys.stdout.flush()
  time.sleep(.05)

def producer():
 with (ROOT/'job.log').open('w',buffering=1) as f:
  f.write('$ python qa-workload.py\n[RUNNING] job qa-durability\n')
  for i in range(1800):
   if (ROOT/'finish-job').exists():
    f.write('[FAILED] Deliberate QA exit 42; computer and terminal remain healthy.\n')
    write(ROOT/'job-result.json',{'exit_code':42,'ticks':i});sys.exit(42)
   f.write(f'progress {i:04d}: job running independently of terminal windows\n');time.sleep(.1)
 sys.exit(3)

if len(sys.argv)>1:
 if sys.argv[1]=='observer':observer(sys.argv[2]);sys.exit()
 if sys.argv[1]=='producer':producer();sys.exit()

def wait_until(predicate,seconds=30):
 end=time.monotonic()+seconds
 while not predicate():
  if time.monotonic()>end:raise TimeoutError('QA stage timed out')
  time.sleep(.02)

processes=[];logs=[];result={'attachments':[]}
try:
 producer_p=subprocess.Popen([PYTHON,__file__,'producer']);processes.append(producer_p)
 wait_until(lambda:(ROOT/'job.log').exists())
 commands={
 'alacritty':['alacritty','--config-file','/home/agent/qa/alacritty.toml','--daemon','--socket',str(ROOT/'alacritty.sock')],
 'ghostty':['ghostty','--class=com.toad.QAGhostty','--gtk-single-instance=true','--initial-window=false','--quit-after-last-window-closed=false','--confirm-close-surface=false','--linux-cgroup=never','--font-size=12']}
 daemons={}
 for app,args in commands.items():
  log=(ROOT/(app+'.log')).open('w');logs.append(log)
  daemons[app]=subprocess.Popen(args,stdout=log,stderr=log);processes.append(daemons[app])
 wait_until(lambda:(ROOT/'alacritty.sock').exists());time.sleep(.5)
 result['daemon_pids']={a:p.pid for a,p in daemons.items()};result['producer_pid']=producer_p.pid
 def attach(app,phase):
  label=app+'-'+phase;marker=ROOT/(label+'.ready')
  run=['/usr/bin/env','TOAD_QA_CONTEXT=explicit-job-env',PYTHON,__file__,'observer',label]
  if app=='alacritty':cmd=['alacritty','msg','--socket',str(ROOT/'alacritty.sock'),'create-window','--working-directory',str(ROOT),'--title','Toad Job | Alacritty | '+phase,'-e']+run
  else:cmd=['ghostty','+new-window','--class=com.toad.QAGhostty','--working-directory='+str(ROOT),'--title=Toad Job | Ghostty | '+phase,'-e']+run
  start=time.monotonic();r=subprocess.run(cmd,capture_output=True,text=True,timeout=10)
  row={'terminal':app,'phase':phase,'ipc_exit':r.returncode,'stderr':r.stderr,'argv':cmd}
  if r.returncode==0:
   wait_until(marker.exists);row.update(json.loads(marker.read_text()));row['attach_ms']=(time.monotonic()-start)*1000
  result['attachments'].append(row);write(ROOT/'result.json',result)
 for app in daemons:attach(app,'initial')
 write(ROOT/'stage.json',{'stage':'ready-for-close','pids':result['daemon_pids']})
 wait_until(lambda:(ROOT/'reopen').exists(),180)
 result['alive_after_windows_closed']={a:p.poll() is None for a,p in daemons.items()};result['producer_alive_after_close']=producer_p.poll() is None
 for app in daemons:attach(app,'reopened')
 write(ROOT/'stage.json',{'stage':'reopened'})
 wait_until(lambda:(ROOT/'finish-job').exists(),120)
 producer_p.wait(timeout=10);result['job_exit']=producer_p.returncode
 result['alive_after_job_exit']={a:p.poll() is None for a,p in daemons.items()}
 write(ROOT/'result.json',result);write(ROOT/'stage.json',{'stage':'complete'})
 wait_until(lambda:(ROOT/'stop-observers').exists(),120)
except Exception as e:
 result['error']=repr(e);write(ROOT/'result.json',result);print(repr(e),flush=True)
finally:
 for p in reversed(processes):
  if p.poll() is None:p.terminate()
  try:p.wait(timeout=3)
  except subprocess.TimeoutExpired:p.kill();p.wait()
 for f in logs:f.close()
