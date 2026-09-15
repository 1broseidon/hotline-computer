import subprocess,time,json,urllib.request
from pathlib import Path
r=Path('/tmp/toad-candidate-b7cd1fa');name=(r/'container-name.txt').read_text().strip();samples=[]
for _ in range(3):
 subprocess.run(['docker','kill',name],check=True,stdout=subprocess.DEVNULL)
 start=time.monotonic();subprocess.run(['docker','start',name],check=True,stdout=subprocess.DEVNULL);launched=time.monotonic()
 port=subprocess.check_output(['docker','port',name,'8787/tcp'],text=True).strip().rsplit(':',1)[1];located=time.monotonic();url='http://127.0.0.1:'+port
 for attempt in range(150):
  try:
   with urllib.request.urlopen(url+'/health',timeout=1):break
  except OSError:time.sleep(.05)
 else:raise RuntimeError('failed health')
 ended=time.monotonic();(r/'url.txt').write_text(url)
 sample={'docker_start_seconds':launched-start,'port_lookup_seconds':located-launched,'health_wait_seconds':ended-located,'total_seconds':ended-start};samples.append(sample);print(json.dumps(sample),flush=True)
(r/'startup-timings.json').write_text(json.dumps({'initial_recovery_seconds':json.loads((r/'recovery/recovery/results.json').read_text())['startup_seconds'],'additional_samples':samples},indent=2))
