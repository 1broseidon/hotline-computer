"""Fresh containers, same limits; never touches existing desktops or their volumes."""
import importlib.util, json, pathlib, secrets, subprocess, sys, time, urllib.request
HERE=pathlib.Path(__file__).resolve().parent
ROOT=HERE.parent.parent
spec=importlib.util.spec_from_file_location('acceptance',ROOT/'tests/acceptance.py')
a=importlib.util.module_from_spec(spec); spec.loader.exec_module(a)

def docker(*args, check=True):
    r=subprocess.run(['docker',*args],text=True,capture_output=True)
    if check and r.returncode: raise RuntimeError(r.stderr)
    return r.stdout.strip()

def usage(cid):
    return json.loads(docker('exec',cid,'python3','-c', '''import json,pathlib
p=pathlib.Path('/sys/fs/cgroup')
def kv(name): return {k:int(v) for k,v in (s.split() for s in (p/name).read_text().splitlines())}
m=kv('memory.stat'); c=kv('cpu.stat')
print(json.dumps({'memory_current':int((p/'memory.current').read_text()),'anon':m.get('anon',0),'shmem':m.get('shmem',0),'inactive_file':m.get('inactive_file',0),'cpu_usec':c['usage_usec']}))'''))

def measured(label,fn):
    start=time.perf_counter(); value=fn()
    return {'name':label,'seconds':time.perf_counter()-start,'value':value}

def run(image,label,index):
    out=HERE/'benchmark'/f'{label}-{index}';out.mkdir(parents=True,exist_ok=True)
    token=secrets.token_hex(24)
    cid=docker('create','--cap-drop=ALL','--security-opt=no-new-privileges','--cpus=2','--pids-limit=1024','--memory=4g','--shm-size=1g','-p','127.0.0.1::8787','-e','TOAD_COMPUTER_TOKEN='+token,image)
    row={'image':image,'label':label,'index':index,'container_id':cid,'measurements':[]}
    try:
        started=time.perf_counter();docker('start',cid)
        port=docker('port',cid,'8787/tcp').rsplit(':',1)[1];url='http://127.0.0.1:'+port
        for _ in range(600):
            try:
                with urllib.request.urlopen(url+'/health',timeout=.2) as response: health=response.read().decode()
                break
            except Exception: time.sleep(.01)
        else: raise RuntimeError('health timeout')
        row['start_to_health_s']=time.perf_counter()-started
        if label=='alpine-030': return row
        c=a.Computer(url,token,out)
        row['info']=c.call('state',{'action':'info'})
        time.sleep(1);first=usage(cid);time.sleep(2);last=usage(cid)
        row['idle']={'before':first,'after':last,'cpu_core_percent':(last['cpu_usec']-first['cpu_usec'])/20000}
        start=time.perf_counter();c.call('browser',{'action':'navigate','url':'data:text/html,<title>OS review</title><h1>Ready</h1>'})
        row['first_browser_navigation_s']=time.perf_counter()-start
        row['measurements'].append(measured('simple_form',lambda:a.browser(c)))
        row['measurements'].append(measured('wizard',lambda:a.wizard(c)))
        samples=[]
        for _ in range(5):
            start=time.perf_counter();job=c.call('shell',{'action':'start','command':'/bin/true','label':'OS review no-op'});samples.append((time.perf_counter()-start)*1000);c.done(job)
        row['job_ack_ms']=samples
        row['measurements'].append(measured('observer',lambda:c.call('shell',{'action':'show','job_id':job['id']})))
        time.sleep(1);first=usage(cid);time.sleep(2);last=usage(cid)
        row['browser_observer_idle']={'before':first,'after':last,'cpu_core_percent':(last['cpu_usec']-first['cpu_usec'])/20000}
        row['glx']=docker('exec',cid,'glxinfo','-B',check=False)
        row['processes']=docker('top',cid,'-eo','pid,comm')
    except Exception as e: row['error']=str(e)
    finally:
        (out/'container.log').write_text(docker('logs',cid,check=False))
        docker('rm','-f',cid)
        (out/'result.json').write_text(json.dumps(row,indent=2))
    return row

if __name__=='__main__':
    configs=[('ghcr.io/1broseidon/toad-computer:0.4.0','debian-040'),('toad-computer:os-review-alpine-current','alpine-current')]
    rows=[]
    for index in range(1):
        for image,label in (configs if index%2==0 else list(reversed(configs))):
            row=run(image,label,index);rows.append(row)
            print(json.dumps({k:row[k] for k in ['label','index','start_to_health_s','first_browser_navigation_s','error'] if k in row}),flush=True)
            (HERE/'benchmark-results.json').write_text(json.dumps(rows,indent=2))
