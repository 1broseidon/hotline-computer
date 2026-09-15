from qa import *
c=client()
# Dismiss the observed VS Code welcome dialog without signing in.
c.call('input',{'action':'click','x':1410,'y':309,'settle_ms':500})
w=next(w for w in c.call('windows',{'action':'list'}) if w['class'].lower()=='code.code')
c.call('windows',{'action':'focus','window_id':w['id']})
c.call('input',{'action':'key','combo':'ctrl+1','settle_ms':150})
c.call('input',{'action':'key','combo':'ctrl+s','settle_ms':500})
record('vscode-save-retry.json',{'saved':c.call('files',{'action':'get','path':'/home/agent/workspace/vscode-qa.txt'})})
c.screenshot('vscode-saved.png')
# Expose the missing source-build program precisely.
job=c.call('shell',{'action':'start','command':'make','args':['V=1','geany.desktop'],'cwd':'/home/agent/workspace/source/geany-2.1','label':'Diagnose Geany desktop launcher'})
try:c.done(job,15)
except Exception:pass
r=c.call('shell',{'action':'read','job_id':job['id']});record('geany-desktop-diagnostic.json',r);print(r['output'],flush=True)
# User-local apt metadata and package extraction need no privilege.
script='''set -eu
mkdir -p /home/agent/workspace/apt/lists/partial /home/agent/workspace/apt/cache/archives/partial /home/agent/workspace/gettext
apt-get -o Dir::State::lists=/home/agent/workspace/apt/lists -o Dir::Cache=/home/agent/workspace/apt/cache update
cd /home/agent/workspace/gettext
apt-get -o Dir::State::lists=/home/agent/workspace/apt/lists download gettext
for package in *.deb; do dpkg-deb -x "$package" extracted; done
ldd extracted/usr/bin/msgfmt
'''
c.call('files',{'action':'put','path':'/home/agent/workspace/get-gettext.sh','content':script})
job=c.call('files',{'action':'run','path':'/home/agent/workspace/get-gettext.sh','cwd':'/home/agent/workspace'})
record('gettext-job.json',job)
print('gettext acquisition started',flush=True)
