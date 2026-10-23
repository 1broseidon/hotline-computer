"""Reproducible probes against the isolated audit computer only."""
from client import Client, OUT
import json
import re
import sys
import time
from urllib.parse import quote

c = Client(holder='audit')
def call(tool, args, label):
    value = c.call(tool, args, label, print_result=False)
    print(label, json.dumps(value, ensure_ascii=False)[:1800], flush=True)
    return value

if sys.argv[1] == 'jobs':
    start = {'action':'start','command':'python3','args':['-u','-c','import time; print("started"); time.sleep(30); print("finished")'],'request_id':'audit-retry-1','label':'Check managed job retry'}
    job = call('shell', start, 'job-start')
    again = call('shell', start, 'job-retry')
    assert again['id'] == job['id']
    call('shell', {**start, 'args':['-c','print("different")']}, 'job-retry-conflict')
    call('shell', {'action':'read','job_id':job['id']}, 'job-read')
    call('shell', {'action':'cancel','job_id':job['id']}, 'job-cancel')
    pipe = call('shell', {'action':'start','command':'python3','args':['-u','-c','import sys; print(sys.stdin.read().upper())'],'label':'Check stdin and EOF'}, 'pipe-start')
    call('shell', {'action':'write','job_id':pipe['id'],'text':'Hello stdin\n','eof':True}, 'pipe-write')
    call('shell', {'action':'wait','job_id':pipe['id'],'wait_ms':5000}, 'pipe-wait')
    call('shell', {'action':'read','job_id':pipe['id']}, 'pipe-read')
    call('shell', {'command':'python3','args':['-c','import sys; print("failure detail", file=sys.stderr); sys.exit(17)'],'label':'Check nonzero exit reporting'}, 'job-failure')
    call('shell', {'command':'python3','args':['-c','print("x" * 20000); print("END-OF-OUTPUT")'],'max_output':100,'label':'Check bounded output'}, 'job-output')
    call('files', {'action':'put','path':'/home/agent/audit/hello.txt','content':'Hello Zoë 审核\n'}, 'file-put')
    call('files', {'action':'get','path':'/home/agent/audit/hello.txt'}, 'file-get')
    call('files', {'action':'get','path':'/etc/passwd'}, 'file-outside-home')
    call('capture', {'mode':'png'}, 'capture-png-default')
    p = json.loads((OUT/'capture-png-default.json').read_text())['result']['content'][0]['text']
    call('files', {'action':'get','path':p}, 'capture-png-get')

elif sys.argv[1] == 'clients':
    a, b = Client('client-a'), Client('client-b')
    a.call('state', {'action':'control','duration':30}, 'lease-anonymous-a')
    b.call('files', {'action':'put','path':'/home/agent/audit/client-b.txt','content':'written during client A lease'}, 'lease-anonymous-b-write')
    b.call('state', {'action':'release'}, 'lease-anonymous-b-release')
    a, b = Client('named-a','named-a'), Client('named-b','named-b')
    a.call('state', {'action':'control','duration':30}, 'lease-named-a')
    b.call('files', {'action':'put','path':'/home/agent/audit/named-b.txt','content':'should be refused'}, 'lease-named-b-write')
    a.call('state', {'action':'release'}, 'lease-named-a-release')
    for version in ['2024-11-05','2025-06-18','2025-11-25','2026-07-28']:
        x = Client('protocol-'+version, version=version)
        print('protocol',version,x.initialized,flush=True)
        (OUT/('protocol-'+version+'.json')).write_text(json.dumps(x.initialized,indent=2))

