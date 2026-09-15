import sys,json,hashlib
from pathlib import Path
sys.path.insert(0,'/Users/george/Projects/personal/toad-computer/tests')
from acceptance import Computer,execute
r=Path('/tmp/toad-candidate-b7cd1fa');out=r/'host-native';c=Computer((r/'url.txt').read_text().strip(),(r/'token').read_text().strip(),out)
expected='  café 🐸\tfirst line\nsecond line\n  trailing spaces  '
actual=c.call('browser',{'action':'eval','js':'document.getElementById("clipboard").value'})
assert actual==expected,(actual,expected)
c.screenshot('17-host-browser-clipboard.png')
(out/'results.json').write_text(json.dumps({'passed':True,'native_field_exact':True,'browser_multiline_tabs_whitespace_exact':True,'host_app_reopened_existing_container':True,'automation_note':'CUA paste reported a clipboard-read timeout, but the page acknowledged Pasted and the guest value matched exactly. The action was not retried.'},indent=2))
script='#!/bin/bash\nprintf "installer argv: %s\\n" "$1"\nexit 19\n'
server_source='''import http.server
PAYLOAD = '''+repr(script.encode())+'''
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  if self.path=='/redirect':
   self.send_response(302);self.send_header('Location','/install.sh');self.end_headers();return
  self.send_response(200);self.send_header('Content-Length',str(len(PAYLOAD)));self.end_headers();self.wfile.write(PAYLOAD)
http.server.HTTPServer(('127.0.0.1',8084),Handler).serve_forever()
'''
c.call('files',{'action':'put','path':'/home/agent/qa/installer-server.py','content':server_source})
server=c.call('shell',{'action':'start','command':'python3','args':['/home/agent/qa/installer-server.py'],'label':'Redirected installer fixture'})
try:
 execute(c,'python3',['-c','import urllib.request,time\nfor _ in range(100):\n try:urllib.request.urlopen("http://127.0.0.1:8084/install.sh");break\n except OSError:time.sleep(.05)'])
 job=c.call('files',{'action':'run','url':'http://127.0.0.1:8084/redirect','path':'/home/agent/qa/path with spaces/install fixture.sh','sha256':hashlib.sha256(script.encode()).hexdigest(),'args':['argument with spaces']})
 job=c.call('shell',{'action':'wait','job_id':job['id'],'wait_ms':5000})
 assert job['state']=='failed' and job['exit_code']==19,job
 output=c.call('shell',{'action':'read','job_id':job['id']})['output'];assert 'installer argv: argument with spaces' in output,output
 (out/'redirected-installer.json').write_text(json.dumps({'passed':True,'redirect_followed':True,'path_and_argument_with_spaces':True,'verified_sha256':hashlib.sha256(script.encode()).hexdigest(),'installer_exit_code':19,'output':output},indent=2))
finally:c.call('shell',{'action':'cancel','job_id':server['id']})
print('Exact host clipboard values and redirected installer passed')
