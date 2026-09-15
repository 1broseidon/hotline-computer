from qa import *
import shutil
c=client()
for name,job in json.loads((OUT/'download-jobs.json').read_text()).items():c.done(job,120)
context=WORK/'build';context.mkdir(exist_ok=True)
shutil.copy2(WORK/'code.deb',context/'code.deb')
dockerfile='''FROM ghcr.io/1broseidon/toad-computer:0.4.0
USER root
ENV DEBIAN_FRONTEND=noninteractive
COPY code.deb /tmp/code.deb
RUN printf 'code code/add-microsoft-repo boolean false\\n' | debconf-set-selections \\
    && apt-get update \\
    && apt-get install -y --no-install-recommends /tmp/code.deb featherpad build-essential libgtk-3-dev pkg-config \\
    && rm -f /tmp/code.deb \\
    && rm -rf /var/lib/apt/lists/*
USER agent
'''
(context/'Dockerfile').write_text(dockerfile);(OUT/'Dockerfile.apps').write_text(dockerfile)
start=time.monotonic()
with (OUT/'image-build.log').open('w') as log:
 p=subprocess.run(['docker','build','--progress','plain','-t','toad-computer:desktop-apps-proof',str(context)],stdout=log,stderr=subprocess.STDOUT)
record('image-build.json',{'exit_code':p.returncode,'seconds':time.monotonic()-start})
print('Image build',p.returncode,round(time.monotonic()-start,2),flush=True)
if p.returncode:sys.exit(p.returncode)
# Replace only the disposable QA container created by this script; retain its files.
old=(WORK/'container').read_text().strip();(OUT/'initial-container.log').write_text(docker('logs',old,check=False));docker('rm','-f',old)
token=(WORK/'token').read_text().strip()
cid=docker('run','-d','--cap-drop=ALL','--security-opt=no-new-privileges','--cpus=2','--pids-limit=1024','--memory=4g','--shm-size=1g','-p','127.0.0.1::8787','-e','TOAD_COMPUTER_TOKEN='+token,'-v',str(WORK)+':/home/agent/workspace','toad-computer:desktop-apps-proof')
(WORK/'container').write_text(cid);port=docker('port',cid,'8787/tcp').rsplit(':',1)[1];url='http://127.0.0.1:'+port;(WORK/'url').write_text(url)
for _ in range(200):
 try:urllib.request.urlopen(url+'/health',timeout=.2);break
 except Exception:time.sleep(.05)
c=client();record('app-container.json',{'id':cid,'image':docker('inspect',cid,'--format','{{.Image}}')})
record('packages.json',{'metadata':a.execute(c,'dpkg-query',['-W','code','featherpad','build-essential','libgtk-3-dev','pkg-config']),'sha256':a.execute(c,'sha256sum',['/home/agent/workspace/code.deb','/home/agent/workspace/geany-2.1.tar.gz'])})
job=c.call('files',{'action':'extract','path':'/home/agent/workspace/geany-2.1.tar.gz','destination':'/home/agent/workspace/source'})
c.done(job,30)
script='set -eu\n./configure --prefix=/home/agent/.local --disable-html-docs\nmake -j2\nmake install\n'
c.call('files',{'action':'put','path':'/home/agent/workspace/build-geany.sh','content':script})
job=c.call('files',{'action':'run','path':'/home/agent/workspace/build-geany.sh','cwd':'/home/agent/workspace/source/geany-2.1'})
record('geany-build-job.json',job);print('Geany build started through managed shell',flush=True)
