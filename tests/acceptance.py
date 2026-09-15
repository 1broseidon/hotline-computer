#!/usr/bin/env python3
"""Run against an actual fresh image; retain results and screenshots for release review."""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import re
import subprocess
import time
import urllib.request
import uuid


class Computer:
    def __init__(self, url, token, output):
        self.url = url.rstrip('/') + '/mcp'
        self.output = output
        self.headers = {'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json', 'Accept': 'application/json, text/event-stream', 'X-Computer-Holder': 'release-acceptance'}
        self.session = None
        self.sequence = 0
        self.run_id = uuid.uuid4().hex[:12]
        self.rpc('initialize', {'protocolVersion': '2025-03-26', 'capabilities': {}, 'clientInfo': {'name': 'release-acceptance', 'version': '1'}})
        self.rpc('notifications/initialized', {}, notification=True)

    def rpc(self, method, params, notification=False):
        self.sequence += 1
        payload = {'jsonrpc': '2.0', 'method': method, 'params': params}
        if not notification:
            payload['id'] = self.sequence
        headers = dict(self.headers)
        if self.session:
            headers['Mcp-Session-Id'] = self.session
        request = urllib.request.Request(self.url, data=json.dumps(payload).encode(), headers=headers)
        with urllib.request.urlopen(request, timeout=75) as response:
            self.session = response.headers.get('Mcp-Session-Id') or self.session
            if response.status == 202:
                return None
            if 'text/event-stream' in response.headers.get('Content-Type', ''):
                for line in response:
                    if line.startswith(b'data:') and line[5:].strip():
                        return json.loads(line[5:])
            return json.load(response)

    def call(self, name, args, error=False):
        started = time.monotonic()
        envelope = self.rpc('tools/call', {'name': name, 'arguments': args})
        result = envelope.get('result', {})
        with (self.output / 'trace.jsonl').open('a') as log:
            # Clip screenshot bytes; clipboard payloads are exercised separately.
            safe = {**result, 'content': [item if item.get('type') != 'image' else {'type': 'image'} for item in result.get('content', [])]}
            log.write(json.dumps({'tool': name, 'args': args, 'elapsed_ms': round((time.monotonic()-started)*1000, 1), 'result': safe}) + '\n')
        assert bool(result.get('isError')) == error, envelope
        if error:
            return result
        content = result['content']
        text = '\n'.join(item.get('text', '') for item in content if item.get('type') == 'text')
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            return text

    def done(self, job, timeout=60):
        deadline = time.monotonic() + timeout
        while job['state'] == 'running' and time.monotonic() < deadline:
            job = self.call('shell', {'action': 'wait', 'job_id': job['id'], 'wait_ms': 1000})
        assert job['state'] == 'exited' and job['exit_code'] == 0, self.call('shell', {'action': 'read', 'job_id': job['id']})
        return job

    def screenshot(self, name):
        path = '/home/agent/qa/screenshots/' + self.run_id + '-' + name
        # files put creates the parent; capture then writes the original-resolution PNG.
        self.call('files', {'action': 'put', 'path': path, 'content': ''})
        self.call('capture', {'mode': 'png', 'path': path})
        result = self.rpc('tools/call', {'name': 'files', 'arguments': {'action': 'get', 'path': path}})['result']
        assert not result.get('isError'), result
        content = '\n'.join(item.get('text', '') for item in result['content'])
        assert content.startswith('encoding=base64\n'), content[:100]
        (self.output / name).write_bytes(base64.b64decode(content.split('\n', 2)[2]))


