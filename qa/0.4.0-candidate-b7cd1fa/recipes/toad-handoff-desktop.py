import sys,json,time
from pathlib import Path
sys.path.insert(0,'/Users/george/Projects/personal/toad-computer/tests')
from acceptance import Computer
r=Path('/tmp/toad-candidate-b7cd1fa');out=r/'host-native';c=Computer((r/'url.txt').read_text().strip(),(r/'token').read_text().strip(),out)
fixture=json.loads((r/'native-job.json').read_text())['job'];c.call('shell',{'action':'cancel','job_id':fixture['id']})
root='/home/agent/qa/toad-a89ce91835fc'
app=c.call('shell',{'action':'launch','command':'/home/agent/qa/native-target/debug/toad-desktop','cwd':root,'env':{'TOAD_DATA_DIR':root+'/handoff-data'},'label':'Toad live QA'})
for _ in range(100):
 windows=c.call('windows',{'action':'list'});native=next((w for w in windows if w.get('pid')==app['pid']),None)
 if native:break
 time.sleep(.1)
assert native,windows
version=c.call('shell',{'action':'start','command':'/home/agent/qa/ketch-a89ce91835fc/bin/ketch','args':['--version'],'label':'Ketch installed and verified'})
c.done(version);c.call('shell',{'action':'show','job_id':version['id']})
observer=next(w for w in c.call('windows',{'action':'list'}) if 'toadterminal' in w['class'].lower())
c.call('windows',{'action':'tile','primary_id':native['id'],'observer_id':observer['id']})
for _ in range(100):
 tree=str(c.call('capture',{}))
 if 'New teammate' in tree:break
 time.sleep(.1)
assert 'New teammate' in tree,tree
c.screenshot('18-live-toad-and-ketch.png',settle_ms=1000)
(r/'live-handoff.json').write_text(json.dumps({'toad_job_id':app['id'],'pid':app['pid'],'window':native['id'],'ketch_version_job':version['id'],'viewer_is_read_only':True},indent=2))
print('Native Toad and verified Ketch are visible for handoff')
