"""Short compatibility check; reuses immutable Nix paths and a previously built QA app."""
import importlib.util,json,pathlib,secrets,time,urllib.request,traceback
spec=importlib.util.spec_from_file_location('bench',pathlib.Path(__file__).with_name('benchmark.py'))
b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
a=b.a; docker=b.docker
seed=pathlib.Path('/Volumes/Storage/george/toad-qa-cache/os-review-2026-09-15/seed')
binary='/Volumes/Storage/george/toad-qa-cache/native-target/debug/toad-desktop'
rows=[]
for label,image in [('debian','ghcr.io/1broseidon/toad-computer:0.4.0'),('alpine','toad-computer:os-review-alpine-current')]:
    out=b.HERE/'compatibility'/label;out.mkdir(parents=True,exist_ok=True)
    token=secrets.token_hex(24)
    cid=docker('run','-d','--cap-drop=ALL','--security-opt=no-new-privileges','--cpus=2','--pids-limit=1024','--memory=4g','--shm-size=1g','-p','127.0.0.1::8787','-e','TOAD_COMPUTER_TOKEN='+token,'-v','toad-nix-glibc:/nix:ro','-v',str(seed)+':/home/agent/review-seed:ro','-v',binary+':/home/agent/prebuilt-toad:ro',image)
    row={'label':label,'image':image,'cases':[]}
    try:
        port=docker('port',cid,'8787/tcp').rsplit(':',1)[1];url='http://127.0.0.1:'+port
        for _ in range(300):
            try: urllib.request.urlopen(url+'/health',timeout=.2);break
            except Exception: time.sleep(.05)
        c=a.Computer(url,token,out)
        docker('exec',cid,'sh','-c','mkdir -p /home/agent/.cache/toad; cp -R /home/agent/review-seed/environments /home/agent/.cache/toad/; mkdir -p /home/agent/project/.toad; cp /home/agent/review-seed/native-environment.json /home/agent/project/.toad/environment.json; cp -R /home/agent/review-seed/ui /home/agent/project/ui')
        def case(name,fn):
            start=time.monotonic()
            try:
                value=fn();result={'name':name,'passed':True,'output':value}
            except Exception as e: result={'name':name,'passed':False,'error':str(e)}
            result['seconds']=round(time.monotonic()-start,3);row['cases'].append(result)
            print(json.dumps({'label':label,**result}),flush=True)
            (out/'results.json').write_text(json.dumps(row,indent=2))
        case('Ketch download and local scrape',lambda:a.ketch(c))
        def environments():
            results={}
            for profile,command,args in [('python','python3',['-c','import ssl,sqlite3; print(6*7)']),('node','node',['-e','console.log(6*7)']),('rust','rustc',['--version']),('go','go',['version'])]:
                root='/home/agent/profiles/'+profile
                a.prepare(c,profile,root)
                results[profile]=a.execute(c,command,args,cwd=root)
            return results
        case('four cached Nix toolchains',environments)
        case('native Toad frontend build',lambda:a.execute(c,'bun',['run','build'],cwd='/home/agent/project/ui',timeout=90))
        def native():
            job=c.call('shell',{'action':'start','command':'/home/agent/prebuilt-toad','cwd':'/home/agent/project','env':{'TOAD_DATA_DIR':'/home/agent/project/qa-data'},'label':'Native app base OS check'})
            try:
                a.native_screens(c,job)
            finally:
                c.call('shell',{'action':'cancel','job_id':job['id']})
        case('prebuilt Nix Toad native screens',native)
        row['system_versions']=docker('exec',cid,'sh','-c','cat /etc/os-release; chromium --version; nix --version; alacritty --version; ldd --version 2>&1 || true')
    except Exception as e:
        row['setup_error']=str(e);print(traceback.format_exc(),flush=True)
    finally:
        (out/'container.log').write_text(docker('logs',cid,check=False))
        docker('rm','-f',cid)
        (out/'results.json').write_text(json.dumps(row,indent=2));rows.append(row)
        (b.HERE/'compatibility-results.json').write_text(json.dumps(rows,indent=2))
