from qa import *

def launch(c,name,command,args,match,timeout=15):
 job=c.call('shell',{'action':'start','command':command,'args':args,'label':name+' desktop QA','cwd':'/home/agent/workspace'})
 deadline=time.monotonic()+timeout
 while time.monotonic()<deadline:
  windows=c.call('windows',{'action':'list'})
  wins=[w for w in windows if match in w['class'].lower()]
  if wins:return job,wins[0]
  status=c.call('shell',{'action':'status','job_id':job['id']})
  if status['state'] not in ['running','exited']:break
  time.sleep(.2)
 result=c.call('shell',{'action':'read','job_id':job['id']})
 record(name+'-launch-failure.json',result)
 return job,None

c=client();results=[]
settings={'workbench.startupEditor':'none','security.workspace.trust.enabled':False,'telemetry.telemetryLevel':'off','update.mode':'none','extensions.autoUpdate':False,'workbench.tips.enabled':False}
c.call('files',{'action':'put','path':'/home/agent/workspace/code-data/User/settings.json','content':json.dumps(settings)})
apps=[('vscode','/usr/bin/code',['--new-window','--wait','--disable-extensions','--disable-workspace-trust','--force-renderer-accessibility','--user-data-dir=/home/agent/workspace/code-data','--extensions-dir=/home/agent/workspace/code-extensions'],'code'),('featherpad','/usr/bin/featherpad',[],'featherpad')]
for name,command,args,match in apps:
 row={'name':name};start=time.monotonic();path='/home/agent/workspace/'+name+'-qa.txt'
 c.call('files',{'action':'put','path':path,'content':name+' original QA text\n'})
 try:
  job,window=launch(c,name,command,args+[path],match)
  if not window and name=='vscode':
   c.call('shell',{'action':'cancel','job_id':job['id']})
   row['needs_no_sandbox']=True
   job,window=launch(c,name+'-no-sandbox',command,args+['--no-sandbox',path],match)
  assert window, 'Application did not open a window'
  row['launch_seconds']=time.monotonic()-start;row['job']=job;row['window']=window
  c.call('windows',{'action':'maximize','window_id':window['id']});c.call('windows',{'action':'focus','window_id':window['id']})
  time.sleep(2)
  (OUT/(name+'-tree-before.txt')).write_text(str(c.call('capture',{})))
  c.screenshot(name+'-before.png')
  if name=='vscode':c.call('input',{'action':'key','combo':'ctrl+1','settle_ms':200})
  c.call('input',{'action':'key','combo':'ctrl+a','settle_ms':100})
  text=name+' desktop QA passed — café 🐸\nEdited and saved through the desktop UI.\n'
  c.call('input',{'action':'paste','text':text,'settle_ms':200})
  c.call('input',{'action':'key','combo':'ctrl+s','settle_ms':500})
  saved=c.call('files',{'action':'get','path':path})
  row['saved']=saved;row['passed']=text.strip() in str(saved)
  c.screenshot(name+'-edited.png',settle_ms=500)
  (OUT/(name+'-tree-after.txt')).write_text(str(c.call('capture',{})))
 except Exception as e:row['error']=str(e);row['passed']=False
 results.append(row);record('app-results.json',results);print(json.dumps({k:row[k] for k in ['name','passed','launch_seconds','needs_no_sandbox','error'] if k in row}),flush=True)
