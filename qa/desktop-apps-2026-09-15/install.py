from qa import *
import os
cid=(WORK/'container').read_text().strip();c=client()
steps=[('apt-update',['apt-get','update']),('build-dependencies',['apt-get','install','-y','--no-install-recommends','build-essential','libgtk-3-dev','pkg-config'])]
for name,args in steps:
 start=time.monotonic()
 with (OUT/(name+'.log')).open('w') as out:
  p=subprocess.run(['docker','exec','--user','0','-e','DEBIAN_FRONTEND=noninteractive',cid,*args],stdout=out,stderr=subprocess.STDOUT)
 print(name,p.returncode,round(time.monotonic()-start,2),flush=True)
 if p.returncode:sys.exit(p.returncode)
for name,job in json.loads((OUT/'download-jobs.json').read_text()).items():
 c.done(job,180);print(name,'download complete',flush=True)
a.execute(c,'apt-get',['download','featherpad'],cwd='/home/agent/workspace',timeout=60)
packages=json.loads(a.execute(c,'python3',['-c','import glob,json;print(json.dumps(glob.glob("/home/agent/workspace/*.deb")))']))
with (OUT/'deb-install.log').open('w') as out:
 start=time.monotonic()
 # Keep this disposable test from adding a third-party apt repository.
 subprocess.run(['docker','exec','-i','--user','0',cid,'debconf-set-selections'],input='code code/add-microsoft-repo boolean false\n',text=True,stdout=out,stderr=subprocess.STDOUT,check=True)
 p=subprocess.run(['docker','exec','--user','0','-e','DEBIAN_FRONTEND=noninteractive',cid,'apt-get','install','-y','--no-install-recommends',*packages],stdout=out,stderr=subprocess.STDOUT)
 print('deb install',p.returncode,round(time.monotonic()-start,2),flush=True)
 if p.returncode:sys.exit(p.returncode)
record('packages.json',{'metadata':a.execute(c,'dpkg-query',['-W','code','featherpad','build-essential','libgtk-3-dev','pkg-config']),'files':a.execute(c,'bash',['-c','sha256sum /home/agent/workspace/*.deb /home/agent/workspace/geany-2.1.tar.gz'])})
job=c.call('files',{'action':'extract','path':'/home/agent/workspace/geany-2.1.tar.gz','destination':'/home/agent/workspace/source'})
c.done(job,30)
script='set -eu\n./configure --prefix=/home/agent/.local --disable-html-docs\nmake -j2\nmake install\n'
c.call('files',{'action':'put','path':'/home/agent/workspace/build-geany.sh','content':script})
job=c.call('files',{'action':'run','path':'/home/agent/workspace/build-geany.sh','cwd':'/home/agent/workspace/source/geany-2.1'})
record('geany-build-job.json',job);print('Geany C/C++ build started',flush=True)
