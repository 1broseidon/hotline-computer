"""Independent stdlib MCP client used for the agent audit; no Hotline adapter."""
import base64
import json
import os
from pathlib import Path
import sys
import time
import urllib.request

OUT = Path(__file__).resolve().parent
BASE = os.getenv('AUDIT_URL', 'http://127.0.0.1:18787')
TOKEN = next(line.split('=', 1)[1] for line in Path(os.getenv('AUDIT_TOKEN_FILE', '/tmp/hotline-agent-audit.env')).read_text().splitlines() if line.startswith('HOTLINE_COMPUTER_TOKEN='))


class Client:
    def __init__(self, name='audit', holder=None, version='2025-03-26'):
        self.name, self.sequence, self.session = name, 0, None
        self.headers = {'Authorization': 'Bearer ' + TOKEN, 'Content-Type': 'application/json', 'Accept': 'application/json, text/event-stream'}
        if holder is not None:
            self.headers['X-Computer-Holder'] = holder
        self.initialized = self.rpc('initialize', {'protocolVersion': version, 'capabilities': {}, 'clientInfo': {'name': name, 'version': '1'}})
        self.headers['MCP-Protocol-Version'] = self.initialized['result']['protocolVersion']
        self.rpc('notifications/initialized', {}, notification=True)

    def rpc(self, method, params, notification=False):
        self.sequence += 1
        body = {'jsonrpc': '2.0', 'method': method, 'params': params}
        if not notification:
            body['id'] = self.sequence
        headers = dict(self.headers)
        if self.session:
            headers['Mcp-Session-Id'] = self.session
        request = urllib.request.Request(BASE + '/mcp', data=json.dumps(body).encode(), headers=headers)
        with urllib.request.urlopen(request, timeout=75) as response:
            self.session = response.headers.get('Mcp-Session-Id') or self.session
            if response.status == 202:
                return None
            if 'text/event-stream' in response.headers.get('Content-Type', ''):
                for line in response:
                    if line.startswith(b'data:') and line[5:].strip():
                        item = json.loads(line[5:])
                        if item.get('id') == body.get('id'):
                            return item
                raise RuntimeError('SSE ended before response')
            return json.load(response)

    def call(self, tool, args, label=None, print_result=True):
        label = label or f'{self.name}-{time.time_ns()}-{tool}'
        started = time.monotonic()
        result = self.rpc('tools/call', {'name': tool, 'arguments': args})
        elapsed = round(time.monotonic() - started, 3)
        safe = json.loads(json.dumps(result))
        for i, block in enumerate(safe.get('result', {}).get('content', [])):
            if block.get('type') == 'image':
                path = OUT / f'{label}-{i}.png'
                path.write_bytes(base64.b64decode(block.pop('data')))
                block['saved_path'] = str(path)
        record = {'label': label, 'client': self.name, 'tool': tool, 'args': args, 'seconds': elapsed, **safe}
        (OUT / f'{label}.json').write_text(json.dumps(record, indent=2))
        with (OUT / 'trace.jsonl').open('a') as log:
            log.write(json.dumps(record) + '\n')
        if print_result:
            print(json.dumps(record, ensure_ascii=False))
        blocks = result.get('result', {}).get('content', [])
        text = '\n'.join(b.get('text', '') for b in blocks if b.get('type') == 'text')
        try:
            return json.loads(text)
        except ValueError:
            return text


if __name__ == '__main__':
    client = Client(holder='audit')
    if sys.argv[1] == 'init':
        print(json.dumps(client.initialized))
        listing = client.rpc('tools/list', {})
        (OUT / 'tools.json').write_text(json.dumps(listing, indent=2))
        print(json.dumps({'tools': [t['name'] for t in listing['result']['tools']], 'schema_bytes': len(json.dumps(listing))}))
    else:
        client.call(sys.argv[1], json.loads(sys.argv[2]), sys.argv[3] if len(sys.argv) > 3 else None)
