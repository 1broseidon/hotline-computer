from qa import *
c=client()
command='export LD_LIBRARY_PATH=/home/agent/workspace/gettext/extracted/usr/lib/aarch64-linux-gnu; make MSGFMT=/home/agent/workspace/gettext/extracted/usr/bin/msgfmt install'
job=c.call('shell',{'action':'start','command':'bash','args':['-c',command],'cwd':'/home/agent/workspace/source/geany-2.1','label':'Finish Geany source installation'})
c.done(job,60);r=c.call('shell',{'action':'read','job_id':job['id'],'max_output':1048576});record('geany-install-final.json',r)
print(a.execute(c,'/home/agent/.local/bin/geany',['--version']))
# Capture only after a working Qt edit/save, to isolate the earlier crash.
q=json.loads((OUT/'qt-probe-job.json').read_text());before=c.call('shell',{'action':'status','job_id':q['id']})
tree=c.call('capture',{});(OUT/'qt-accessibility-tree.txt').write_text(str(tree));time.sleep(.5)
after=c.call('shell',{'action':'status','job_id':q['id']});record('qt-accessibility-probe.json',{'before':before,'after':after})
print('Qt before/after accessibility:',before['state'],after['state'],after.get('signal'))
