import json,os,pathlib,statistics,subprocess,sys,time
ROOT=pathlib.Path('/home/agent/qa/terminal-benchmark-v2');ROOT.mkdir(exist_ok=True)
PYTHON=sys.executable

def atomic_write(path,text):
 temp=path.with_suffix(path.suffix+'.tmp')
 temp.write_text(text);temp.replace(path)

def child(run):
 p=ROOT/run
 atomic_write(p.with_suffix('.ready'),str(time.monotonic_ns()))
 print('Toad terminal comparison: command runs directly through the process API.',flush=True)
 while not p.with_suffix('.go').exists():time.sleep(.005)
 start=time.monotonic_ns()
 payload=''.join(f'QA output {n:05d}: '+('0123456789abcdef'*5)+'\n' for n in range(50000))
 sys.stdout.write(payload);sys.stdout.flush()
 atomic_write(p.with_suffix('.done'),json.dumps({'start_ns':start,'end_ns':time.monotonic_ns(),'bytes':len(payload)}))
 print('\nQA_OUTPUT_COMPLETE: 50,000 lines written.\n',flush=True)
 time.sleep(1)
 return

def memory(pid):
 values={}
 for line in pathlib.Path(f'/proc/{pid}/smaps_rollup').read_text().splitlines():
  parts=line.split()
  if len(parts)>1 and parts[0] in ('Rss:','Pss:','Private_Clean:','Private_Dirty:'):values[parts[0][:-1]]=int(parts[1])
 return values

def cpu(pid):
 s=pathlib.Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split()
 return (int(s[11])+int(s[12]))/os.sysconf('SC_CLK_TCK')

def wait_file(path,process,timeout=15):
 end=time.monotonic()+timeout
 while not path.exists():
  if process.poll() is not None:raise RuntimeError(f'terminal exited {process.returncode}')
  if time.monotonic()>end:raise TimeoutError(str(path))
  time.sleep(.005)

if len(sys.argv)>1 and sys.argv[1]=='child':child(sys.argv[2]);sys.exit()
results=[]
for i in range(6):
 for app in (['alacritty','ghostty'] if i%2==0 else ['ghostty','alacritty']):
  run=f'{app}-{i}';base=ROOT/run
  title=f'Toad QA benchmark: {app} trial {i}'
  if app=='alacritty':args=[app,'--config-file','/home/agent/qa/alacritty.toml','--title',title]
  else:args=[app,'--gtk-single-instance=false','--font-size=12','--title='+title]
  args+=['-e',PYTHON,__file__,'child',run]
  with base.with_suffix('.log').open('w') as log:
   started=time.monotonic_ns();p=subprocess.Popen(args,stdout=log,stderr=log)
   row={'terminal':app,'trial':i,'warmup':i==0,'pid':p.pid,'argv':args}
   try:
    wait_file(base.with_suffix('.ready'),p)
    row['startup_ms']=(int(base.with_suffix('.ready').read_text())-started)/1e6
    time.sleep(.4)
    row['idle_kib']=memory(p.pid);row['cpu_before']=cpu(p.pid)
    base.with_suffix('.go').touch();wait_file(base.with_suffix('.done'),p)
    output=json.loads(base.with_suffix('.done').read_text());row['write_ms']=(output['end_ns']-output['start_ns'])/1e6;row['bytes']=output['bytes']
    time.sleep(.35)
    row['loaded_kib']=memory(p.pid);row['cpu_output_seconds']=cpu(p.pid)-row['cpu_before']
    row['exit_code']=p.wait(timeout=15)
   except Exception as e:
    row['error']=str(e)
    if p.poll() is None:
     p.terminate()
     try:p.wait(timeout=3)
     except subprocess.TimeoutExpired:p.kill();p.wait()
   results.append(row)
   (ROOT/'results.json').write_text(json.dumps(results,indent=2))
   print(json.dumps(row),flush=True)
summary={}
for app in ['alacritty','ghostty']:
 rows=[r for r in results if r['terminal']==app and not r['warmup'] and 'error' not in r]
 if rows:summary[app]={'successful_trials':len(rows),'startup_ms_median':statistics.median(r['startup_ms'] for r in rows),'idle_rss_mib_median':statistics.median(r['idle_kib']['Rss']/1024 for r in rows),'idle_pss_mib_median':statistics.median(r['idle_kib']['Pss']/1024 for r in rows),'write_ms_median':statistics.median(r['write_ms'] for r in rows),'loaded_pss_mib_median':statistics.median(r['loaded_kib']['Pss']/1024 for r in rows),'cpu_output_seconds_median':statistics.median(r['cpu_output_seconds'] for r in rows)}
(ROOT/'summary.json').write_text(json.dumps(summary,indent=2));print(json.dumps(summary),flush=True)