elif sys.argv[1] == 'browser':
    html = '''<title>Audit edge cases</title><h1>Components</h1><button id=original onclick="document.querySelector('output').textContent='ORIGINAL'">Original action</button><output></output><iframe title="Embedded form" srcdoc="<label>Frame name<input></label><button>Frame save</button>"></iframe><div id=host></div><script>document.querySelector('#host').attachShadow({mode:'open'}).innerHTML='<label>Shadow name<input></label><button>Shadow save</button>'</script>'''
    call('browser', {'action':'navigate','url':'data:text/html,'+quote(html)}, 'components-navigate')
    snapshot = call('browser', {'action':'text'}, 'components-text')
    reference = re.search(r'\[(e\d+)\] \[button\] Original action',snapshot).group(1)
    call('capture', {'window':'Audit edge cases'}, 'components-screen')
    call('browser', {'action':'eval','js':"(()=>{const b=document.createElement('button');b.textContent='Different action';b.onclick=()=>document.querySelector('output').textContent='DIFFERENT';document.querySelector('#original').before(b);return 'inserted';})()"}, 'components-insert')
    call('browser', {'action':'text'}, 'components-refreshed')
    call('browser', {'action':'click_ref','ref':reference}, 'components-stale-click')
    call('browser', {'action':'eval','js':"document.querySelector('output').textContent"}, 'components-stale-result')
    call('browser', {'action':'fill','ref':'e999','text':'test'}, 'browser-missing-ref')
    call('input', {'action':'click'}, 'input-missing-coordinates')
    call('input', {'action':'move','x':500,'y':400}, 'input-default-capture')
    call('input', {'action':'move','x':500,'y':400,'capture_after':'final'}, 'input-explicit-capture')

elif sys.argv[1] == 'prepare':
    call('files', {'action':'put','path':'src/audit-project/app.py','content':"import http.server, os\nprint('audit server ready', flush=True)\nhttp.server.test(HandlerClass=http.server.SimpleHTTPRequestHandler, port=int(os.environ['PORT']), bind='127.0.0.1')\n"}, 'project-file')
    call('files', {'action':'put','path':'src/audit-project/index.html','content':'<title>Prepared project</title><h1>Built on the computer</h1><button onclick="this.textContent=\'It works\'">Test project</button>'}, 'project-html')
    call('state', {'action':'manifest','workspace':'src/audit-project'}, 'project-manifest-draft')
    manifest = {'packages':['python312'],'runs':{'test':{'command':['python3','-m','py_compile','app.py']},'app':{'command':['python3','app.py'],'kind':'web'}}}
    call('state', {'action':'prepare','workspace':'src/audit-project','manifest':manifest}, 'project-prepare')

elif sys.argv[1] == 'native':
    source = '''import ctypes as C
g=C.CDLL('libgtk-3.so.0'); o=C.CDLL('libgobject-2.0.so.0')
def bind(name, restype, types):
 f=getattr(g,name); f.restype=restype; f.argtypes=types; return f
ptr=C.c_void_p; integer=C.c_int; string=C.c_char_p
bind('gtk_init',None,[ptr,ptr])(None,None)
win=bind('gtk_window_new',ptr,[integer])(0)
bind('gtk_window_set_title',None,[ptr,string])(win,b'Native audit app')
bind('gtk_window_set_default_size',None,[ptr,integer,integer])(win,600,300)
box=bind('gtk_box_new',ptr,[integer,integer])(1,12)
bind('gtk_container_add',None,[ptr,ptr])(win,box)
entry=bind('gtk_entry_new',ptr,[])()
bind('gtk_entry_set_placeholder_text',None,[ptr,string])(entry,b'Enter audit text')
button=bind('gtk_button_new_with_label',ptr,[string])(b'Save native text')
label=bind('gtk_label_new',ptr,[string])(b'Waiting for input')
pack=bind('gtk_box_pack_start',None,[ptr,ptr,integer,integer,integer])
for child in [entry,button,label]: pack(box,child,0,0,12)
get=bind('gtk_entry_get_text',string,[ptr]); put=bind('gtk_label_set_text',None,[ptr,string])
@C.CFUNCTYPE(None,ptr,ptr)
def clicked(*args): put(label,b'Saved: '+get(entry))
o.g_signal_connect_data.argtypes=[ptr,string,ptr,ptr,ptr,integer]
o.g_signal_connect_data(button,b'clicked',clicked,None,None,0)
bind('gtk_widget_show_all',None,[ptr])(win)
g.gtk_main()
'''
    call('files', {'action':'put','path':'/home/agent/audit/native.py','content':source}, 'native-file')
    call('shell', {'action':'launch','command':'python3','args':['/home/agent/audit/native.py'],'label':'Try native desktop controls'}, 'native-launch')
    call('wait', {'text':'Save native text','timeout':5}, 'native-wait')
    call('capture', {'window':'Native audit app'}, 'native-initial')

