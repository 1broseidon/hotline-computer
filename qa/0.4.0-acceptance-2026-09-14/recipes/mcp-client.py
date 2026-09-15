import base64, datetime, json, pathlib, sys, time, urllib.request
ROOT=pathlib.Path('/tmp/toad-acceptance-040')
URL='http://127.0.0.1:18787/mcp'
HEADERS={'Authorization':'Bearer '+(ROOT/'token').read_text().strip(),'Content-Type':'application/json','Accept':'application/json, text/event-stream','X-Computer-Holder':'acceptance-qa'}
def rpc(payload,session=None):
    headers=dict(HEADERS)
    if session: headers['Mcp-Session-Id']=session
    req=urllib.request.Request(URL,data=json.dumps(payload).encode(),headers=headers)
    with urllib.request.urlopen(req,timeout=100) as response:
        sid=response.headers.get('Mcp-Session-Id')
        if response.status==202: return None,sid
        if 'text/event-stream' in response.headers.get('Content-Type',''):
            for line in response:
                if line.startswith(b'data:') and line[5:].strip():
                    result=json.loads(line[5:])
                    if result.get('id')==payload.get('id'): return result,sid
        return json.loads(response.read()),sid

def call(name,args):
    init,sid=rpc({'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':'2025-03-26','capabilities':{},'clientInfo':{'name':'toad-acceptance','version':'1'}}})
    rpc({'jsonrpc':'2.0','method':'notifications/initialized'},sid)
    started=time.monotonic()
    result,_=rpc({'jsonrpc':'2.0','id':2,'method':'tools/call','params':{'name':name,'arguments':args}},sid)
    record={'time':datetime.datetime.now(datetime.timezone.utc).isoformat(),'tool':name,'args':args,'duration':round(time.monotonic()-started,3),'response':result}
    with (ROOT/'trace.jsonl').open('a') as f: f.write(json.dumps(record)+'\n')
    try: urllib.request.urlopen(urllib.request.Request(URL,method='DELETE',headers={**HEADERS,'Mcp-Session-Id':sid}),timeout=5).close()
    except Exception: pass
    return result
if __name__=='__main__':
    name=sys.argv[1]; args=json.loads(sys.argv[2]) if len(sys.argv)>2 else json.load(sys.stdin)
    result=call(name,args)
    for i,item in enumerate(result.get('result',{}).get('content',[])):
        if item.get('type')=='image':
            path=ROOT/'artifacts'/('capture-'+str(time.time_ns())+'.png');path.write_bytes(base64.b64decode(item['data']));item.clear();item.update(type='saved_image',path=str(path))
    print(json.dumps(result,indent=2))