class ToadComputer(Computer):
    """Use Toad's compiled MCP adapter so all cases also exercise the integration."""
    def __init__(self, url, token_file, output, executable):
        self.output = output
        self.run_id = uuid.uuid4().hex[:12]
        self.process = subprocess.Popen([str(executable), url, str(token_file)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)

    def rpc(self, method, params, notification=False):
        assert method == 'tools/call'
        self.process.stdin.write(json.dumps(params) + '\n')
        self.process.stdin.flush()
        line = self.process.stdout.readline()
        if not line:
            raise RuntimeError('Toad acceptance adapter closed its output')
        return {'result': json.loads(line)}

    def close(self):
        self.process.stdin.close()
        assert self.process.wait(timeout=10) == 0


FORM = '''<!doctype html><html><head><meta charset="utf-8"><title>Toad release acceptance</title>
<style>body{font:18px system-ui;max-width:780px;margin:40px auto;background:#f7f6f1;color:#202420}h1{font-size:30px}label{display:block;margin:14px 0}input,select,textarea,button{font:inherit;padding:8px;border:1px solid #aaa;border-radius:5px}button{background:#215c36;color:white}#result{padding:20px;background:#d5f5dd}small{color:#555}</style></head>
<body><h1>Release acceptance · browser workflow</h1><p>Complete the form, review, and submit.</p>
<form id="form"><section id="one"><label>Name <input id="name" required></label><label>Email <input id="email" type="email" required></label><label>Date <input id="date" type="date" required></label>
<label>Languages <select id="languages" multiple required><option value="python">Python</option><option value="go">Go</option><option value="rust">Rust</option><option value="disabled" disabled>Unavailable</option></select></label>
<label><input id="agree" type="checkbox" required>Agree to test</label><label>Notes <textarea id="notes"></textarea></label><button id="next" type="button" onclick="if(form.reportValidity()){one.hidden=true;two.hidden=false}">Review</button></section>
<section id="two" hidden><h2>Review your submission</h2><p>All values remain available before confirmation.</p><button type="submit">Submit</button></section></form>
<div id="result" hidden></div><hr><small>Negative checks</small><label>Read only <input id="readonly" value="unchanged" readonly></label><label>Disabled <input id="disabled" disabled></label><label>Controlled <input id="controlled" oninput="this.value='fixed'"></label><input id="hidden" value="secret-hidden" type="hidden"><input type="password" value="secret-password">
<script>form.onsubmit=e=>{e.preventDefault();result.hidden=false;result.textContent='Submitted: '+JSON.stringify({name:document.getElementById('name').value,email:email.value,date:date.value,languages:[...languages.selectedOptions].map(o=>o.value),agree:agree.checked,notes:notes.value});form.hidden=true}</script></body></html>'''


def browser(c):
    c.call('files', {'action': 'put', 'path': '/home/agent/qa/forms.html', 'content': FORM})
    c.call('browser', {'action': 'navigate', 'url': 'file:///home/agent/qa/forms.html'})
    snapshot = c.call('browser', {'action': 'text'})
    assert 'secret-hidden' not in snapshot and 'secret-password' not in snapshot
    assert 'readonly' in snapshot and 'disabled' in snapshot
    refs = c.call('browser', {'action': 'eval', 'js': "Object.fromEntries([...document.querySelectorAll('[id][data-toad-ref]')].map(e=>[e.id,e.dataset.toadRef]))"})
    for key, text in [('name', 'Agent QA'), ('email', 'qa@example.test'), ('date', '2026-09-15'), ('notes', 'Unicode: café 🐸\nSecond line')]:
        result = c.call('browser', {'action': 'fill', 'ref': refs[key], 'text': text})
        assert result['value'] == text, result
    result = c.call('browser', {'action': 'select', 'ref': refs['languages'], 'values': ['python', 'rust']})
    assert result['value'] == ['python', 'rust']
    c.call('browser', {'action': 'check', 'ref': refs['agree']})
    for key in ['readonly', 'disabled', 'controlled']:
        c.call('browser', {'action': 'fill', 'ref': refs[key], 'text': 'must not claim success'}, error=True)
    c.call('browser', {'action': 'select', 'ref': refs['languages'], 'values': ['missing']}, error=True)
    c.call('browser', {'action': 'click_ref', 'ref': refs['next']})
    snapshot = c.call('browser', {'action': 'text'})
    submit = re.search(r'\[(e\d+)\] \[button\] Submit', snapshot).group(1)
    c.call('browser', {'action': 'click_ref', 'ref': submit})
    result = c.call('browser', {'action': 'text'})
    assert 'Submitted:' in result and '2026-09-15' in result and 'Agent QA' in result, result
    c.screenshot('02-browser-submission.png')


def jobs(c):
    job = c.call('shell', {'action': 'start', 'command': 'bash', 'args': ['-c', 'printf ready; read answer; printf " received:%s" "$answer"'], 'pty': True, 'request_id': 'acceptance-stdin-' + c.run_id})
    repeat = c.call('shell', {'action': 'start', 'command': 'bash', 'args': ['-c', 'printf ready; read answer; printf " received:%s" "$answer"'], 'pty': True, 'request_id': 'acceptance-stdin-' + c.run_id})
    assert job['id'] == repeat['id']
    c.call('shell', {'action': 'write', 'job_id': job['id'], 'text': 'hello\n'})
    c.done(job)
    assert 'received:hello' in c.call('shell', {'action': 'read', 'job_id': job['id']})['output']
    result = c.call('shell', {'command': 'bash', 'args': ['-c', 'printf partial; sleep 10'], 'timeout': 1})
    assert result['state'] == 'timed_out' and result['stdout'] == 'partial'
    job = c.call('shell', {'action': 'start', 'command': 'bash', 'args': ['-c', 'for i in $(seq 1 200); do echo tick:$i; sleep .1; done'], 'label': 'Observer close/reopen acceptance'})
    windows = c.call('windows', {'action': 'list'})
    terminal = next(w for w in windows if 'toadterminal' in w['class'].lower())
    c.call('windows', {'action': 'close', 'window_id': terminal['id']})
    assert c.call('shell', {'action': 'status', 'job_id': job['id']})['state'] == 'running'
    started = time.monotonic()
    c.call('shell', {'action': 'show'})
    deadline = time.monotonic()+3
    while time.monotonic()<deadline:
        if any('toadterminal' in w['class'].lower() for w in c.call('windows', {'action': 'list'})):
            break
        time.sleep(.03)
    else:
        raise AssertionError('observer did not reopen')
    c.output.joinpath('observer-timing.json').write_text(json.dumps({'reopen_ms': (time.monotonic()-started)*1000}))
    c.screenshot('03-running-job-observer.png')
    assert c.call('shell', {'action': 'cancel', 'job_id': job['id']})['state'] == 'cancelled'


def artifacts(c):
    script = '#!/bin/bash\nprintf "installer argument: %s\\n" "$1"\n'
    c.call('files', {'action': 'put', 'path': '/home/agent/qa/install-fixture.sh', 'content': script})
    job = c.call('files', {'action': 'run', 'path': '/home/agent/qa/install-fixture.sh', 'sha256': hashlib.sha256(script.encode()).hexdigest(), 'args': ['value with spaces']})
    c.done(job)
    assert 'installer argument: value with spaces' in c.call('shell', {'action': 'read', 'job_id': job['id']})['output']
    bad = c.call('files', {'action': 'run', 'path': '/home/agent/qa/install-fixture.sh', 'sha256': '0'*64})
    bad = c.call('shell', {'action': 'wait', 'job_id': bad['id'], 'wait_ms': 5000})
    assert bad['state'] == 'failed'



def execute(c, command, args, cwd=None, timeout=120, label=None):
    job = c.call('shell', {'action': 'start', 'command': command, 'args': args, 'cwd': cwd or '/home/agent', 'timeout': timeout, 'label': label or command})
    c.done(job, timeout+10)
    return c.call('shell', {'action': 'read', 'job_id': job['id'], 'max_output': 1048576})['output']


def prepare(c, profile, workspace):
    started = time.monotonic()
    result = c.call('state', {'action': 'prepare', 'name': profile, 'workspace': workspace})
    if not result['ready']:
        c.done(result['job'], 1200)
    return {'profile': profile, 'cached': result['cached'], 'seconds': time.monotonic()-started}


def workspaces(c):
    root = '/home/agent/qa/catalog-' + c.run_id
    fixtures = {
        'python': ('main.py', 'import sqlite3, ssl\nassert sqlite3.connect(":memory:").execute("select 6*7").fetchone()[0]==42\nprint("python fixture: 42")\n', ['python3', 'main.py']),
        'go': ('main.go', 'package main\nimport "fmt"\nfunc main(){fmt.Println("go fixture: 42")}\n', ['go', 'run', 'main.go']),
        'node': ('main.js', 'const assert = require("node:assert/strict"); assert.equal(6*7,42); console.log("node fixture: 42");\n', ['node', 'main.js']),
        'rust': ('main.rs', 'fn main(){assert_eq!(6*7,42); println!("rust fixture: 42");}\n', ['bash', '-c', 'rustc main.rs -o fixture && ./fixture']),
    }
    timings = []
    for profile, (filename, source, command) in fixtures.items():
        workspace = root + '/' + profile
        c.call('files', {'action': 'put', 'path': workspace + '/' + filename, 'content': source})
        timings.append(prepare(c, profile, workspace))
        output = execute(c, command[0], command[1:], cwd=workspace)
        assert profile + ' fixture: 42' in output, output
        # A second workspace inherits the cached environment without a shell hook.
        samples = []
        for n in range(5):
            samples.append(prepare(c, profile, workspace + '/second-' + str(n)))
        assert all(sample['cached'] for sample in samples), samples
        timings.extend(samples)
    # A compiler must accept dependency headers outside the project's own directory.
    output = execute(c, 'bash', ['-c', 'set -e; mkdir -p ../headers; printf "#define ANSWER 42\\n" > ../headers/fixture.h; printf "#include <fixture.h>\\nint main(){return ANSWER != 42;}\\n" > main.c; cc -O0 -I"$PWD/../headers" main.c -o c-fixture; ./c-fixture; cmake --version; perl -e \'print "native dependencies: 42\\n"\''], cwd=root+'/rust')
    assert 'native dependencies: 42' in output
    c.output.joinpath('catalog-timings.json').write_text(json.dumps(timings, indent=2))


def ketch(c):
    arch = c.call('state', {'action': 'info'})['architecture']
    asset_arch, checksum = {
        'aarch64': ('arm64', '61e0a7ef16350534c586d7e2755a490333334fc40bdeac01c092bf77bf030b8c'),
        'x86_64': ('x86_64', 'e84b7667212aeb8b7bd9a92bc2a25f1c31185e2c6fba5a4cd97c2ace5a504c82'),
    }[arch]
    root = '/home/agent/qa/ketch-' + c.run_id
    archive = root + '/ketch.tar.gz'
    job = c.call('files', {'action': 'download', 'repo': '1broseidon/ketch', 'version': 'v0.16.2', 'asset': 'ketch_0.16.2_linux_' + asset_arch + '.tar.gz', 'path': archive, 'sha256': checksum})
    c.done(job, 180)
    job = c.call('files', {'action': 'extract', 'path': archive, 'destination': root + '/bin'})
    c.done(job)
    assert '0.16.2' in execute(c, root+'/bin/ketch', ['--version'])
    c.call('files', {'action': 'put', 'path': root+'/site/index.html', 'content': '<title>Deterministic Ketch QA</title><main><h1>Local release fixture</h1><p>Answer: forty two</p></main>'})
    server = c.call('shell', {'action': 'start', 'command': 'python3', 'args': ['-m', 'http.server', '8081', '--bind', '127.0.0.1', '--directory', root+'/site'], 'label': 'Deterministic scrape fixture'})
    try:
        execute(c, 'python3', ['-c', 'import urllib.request,time\nfor _ in range(100):\n try: urllib.request.urlopen("http://127.0.0.1:8081"); break\n except OSError: time.sleep(.05)\nelse: raise RuntimeError("fixture server did not start")'])
        result = execute(c, root+'/bin/ketch', ['scrape', 'http://127.0.0.1:8081', '--json'])
        assert 'forty two' in result and 'Deterministic Ketch QA' in result, result
        c.output.joinpath('ketch-scrape.json').write_text(result)
    finally:
        c.call('shell', {'action': 'cancel', 'job_id': server['id']})
    c.screenshot('04-ketch-installation.png')


def durability(c):
    timings = []
    for n in range(70):
        started = time.monotonic()
        job = c.call('shell', {'action': 'start', 'command': 'true', 'label': 'Churn '+str(n)})
        timings.append((time.monotonic()-started)*1000)
        c.done(job)
    records = c.call('shell', {'action': 'list'})
    assert len(records) <= 64, len(records)
    missing = c.call('shell', {'action': 'start', 'command': '/no-such-command'})
    assert missing['state'] == 'failed' and missing['exit_code'] == 127
    failed = c.call('shell', {'command': 'bash', 'args': ['-c', 'echo expected failure >&2; exit 23']})
    assert failed['exit_code'] == 23 and 'expected failure' in failed['stderr']
    large = c.call('shell', {'command': 'python3', 'args': ['-c', 'print("x"*5000000)'], 'max_output': 1000})
    assert large['truncated'] and len(large['stdout']) <= 1000
    job = c.call('shell', {'action': 'start', 'command': 'bash', 'args': ['-c', 'sleep 120 & child=$!; echo $child; wait'], 'label': 'Descendant cancellation'})
    deadline = time.monotonic()+3
    while True:
        output = c.call('shell', {'action': 'read', 'job_id': job['id']})['output'].strip()
        if output: break
        assert time.monotonic()<deadline
        time.sleep(.03)
    child = int(output)
    c.call('shell', {'action': 'cancel', 'job_id': job['id']})
    execute(c, 'python3', ['-c', f'import os,time\nfor _ in range(100):\n if not os.path.exists("/proc/{child}"): break\n time.sleep(.02)\nelse: raise RuntimeError("orphan or zombie survived cancellation")'])
    execute(c, 'python3', ['-c', 'import pathlib\nzombies=[]\nfor p in pathlib.Path("/proc").glob("[0-9]*/stat"):\n try:\n  if p.read_text().rsplit(")",1)[1].split()[0]=="Z": zombies.append(str(p))\n except FileNotFoundError: pass\nassert not zombies,zombies\nprint("No zombie processes")'])
    c.output.joinpath('job-acknowledgement-ms.json').write_text(json.dumps({'samples': timings, 'p50': sorted(timings)[len(timings)//2], 'p95': sorted(timings)[int(len(timings)*.95)], 'max': max(timings)}, indent=2))


TOAD_REVISION = 'f3c17b78d85b4cc235455862d9c9241579157b39'


def native(c):
    root = '/home/agent/qa/toad-' + c.run_id
    execute(c, 'git', ['clone', '--filter=blob:none', 'https://github.com/1broseidon/toad.git', root], timeout=300)
    execute(c, 'git', ['checkout', '--detach', TOAD_REVISION], cwd=root)
    timing = prepare(c, 'rust-tauri', root)
    c.output.joinpath('native-environment.json').write_text(json.dumps(timing, indent=2))
    script = 'set -euo pipefail\ncd ui\nbun install --frozen-lockfile\nbun run build\ncd ..\ncargo build --locked -p toad-desktop --features tauri/custom-protocol\n'
    c.call('files', {'action': 'put', 'path': root+'/qa-build.sh', 'content': script})
    job = c.call('files', {'action': 'run', 'path': root+'/qa-build.sh', 'cwd': root, 'sha256': hashlib.sha256(script.encode()).hexdigest()})
    # Desktop work remains responsive during the native build.
    browser(c)
    c.screenshot('05-browsing-during-native-build.png')
    c.done(job, 2400)
    c.output.joinpath('native-build-output.txt').write_text(c.call('shell', {'action': 'read', 'job_id': job['id'], 'max_output': 1048576})['output'])
    app = c.call('shell', {'action': 'start', 'command': root+'/target/debug/toad-desktop', 'cwd': root, 'env': {'TOAD_DATA_DIR': root+'/qa-data'}, 'label': 'Toad native screen acceptance'})
    native_screens(c, app)


def native_screens(c, app):
    deadline = time.monotonic()+30
    while time.monotonic()<deadline:
        assert c.call('shell', {'action': 'status', 'job_id': app['id']})['state'] == 'running', c.call('shell', {'action': 'read', 'job_id': app['id']})
        windows = c.call('windows', {'action': 'list'})
        native_windows = [w for w in windows if w.get('pid') == app['pid'] and 'toadterminal' not in w['class'].lower()]
        if native_windows: break
        time.sleep(.2)
    else: raise AssertionError('Toad did not map a native window')
    c.call('windows', {'action': 'focus', 'window_id': native_windows[0]['id']})
    c.call('windows', {'action': 'maximize', 'window_id': native_windows[0]['id']})
    window_id = native_windows[0]['id']

    def tree_with(label):
        deadline = time.monotonic()+10
        while time.monotonic()<deadline:
            capture = str(c.call('capture', {}))
            marker = '[' + window_id + ' '
            tree = capture.split(marker, 1)[1].split('\n[0x', 1)[0] if marker in capture else ''
            if label in tree:
                return tree
            time.sleep(.1)
        raise AssertionError('Native Toad tree did not expose ' + label + ': ' + tree)

    def click_label(tree, label):
        match = re.search(r'\[button\] ' + re.escape(label) + r' (-?\d+),(-?\d+) (\d+)x(\d+)', tree)
        assert match, tree
        x, y, width, height = map(int, match.groups())
        c.call('input', {'action': 'click', 'x': x+width//2, 'y': y+height//2, 'settle_ms': 200})

    tree = tree_with('[button] New teammate')
    assert 'Submitted:' not in tree, 'browser content was assigned to the native app'
    c.screenshot('06-toad-native-screen.png')
    click_label(tree, 'Settings')
    tree = tree_with('[button] Computer')
    c.screenshot('07-toad-settings.png')
    click_label(tree, 'Computer')
    tree = tree_with('Desktop image')
    c.screenshot('08-toad-computer-settings.png')
    c.output.joinpath('native-accessibility.txt').write_text(tree)
    c.output.joinpath('native-window.json').write_text(json.dumps(native_windows, indent=2))
    c.call('shell', {'action': 'cancel', 'job_id': app['id']})

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--url', required=True)
    parser.add_argument('--token-file', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--toad-bridge', type=Path)
    parser.add_argument('--suite', choices=['quick', 'full', 'workspaces', 'native'], default='quick')
    options = parser.parse_args()
    options.output.mkdir(parents=True, exist_ok=True)
    c = ToadComputer(options.url, options.token_file, options.output, options.toad_bridge) if options.toad_bridge else Computer(options.url, options.token_file.read_text().strip(), options.output)
    report = {'info': c.call('state', {'action': 'info'}), 'cases': []}
    guide = c.call('state', {'action': 'guide'})
    assert hashlib.sha256(guide['skill'].encode()).hexdigest() == guide['sha256']
    cases = [('browser forms', browser), ('managed jobs and observer', jobs), ('verified script execution', artifacts)]
    if options.suite == 'full':
        cases += [('catalog workspaces', workspaces), ('Ketch installation and scraping', ketch), ('job durability and responsiveness', durability), ('native Toad build and screens', native)]
    elif options.suite == 'workspaces':
        cases = [('catalog workspaces', workspaces), ('Ketch installation and scraping', ketch), ('job durability and responsiveness', durability)]
    elif options.suite == 'native':
        cases = [('native Toad build and screens', native)]
    for name, case in cases:
        started = time.monotonic()
        try:
            case(c)
            result = {'name': name, 'passed': True}
        except Exception as error:
            result = {'name': name, 'passed': False, 'error': str(error)}
            try:
                c.screenshot('failure-' + name.replace(' ', '-') + '.png')
            except Exception:
                pass
        result['seconds'] = round(time.monotonic()-started, 3)
        report['cases'].append(result)
        print(json.dumps(result), flush=True)
        (options.output / 'results.json').write_text(json.dumps(report, indent=2))
    if isinstance(c, ToadComputer):
        c.close()
    return 0 if all(case['passed'] for case in report['cases']) else 1


if __name__ == '__main__':
    raise SystemExit(main())