elif sys.argv[1] == 'http':
    from client import BASE, TOKEN
    import urllib.request
    import urllib.error
    results=[]
    for label,path,headers in [
        ('health','/health',{}),
        ('mcp-no-auth','/mcp',{'Content-Type':'application/json','Accept':'application/json, text/event-stream'}),
        ('files-no-auth','/files',{}),
        ('foreign-origin','/mcp',{'Authorization':'Bearer '+TOKEN,'Origin':'https://audit-untrusted.example','Content-Type':'application/json','Accept':'application/json, text/event-stream'}),
    ]:
        data=json.dumps({'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':'2025-11-25','capabilities':{},'clientInfo':{'name':'http-audit','version':'1'}}}).encode() if path=='/mcp' else None
        try:
            with urllib.request.urlopen(urllib.request.Request(BASE+path,data=data,headers=headers),timeout=5) as r:
                result={'label':label,'status':r.status,'content_type':r.headers.get('Content-Type')}
        except urllib.error.HTTPError as e:
            result={'label':label,'status':e.code,'body':e.read().decode()}
        results.append(result)
    (OUT/'http-boundaries.json').write_text(json.dumps(results,indent=2))
    print(json.dumps(results),flush=True)

elif sys.argv[1] == 'context':
    html='<title>Long page</title>'+''.join('<p>Paragraph '+str(i)+' '+('ordinary content '*20)+'</p>' for i in range(1000))
    c.call('files',{'action':'put','path':'/home/agent/src/audit-project/long.html','content':html},'long-page-file',print_result=False)
    call('browser',{'action':'navigate','url':'http://localhost:39463/long.html'},'long-page-navigate')
    text=c.call('browser',{'action':'text'},'long-page-text',print_result=False)
    print('browser text bytes',len(text.encode()),'starts',text[:80],'ends',text[-80:],flush=True)

elif sys.argv[1] == 'recovery':
    windows=call('windows',{'action':'list'},'before-browser-close')
    browser=next(w for w in windows if 'Chromium' in w['class'])
    call('windows',{'action':'close','window_id':browser['id']},'browser-close')
    call('browser',{'action':'navigate','url':'data:text/html,<title>Recovered</title><h1>Browser recovered</h1>'},'browser-reopen')
    call('browser',{'action':'text'},'browser-recovered-text')
    call('shell',{'action':'start','command':'python3','args':['-u','-c','import time; print("RESTART-EVIDENCE"); time.sleep(600)'],'label':'Check restart persistence'},'restart-job')

elif sys.argv[1] == 'boundaries':
    call('browser',{'action':'navigate','url':'data:text/html,<h1>Isolated boundary check</h1>'},'boundary-browser')
    code="""import json, urllib.request
from pathlib import Path
result={}
try: result['pid1_contains_server_token']=any(x.startswith(b'HOTLINE_COMPUTER_TOKEN=') for x in Path('/proc/1/environ').read_bytes().split(b'\\0'))
except Exception as e: result['pid1_error']=str(e)
try:
 with urllib.request.urlopen('http://127.0.0.1:9222/json/version',timeout=3) as r:
  value=json.load(r)
  result['cdp_status']=r.status
  result['cdp_websocket_advertised']='webSocketDebuggerUrl' in value
except Exception as e: result['cdp_error']=str(e)
print(json.dumps(result))
"""
    call('shell',{'command':'python3','args':['-c',code],'label':'Check isolated server boundaries without printing secrets'},'boundary-check')
