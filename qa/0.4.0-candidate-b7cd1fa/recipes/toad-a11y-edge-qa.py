import sys,json,time
from pathlib import Path
sys.path.insert(0,'/Users/george/Projects/personal/toad-computer/tests')
from acceptance import Computer,execute
r=Path('/tmp/toad-candidate-b7cd1fa');out=r/'a11y-edges';out.mkdir(exist_ok=True)
c=Computer((r/'url.txt').read_text().strip(),(r/'token').read_text().strip(),out)
fixture=next(j for j in c.call('shell',{'action':'list'}) if j['label']=='Native controls acceptance')
job=c.call('shell',{'action':'launch','command':fixture['command'],'cwd':fixture['cwd'],'label':'Window identity edge acceptance'})
try:
 for _ in range(60):
  native=[w for w in c.call('windows',{'action':'list'}) if w.get('pid')==job['pid']]
  if len(native)==2:break
  time.sleep(.1)
 assert len(native)==2,native
 primary=next(w for w in native if w['minimum_size'][0]>=640)
 secondary=next(w for w in native if w['id']!=primary['id'])
 c.call('windows',{'action':'tile','primary_id':primary['id'],'observer_id':secondary['id']})
 before=str(c.call('capture',{}));assert 'Primary acceptance window' in before and 'Secondary acceptance window' in before,before
 execute(c,'xprop',['-id',primary['id'],'-f','_NET_WM_NAME','8u','-set','_NET_WM_NAME','Renamed QA window'])
 tree=str(c.call('capture',{}));section=tree.split('['+primary['id']+' ',1)[1].split('\n[0x',1)[0]
 assert 'Renamed QA window' in section,section
 assert 'Primary acceptance window' in section and 'Secondary acceptance window' not in section,section
 (out/'renamed-tree.txt').write_text(tree)
 c.screenshot('16-renamed-window-identity.png')
 c.call('windows',{'action':'close','window_id':secondary['id']})
 tree=str(c.call('capture',{}));assert '['+secondary['id']+' ' not in tree,tree
 (out/'results.json').write_text(json.dumps({'passed':True,'renamed_window_keeps_its_own_controls_without_substitution':True,'disappearing_window_removed':True},indent=2))
finally:c.call('shell',{'action':'cancel','job_id':job['id']})
