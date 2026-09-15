from qa import *
c=client();path='/home/agent/workspace/geany-qa.txt';c.call('files',{'action':'put','path':path,'content':'Geany source build QA\n'})
start=time.monotonic();job=c.call('shell',{'action':'start','command':'/home/agent/.local/bin/geany','args':['--new-instance',path],'label':'Geany C/C++ source-built app'})
record('geany-app-job.json',job)
for _ in range(50):
 windows=c.call('windows',{'action':'list'});w=next((w for w in windows if 'geany' in w['class'].lower()),None)
 if w:break
 time.sleep(.1)
assert w, 'Geany did not map a window'
launch=time.monotonic()-start
c.call('windows',{'action':'maximize','window_id':w['id']});c.call('windows',{'action':'focus','window_id':w['id']})
time.sleep(.5)
(OUT/'geany-tree.txt').write_text(str(c.call('capture',{})))
c.call('input',{'action':'key','combo':'ctrl+a','settle_ms':100})
text='Geany C/C++ source build proof — café 🐸\nEdited and saved through the GTK desktop UI.\n'
c.call('input',{'action':'paste','text':text,'settle_ms':250});c.call('input',{'action':'key','combo':'ctrl+s','settle_ms':500})
saved=c.call('files',{'action':'get','path':path});c.screenshot('geany-saved.png',settle_ms=300)
result={'passed':saved==text,'saved':saved,'launch_seconds':launch,'window':w,'job':job};record('geany-ui-result.json',result);print(json.dumps({'passed':result['passed'],'launch_seconds':launch}))
# Direct Electron startup makes sandbox diagnostics visible, unlike its CLI launcher.
p=c.call('shell',{'action':'start','command':'/usr/share/code/code','args':['--user-data-dir=/home/agent/workspace/code-sandbox-probe','--disable-extensions','--enable-logging=stderr','/home/agent/workspace/vscode-qa.txt'],'label':'VS Code default sandbox probe'})
time.sleep(3)
r=c.call('shell',{'action':'read','job_id':p['id'],'max_output':32768});record('vscode-sandbox-probe.json',r);print('Sandbox probe',r['job']['state'],r['output'][-1700:])
c.call('shell',{'action':'cancel','job_id':p['id']})
