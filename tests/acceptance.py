#!/usr/bin/env python3
"""Run against an actual fresh image; retain results and screenshots for release review."""
import argparse
import base64
import hashlib
import io
import json
from pathlib import Path
import re
import subprocess
import time
import urllib.request
import uuid
import zipfile

from PIL import Image, ImageChops


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

    def screenshot(self, name, settle_ms=100):
        path = '/home/agent/qa/screenshots/' + self.run_id + '-' + name
        # files put creates the parent; capture then writes the original-resolution PNG.
        self.call('files', {'action': 'put', 'path': path, 'content': ''})
        self.call('capture', {'mode': 'png', 'path': path, 'settle_ms': settle_ms})
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
    assert c.call('browser', {'action': 'eval', 'js': 'document.body.focus()'}) is None
    c.call('browser', {'action': 'eval', 'js': 'throw new Error("expected evaluation failure")'}, error=True)
    snapshot = c.call('browser', {'action': 'text'})
    assert 'secret-hidden' not in snapshot and 'secret-password' not in snapshot
    assert 'readonly' in snapshot and 'disabled' in snapshot
    refs = c.call('browser', {'action': 'eval', 'js': "Object.fromEntries([...document.querySelectorAll('[id][data-toad-ref]')].map(e=>[e.id,e.dataset.toadRef]))"})
    for key, text in [('name', 'Agent QA'), ('email', 'qa@example.test'), ('date', '2026-09-15'), ('notes', 'Unicode: café 🐸\nSecond line')]:
        result = c.call('browser', {'action': 'fill', 'ref': refs[key], 'text': text})
        assert result['value'] == text, result
    for incorrect in [{'value': 'wrong argument'}, {}]:
        failure = c.call('browser', {'action': 'fill', 'ref': refs['name'], **incorrect}, error=True)
        assert 'fill requires text' in str(failure), failure
        assert c.call('browser', {'action': 'eval', 'js': 'document.getElementById("name").value'}) == 'Agent QA'
    assert c.call('browser', {'action': 'fill', 'ref': refs['name'], 'text': ''})['value'] == ''
    c.call('browser', {'action': 'fill', 'ref': refs['name'], 'text': 'Agent QA'})
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


def browser_refs(c):
    c.call('browser', {'action': 'text'})
    return c.call('browser', {'action': 'eval', 'js': "Object.fromEntries([...document.querySelectorAll('[data-toad-ref]')].map(e=>[e.id||e.name,e.dataset.toadRef]))"})


def wizard(c):
    fixture = Path(__file__).with_name('fixtures').joinpath('forms.html').read_text()
    c.call('files', {'action':'put', 'path':'/home/agent/qa/wizard.html', 'content':fixture})
    c.call('files', {'action':'put', 'path':'/home/agent/qa/notes.txt', 'content':'Synthetic acceptance attachment'})
    c.call('browser', {'action':'navigate', 'url':'file:///home/agent/qa/wizard.html'})
    refs = browser_refs(c)
    for field, value in [('applicant','Agent QA'),('contact','qa@example.test')]:
        c.call('browser', {'action':'fill', 'ref':refs[field], 'text':value})
    c.call('browser', {'action':'click_ref', 'ref':refs['next1']})
    refs = browser_refs(c)
    c.call('browser', {'action':'select', 'ref':refs['project'], 'value':'Team'})
    refs = browser_refs(c)
    c.call('browser', {'action':'fill', 'ref':refs['org'], 'text':'Toad QA'})
    c.call('browser', {'action':'fill', 'ref':refs['date'], 'text':'2026-09-15'})
    c.call('browser', {'action':'select', 'ref':refs['platforms'], 'values':['Linux','macOS']})
    c.call('browser', {'action':'upload', 'ref':refs['attachment'], 'path':'/home/agent/qa/notes.txt'})
    c.call('browser', {'action':'click_ref', 'ref':refs['back1']})
    refs = browser_refs(c)
    assert c.call('browser', {'action':'eval', 'js':'document.getElementById("applicant").value'}) == 'Agent QA'
    c.call('browser', {'action':'click_ref', 'ref':refs['next1']})
    refs = browser_refs(c)
    c.call('browser', {'action':'click_ref', 'ref':refs['next2']})
    snapshot = c.call('browser', {'action':'text'})
    assert 'Review application' in snapshot and 'notes.txt' in snapshot and 'Toad QA' in snapshot, snapshot
    refs = browser_refs(c)
    c.call('browser', {'action':'click_ref', 'ref':refs['back2']})
    refs = browser_refs(c)
    c.call('browser', {'action':'click_ref', 'ref':refs['next2']})
    refs = browser_refs(c)
    c.call('browser', {'action':'check', 'ref':refs['terms']})
    c.screenshot('02b-browser-review.png')
    c.call('browser', {'action':'click_ref', 'ref':refs['submit']})
    c.call('wait', {'text':'Application QA-040 accepted', 'timeout':10})
    values = c.call('browser', {'action':'eval', 'js':'JSON.parse(localStorage.getItem("complex-result"))'})
    assert values == {'name':'Agent QA','email':'qa@example.test','project':'Team','organization':'Toad QA','date':'2026-09-15','file':'notes.txt','platforms':['Linux','macOS']}, values
    c.screenshot('02c-browser-wizard-submitted.png')


def public_form(c):
    c.call('browser', {'action':'navigate', 'url':'https://www.selenium.dev/selenium/web/web-form.html'})
    refs = browser_refs(c)
    c.call('browser', {'action':'fill', 'ref':refs['my-text-id'], 'text':'Synthetic Toad QA'})
    c.call('browser', {'action':'fill', 'ref':refs['my-textarea'], 'text':'Public browser form acceptance'})
    c.call('browser', {'action':'select', 'ref':refs['my-select'], 'value':'2'})
    c.call('browser', {'action':'upload', 'ref':refs['my-file'], 'path':'/home/agent/qa/notes.txt'})
    c.call('browser', {'action':'fill', 'ref':refs['my-readonly'], 'text':'unchanged'}, error=True)
    snapshot = c.call('browser', {'action':'text'})
    submit = re.search(r'\[(e\d+)\] \[button\] Submit', snapshot).group(1)
    c.call('browser', {'action':'click_ref', 'ref':submit})
    c.call('wait', {'text':'Received!', 'timeout':10})
    c.screenshot('02d-public-form-received.png')


def password_prompt(c):
    # A form with a password, filled the way a person would, by pointer and
    # keys, must not leave Chromium's "Save password?" bubble over the page.
    c.call('browser', {'action':'navigate', 'url':'https://www.selenium.dev/selenium/web/web-form.html'})
    fields = c.call('browser', {'action':'eval', 'js':"""(() => {
      const off = { x: window.screenX, y: window.screenY + (window.outerHeight - window.innerHeight) };
      const at = (selector) => { const r = document.querySelector(selector).getBoundingClientRect(); return [Math.round(off.x + r.left + r.width/2), Math.round(off.y + r.top + r.height/2)]; };
      return JSON.stringify({ text: at('#my-text-id'), password: at('input[name=my-password]'), submit: at('button[type=submit]') }); })()"""})
    fields = json.loads(fields) if isinstance(fields, str) else fields
    c.call('input', {'action':'click','x':fields['text'][0],'y':fields['text'][1]})
    c.call('input', {'action':'type','text':'Synthetic Toad QA'})
    c.call('input', {'action':'click','x':fields['password'][0],'y':fields['password'][1]})
    c.call('input', {'action':'type','text':'not-a-real-secret'})
    c.call('input', {'action':'click','x':fields['submit'][0],'y':fields['submit'][1]})
    c.call('wait', {'text':'Received!', 'timeout':10})
    time.sleep(1)
    capture = str(c.call('capture', {}))
    c.screenshot('02e-password-form-submitted.png')
    assert 'Save password' not in capture and 'password manager' not in capture.lower(), capture[:2000]


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
    samples = []
    for _ in range(12):
        windows = c.call('windows', {'action': 'list'})
        terminal = next(w for w in windows if 'toadterminal' in w['class'].lower())
        c.call('windows', {'action': 'close', 'window_id': terminal['id']})
        assert c.call('shell', {'action': 'status', 'job_id': job['id']})['state'] == 'running'
        started = time.monotonic()
        c.call('shell', {'action': 'show', 'job_id': job['id']})
        assert any('toadterminal' in w['class'].lower() for w in c.call('windows', {'action': 'list'})), 'observer did not reopen'
        samples.append((time.monotonic()-started)*1000)
    c.output.joinpath('observer-timing.json').write_text(json.dumps({'samples': samples, 'p50': sorted(samples)[len(samples)//2], 'p95': sorted(samples)[int(len(samples)*.95)], 'max': max(samples)}, indent=2))
    assert max(samples) < 300, samples
    shown = c.call('shell', {'action':'show','job_id':job['id']})
    daemon = shown['pid']  # PID captured by the observer service at spawn.
    c.done(c.call('shell', {'action':'start','command':'kill','args':['-KILL',str(daemon)],'label':'Observer daemon recovery fixture'}))
    assert c.call('shell', {'action':'status','job_id':job['id']})['state']=='running'
    deadline = time.monotonic()+3
    while any('toadterminal' in w['class'].lower() for w in c.call('windows',{'action':'list'})):
        assert time.monotonic()<deadline
        time.sleep(.05)
    reopened = c.call('shell', {'action':'show','job_id':job['id']})
    assert reopened['pid'] != daemon
    c.screenshot('03-running-job-observer.png')
    assert c.call('shell', {'action': 'cancel', 'job_id': job['id']})['state'] == 'cancelled'
    c.call('shell', {'action':'show', 'job_id':job['id']})
    c.screenshot('03b-completed-job-observer.png', settle_ms=200)
    c.call('shell', {'action':'show', 'job_id':'no-such-job'}, error=True)
    c.call('shell', {'action':'show'})


def bar_layout(c):
    """Where the bar drew its parts, as the desktop publishes it on the root window."""
    code = """import ast,json,subprocess
line=subprocess.check_output(['xprop','-root','_TOAD_BAR_LAYOUT'],text=True).strip()
print(json.dumps(json.loads(ast.literal_eval(line.split(' = ',1)[1]))))
"""
    return json.loads(execute(c,'python3',['-c',code],label='Bar layout lookup').strip().splitlines()[-1])


def centre(rect):
    x,y,w,h = rect
    return x+w//2, y+h//2


def desktop_job_menu(c):
    # The layout lookup is itself a job, so it runs before the job under test
    # to keep that one newest, and so at the top of the list.
    layout = bar_layout(c)
    job = c.call('shell',{'action':'start','command':'sh','args':['-c','echo "A failed job stays inspectable"; exit 7'],'label':'Inspect a completed failure'})
    job = c.call('shell',{'action':'wait','job_id':job['id'],'wait_ms':5000})
    assert job['state']=='failed' and job['exit_code']==7,job
    assert layout['height']==36 and layout['jobs'][2]>0, layout
    jobs_x, jobs_y = centre(layout['jobs'])
    popup = layout['popup']
    def row(index):
        # Row 0 opens the terminal; the newest job is row 1.
        return popup['x']+120, popup['y']+popup['pad']+popup['row']*index+popup['row']//2
    c.call('input',{'action':'click','x':jobs_x,'y':jobs_y})
    c.screenshot('03c-desktop-job-menu.png')
    c.call('input',{'action':'click','x':row(1)[0],'y':row(1)[1]})
    assert c.call('files',{'action':'get','path':'/home/agent/.toad/observer-view.json'}) == job['id']
    c.screenshot('03d-selected-failed-job.png',settle_ms=200)
    c.call('input',{'action':'click','x':jobs_x,'y':jobs_y})
    c.call('input',{'action':'click','x':row(0)[0],'y':row(0)[1]})
    assert c.call('files',{'action':'get','path':'/home/agent/.toad/observer-view.json'}) is None
    # The toad menu: the mark opens it, a letter picks, Escape closes.
    mark_x, mark_y = centre(layout['mark'])
    c.call('input',{'action':'click','x':mark_x,'y':mark_y})
    c.screenshot('03e-toad-menu.png')
    c.call('input',{'action':'key','combo':'d'})
    c.screenshot('03f-about-this-computer.png')
    c.call('input',{'action':'key','combo':'Escape'})
    c.call('input',{'action':'click','x':mark_x,'y':mark_y})
    c.call('input',{'action':'key','combo':'c'})
    c.screenshot('03g-jobs-from-menu.png')
    c.call('input',{'action':'key','combo':'Escape'})
    c.screenshot('03h-menus-closed.png')


def tray_counts(c):
    held = c.call('shell', {'action':'start','command':'sleep','args':['20'],'label':'Tray active-job fixture'})
    try:
        code = """import ast,json,subprocess,time
# Observe both properties from an independent X11 connection after startup.
time.sleep(.15)
lines=subprocess.check_output(['xprop','-root','_TOAD_JOBS_RUNNING','_TOAD_JOB_SUMMARY'],text=True).splitlines()
counts=[int(value.strip()) for value in lines[0].split(' = ',1)[1].split(',')]
jobs=json.loads(ast.literal_eval(lines[1].split(' = ',1)[1]))
assert any(j['label']=='Tray active-job fixture' and j['state']=='running' for j in jobs),jobs
running=sum(j['state']=='running' for j in jobs)
completed=sum(j['state']=='exited' and j['exit_code']==0 for j in jobs)
assert counts==[running,completed,len(jobs)-running-completed],(counts,jobs)
print('Tray counts match live jobs:',counts)
"""
        execute(c,'python3',['-c',code],label='Tray property verification')
        c.screenshot('04b-tray-active-jobs.png')
    finally:
        c.call('shell', {'action':'cancel','job_id':held['id']})


def nix_failure(c):
    home = '/home/agent/qa/nix-failure-' + c.run_id
    workspace = home + '/workspace'
    c.call('files', {'action':'put','path':workspace+'/fixture','content':''})
    job = c.call('shell', {'action':'start','command':'/usr/bin/toad-computer','args':['prepare',json.dumps({'source':'packages','packages':['python312'],'nixpkgs':c.call('state', {'action':'info'})['nixpkgs']}),workspace,home],'env':{'NIX_REMOTE':'unix:///home/agent/qa/missing-nix-daemon.sock'},'label':'Nix failure diagnostics','timeout':30})
    job = c.call('shell', {'action':'wait','job_id':job['id'],'wait_ms':30000})
    assert job['state']=='failed',job
    output = c.call('shell', {'action':'read','job_id':job['id']})['output']
    assert 'missing-nix-daemon.sock' in output and 'Nix preparation failed' in output,output
    c.output.joinpath('nix-failure.txt').write_text(output)


def artifacts(c):
    script = '#!/bin/bash\nprintf "installer argument: %s; environment: %s\\n" "$1" "$INSTALL_FIXTURE"\n'
    c.call('files', {'action': 'put', 'path': '/home/agent/qa/install-fixture.sh', 'content': script})
    job = c.call('files', {'action': 'run', 'path': '/home/agent/qa/install-fixture.sh', 'sha256': hashlib.sha256(script.encode()).hexdigest(), 'args': ['value with spaces'], 'env': {'INSTALL_FIXTURE': 'explicit environment'}})
    c.done(job)
    assert 'installer argument: value with spaces; environment: explicit environment' in c.call('shell', {'action': 'read', 'job_id': job['id']})['output']
    bad = c.call('files', {'action': 'run', 'path': '/home/agent/qa/install-fixture.sh', 'sha256': '0'*64})
    bad = c.call('shell', {'action': 'wait', 'job_id': bad['id'], 'wait_ms': 5000})
    assert bad['state'] == 'failed'
    archive = io.BytesIO()
    with zipfile.ZipFile(archive, 'w') as bundle:
        bundle.writestr('../escaped.txt', 'must not escape')
    path = '/home/agent/qa/unsafe-' + c.run_id + '.zip'
    c.call('files', {'action': 'put', 'path': path, 'encoding': 'base64', 'content': base64.b64encode(archive.getvalue()).decode()})
    extracted = c.call('files', {'action': 'extract', 'path': path, 'destination': path+'.out'})
    extracted = c.call('shell', {'action': 'wait', 'job_id': extracted['id'], 'wait_ms': 5000})
    assert extracted['state'] == 'failed'
    assert 'escapes the destination' in c.call('shell', {'action': 'read', 'job_id': extracted['id']})['output']
    c.call('files', {'action': 'get', 'path': '/home/agent/qa/escaped.txt'}, error=True)
    special = io.BytesIO()
    with zipfile.ZipFile(special, 'w') as bundle:
        entry = zipfile.ZipInfo('named-pipe')
        entry.create_system = 3
        entry.external_attr = (0o010000 | 0o600) << 16
        bundle.writestr(entry, '')
    path = '/home/agent/qa/special-' + c.run_id + '.zip'
    c.call('files', {'action':'put','path':path,'encoding':'base64','content':base64.b64encode(special.getvalue()).decode()})
    extracted = c.call('files', {'action':'extract','path':path,'destination':path+'.out'})
    extracted = c.call('shell', {'action':'wait','job_id':extracted['id'],'wait_ms':5000})
    assert extracted['state']=='failed',extracted
    assert 'special files are not supported' in c.call('shell', {'action':'read','job_id':extracted['id']})['output']




def download_failures(c):
    root = '/home/agent/qa/downloads-' + c.run_id
    server_source = """from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import time
class Handler(BaseHTTPRequestHandler):
 def do_GET(self):
  if self.path=='/missing': self.send_error(404); return
  self.send_response(200)
  length = {'/ok':12,'/partial':20,'/large':2**31,'/slow':2**22}[self.path]
  self.send_header('Content-Length',str(length)); self.end_headers()
  if self.path=='/ok': self.wfile.write(b'fixture-data')
  elif self.path=='/partial': self.wfile.write(b'partial'); self.close_connection=True
  elif self.path=='/slow': self.wfile.write(b'x'*(2**20)); self.wfile.flush(); time.sleep(120)
ThreadingHTTPServer(('127.0.0.1',8082),Handler).serve_forever()
"""
    c.call('files', {'action':'put','path':root+'/server.py','content':server_source})
    server = c.call('shell', {'action':'start','command':'python3','args':[root+'/server.py'],'label':'Artifact HTTP failure fixture'})
    try:
        execute(c,'python3',['-c','import urllib.request,time\nfor _ in range(100):\n try: urllib.request.urlopen("http://127.0.0.1:8082/ok"); break\n except OSError: time.sleep(.05)\nelse: raise RuntimeError("fixture did not start")'])
        checksum = hashlib.sha256(b'fixture-data').hexdigest()
        spec = {'action':'download','url':'http://127.0.0.1:8082/ok','path':root+'/cached','sha256':checksum}
        c.done(c.call('files',spec))
        cached = c.done(c.call('files',spec))
        assert '"cached": true' in c.call('shell',{'action':'read','job_id':cached['id']})['output']
        for endpoint, expected in [('missing','404'),('partial','incomplete'),('large','exceeds 1 GiB')]:
            job = c.call('files',{'action':'download','url':'http://127.0.0.1:8082/'+endpoint,'path':root+'/'+endpoint})
            job = c.call('shell',{'action':'wait','job_id':job['id'],'wait_ms':5000})
            assert job['state']=='failed',job
            assert expected in c.call('shell',{'action':'read','job_id':job['id']})['output']
            c.call('files',{'action':'get','path':root+'/'+endpoint},error=True)
        interrupted = c.call('files',{'action':'download','url':'http://127.0.0.1:8082/slow','path':root+'/interrupted'})
        deadline = time.monotonic()+5
        while 'Downloaded' not in c.call('shell',{'action':'read','job_id':interrupted['id']})['output']:
            assert time.monotonic()<deadline
            time.sleep(.05)
        c.call('shell',{'action':'cancel','job_id':interrupted['id']})
        c.call('files',{'action':'get','path':root+'/interrupted'},error=True)
        assert not any(entry['name'].startswith('.toad-artifact-') for entry in c.call('files',{'action':'list','path':root}))
        retried = c.done(c.call('files',{'action':'download','url':'http://127.0.0.1:8082/ok','path':root+'/interrupted','sha256':checksum}))
        assert retried['exit_code']==0
    finally:
        c.call('shell',{'action':'cancel','job_id':server['id']})


def execute(c, command, args, cwd=None, timeout=120, label=None):
    job = c.call('shell', {'action': 'start', 'command': command, 'args': args, 'cwd': cwd or '/home/agent', 'timeout': timeout, 'label': label or command})
    c.done(job, timeout+10)
    return c.call('shell', {'action': 'read', 'job_id': job['id'], 'max_output': 1048576})['output']


def prepare(c, packages, workspace, **options):
    started = time.monotonic()
    result = c.call('state', {'action': 'prepare', 'workspace': workspace, **({'packages':packages} if packages is not None else {}), **options})
    if not result['ready']:
        c.done(result['job'], 1200)
    return {'packages': packages, 'cached': result['cached'], 'seconds': time.monotonic()-started}


def workspaces(c):
    root = '/home/agent/qa/workspaces-' + c.run_id
    fixtures = {
        'python': ('main.py', 'import sqlite3, ssl\nassert sqlite3.connect(":memory:").execute("select 6*7").fetchone()[0]==42\nprint("python fixture: 42")\n', ['python3', 'main.py']),
        'go': ('main.go', 'package main\nimport "fmt"\nfunc main(){fmt.Println("go fixture: 42")}\n', ['go', 'run', 'main.go']),
        'node': ('main.js', 'const assert = require("node:assert/strict"); assert.equal(6*7,42); console.log("node fixture: 42");\n', ['node', 'main.js']),
        'rust': ('main.rs', 'fn main(){assert_eq!(6*7,42); println!("rust fixture: 42");}\n', ['bash', '-c', 'rustc main.rs -o fixture && ./fixture']),
    }
    dependencies = {'python':['python312','uv'], 'go':['go','gopls'], 'node':['nodejs','bun','pnpm'], 'rust':['rustc','cargo','clang','cmake','perl','pkg-config','openssl']}
    timings = []
    for profile, (filename, source, command) in fixtures.items():
        packages = dependencies[profile]
        workspace = root + '/' + profile
        c.call('files', {'action': 'put', 'path': workspace + '/' + filename, 'content': source})
        started = time.monotonic()
        first = c.call('state', {'action':'prepare','packages':packages,'workspace':workspace})
        concurrent = c.call('state', {'action':'prepare','packages':packages,'workspace':workspace+'/concurrent'})
        for prepared in [first, concurrent]:
            if not prepared['ready']: c.done(prepared['job'],1200)
        timings.append({'profile':profile,'cached':first['cached'],'concurrent':True,'seconds':time.monotonic()-started})
        output = execute(c, command[0], command[1:], cwd=workspace)
        assert profile + ' fixture: 42' in output, output
        # A second workspace inherits the cached environment without a shell hook.
        samples = []
        for n in range(5):
            samples.append(prepare(c, packages, workspace + '/second-' + str(n)))
        assert all(sample['cached'] for sample in samples), samples
        timings.extend(samples)
    # A compiler must accept dependency headers outside the project's own directory.
    output = execute(c, 'bash', ['-c', 'set -e; mkdir -p ../headers; printf "#define ANSWER 42\\n" > ../headers/fixture.h; printf "#include <fixture.h>\\nint main(){return ANSWER != 42;}\\n" > main.c; cc -O0 -I"$PWD/../headers" main.c -o c-fixture; ./c-fixture; cmake --version; perl -e \'print "native dependencies: 42\\n"\''], cwd=root+'/rust')
    assert 'native dependencies: 42' in output
    c.output.joinpath('workspace-timings.json').write_text(json.dumps(timings, indent=2))


# A public CLI library catches dependency assumptions that an inline hello-world misses.
CLICK_REVISION = '934813e4d421071a1b3db3973c02fe2721359a6e'


def second_repository(c):
    root = '/home/agent/qa/click-' + c.run_id
    execute(c, 'git', ['clone', '--filter=blob:none', 'https://github.com/pallets/click.git', root], timeout=180)
    execute(c, 'git', ['checkout', '--detach', CLICK_REVISION], cwd=root)
    prepare(c, ['python312','uv'], root)
    execute(c, 'uv', ['venv', '.venv'], cwd=root)
    execute(c, 'uv', ['pip', 'install', '--python', '.venv/bin/python', '--no-deps', '-e', '.', 'pytest==8.3.5'], cwd=root, timeout=180)
    execute(c, 'uv', ['pip', 'install', '--python', '.venv/bin/python', 'iniconfig==2.0.0', 'packaging==24.2', 'pluggy==1.5.0'], cwd=root, timeout=180)
    output = execute(c, '.venv/bin/python', ['-m', 'pytest', '-q', 'tests/test_arguments.py', 'tests/test_options.py'], cwd=root, timeout=180)
    assert 'passed' in output and 'failed' not in output, output
    c.output.joinpath('second-repository-tests.txt').write_text(output)


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
    flake = Path(__file__).with_name('fixtures').joinpath('toad.nix').read_text().replace('@NIXPKGS@',c.call('state', {'action':'info'})['nixpkgs'])
    c.call('files', {'action':'put','path':root+'/qa-nix/flake.nix','content':flake})
    timing = prepare(c, None, root, flake='qa-nix')
    assert 'tauri-cli' in execute(c, 'cargo', ['tauri', '--version'], cwd=root)
    timing['repeat'] = prepare(c, None, root)
    c.output.joinpath('native-environment.json').write_text(json.dumps(timing, indent=2))
    target = '/home/agent/qa/native-target'
    script = 'set -euo pipefail\nexport CARGO_TARGET_DIR=/home/agent/qa/native-target\nexport CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0\ncd ui\nbun install --frozen-lockfile\nbun run build\ncd ..\ncargo build --locked -p toad-desktop --features tauri/custom-protocol\n'
    c.call('files', {'action': 'put', 'path': root+'/qa-build.sh', 'content': script})
    job = c.call('files', {'action': 'run', 'path': root+'/qa-build.sh', 'cwd': root, 'sha256': hashlib.sha256(script.encode()).hexdigest()})
    # Desktop work remains responsive during the native build.
    browser(c)
    c.screenshot('05-browsing-during-native-build.png')
    c.done(job, 2400)
    c.output.joinpath('native-build-output.txt').write_text(c.call('shell', {'action': 'read', 'job_id': job['id'], 'max_output': 1048576})['output'])
    app = c.call('shell', {'action': 'start', 'command': target+'/debug/toad-desktop', 'cwd': root, 'env': {'TOAD_DATA_DIR': root+'/qa-data', 'CARGO_BUILD_JOBS':'2', 'CARGO_PROFILE_DEV_DEBUG':'0', 'CARGO_INCREMENTAL':'0'}, 'label': 'Toad native screen acceptance'})
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
    def painted_screen(name, previous):
        started = time.monotonic()
        while time.monotonic()-started < 5:
            c.screenshot(name)
            with Image.open(c.output/previous) as before, Image.open(c.output/name) as after:
                # Exclude the tray; cursor and hover changes cannot satisfy this check.
                bounds = (0, 80, min(before.width, after.width), min(before.height, after.height))
                change = ImageChops.difference(before.convert('RGB').crop(bounds), after.convert('RGB').crop(bounds)).convert('L')
                histogram = change.histogram()
                fraction = sum(histogram[16:])/(change.width*change.height)
            if fraction > .01:
                with c.output.joinpath('native-paint-timings.jsonl').open('a') as output:
                    output.write(json.dumps({'screen':name, 'seconds':time.monotonic()-started, 'changed_fraction':fraction})+'\n')
                return
            time.sleep(.1)
        raise AssertionError('Accessibility changed but the native screen did not paint: ' + name)

    c.screenshot('06-toad-native-screen.png', settle_ms=1000)
    terminal = next(w for w in c.call('windows', {'action':'list'}) if 'toadterminal' in w['class'].lower())
    c.call('shell', {'action':'show','job_id':app['id']})
    tiled = c.call('windows', {'action':'tile','primary_id':window_id,'observer_id':terminal['id']})
    assert tiled['ok'], tiled
    tree_with('[button] New teammate')
    c.screenshot('06b-toad-and-observer.png', settle_ms=1000)
    c.call('windows', {'action':'maximize','window_id':window_id})
    tree = tree_with('[button] New teammate')
    click_label(tree, 'Settings')
    tree = tree_with('[button] Computer')
    painted_screen('07-toad-settings.png', '06-toad-native-screen.png')
    click_label(tree, 'Computer')
    tree = tree_with('Desktop image')
    painted_screen('08-toad-computer-settings.png', '07-toad-settings.png')
    c.output.joinpath('native-accessibility.txt').write_text(tree)
    c.output.joinpath('native-window.json').write_text(json.dumps(native_windows, indent=2))
    c.call('shell', {'action': 'cancel', 'job_id': app['id']})

def legacy_window(c):
    revision = c.call('state', {'action':'info'})['nixpkgs']
    nixpkgs = 'github:NixOS/nixpkgs/' + revision
    job = c.call('shell',{'action':'launch','command':'nix','args':['shell',nixpkgs+'#xterm','--command','xterm','-title','Legacy title acceptance','-e','sleep','120'],'label':'Legacy Xterm window acceptance'})
    try:
        deadline = time.monotonic()+180
        while time.monotonic()<deadline:
            assert c.call('shell',{'action':'status','job_id':job['id']})['state']=='running',c.call('shell',{'action':'read','job_id':job['id']})
            legacy = next((w for w in c.call('windows',{'action':'list'}) if w['title']=='Legacy title acceptance'),None)
            if legacy: break
            time.sleep(.2)
        assert legacy,'Xterm did not map a window'
        execute(c,'nix',['shell',nixpkgs+'#xorg.xprop','--command','xprop','-id',legacy['id'],'-f','_NET_WM_NAME','8u','-set','_NET_WM_NAME',''],timeout=180)
        observed = next(w for w in c.call('windows',{'action':'list'}) if w['id']==legacy['id'])
        assert observed['title']=='Legacy title acceptance',observed
        c.call('windows',{'action':'focus','window_id':legacy['id']})
        c.screenshot('13-legacy-xterm-title.png')
        c.call('windows',{'action':'close','window_id':legacy['id']})
    finally:
        if c.call('shell',{'action':'status','job_id':job['id']})['state']=='running':
            c.call('shell',{'action':'cancel','job_id':job['id']})


def native_controls(c):
    root = '/home/agent/qa/gtk-' + c.run_id
    source = Path(__file__).with_name('fixtures').joinpath('native-controls.c').read_text()
    c.call('files', {'action':'put','path':root+'/main.c','content':source})
    prepare(c,['gcc','pkg-config','gtk3'],root)
    execute(c,'bash',['-c','cc -Wno-deprecated-declarations main.c -o fixture $(pkg-config --cflags --libs gtk+-3.0)'],cwd=root)
    app = c.call('shell', {'action':'launch','command':root+'/fixture','cwd':root,'label':'Native controls acceptance'})
    try:
        deadline = time.monotonic()+10
        while time.monotonic()<deadline:
            windows = c.call('windows',{'action':'list'})
            native = [w for w in windows if w.get('pid')==app['pid']]
            if len(native)==2: break
            time.sleep(.1)
        assert len(native)==2,windows
        capture = str(c.call('capture',{}))
        assert 'hidden-native-password' not in capture
        # Identical titles/PIDs/geometry are intentionally ambiguous until arranged.
        assert capture.count('no unambiguous application/window match') >= 2, capture
        primary = next(w for w in native if w['minimum_size'][0]>=640)
        secondary = next(w for w in native if w['id']!=primary['id'])
        c.call('windows',{'action':'tile','primary_id':primary['id'],'observer_id':secondary['id']})
        capture = str(c.call('capture',{}))
        def window_tree(window_id):
            return capture.split('['+window_id+' ',1)[1].split('\n[0x',1)[0]
        primary_tree = window_tree(primary['id'])
        secondary_tree = window_tree(secondary['id'])
        assert 'Primary acceptance window' in primary_tree and 'Secondary acceptance window' not in primary_tree, primary_tree
        assert 'Secondary acceptance window' in secondary_tree and 'Primary acceptance window' not in secondary_tree, secondary_tree
        assert 'value="Editable native value"' in primary_tree, primary_tree
        assert re.search(r'Native checked state.*states=\[[^\]]*checked',primary_tree),primary_tree
        c.output.joinpath('native-controls-tree.txt').write_text(capture)
        c.call('windows',{'action':'focus','window_id':primary['id']})
        match = re.search(r'Native clipboard field (-?\d+),(-?\d+) (\d+)x(\d+)',primary_tree)
        assert match,primary_tree
        x,y,w,h = map(int,match.groups())
        c.call('input',{'action':'click','x':x+w//2,'y':y+h//2})
        c.call('input',{'action':'key','combo':'ctrl+a'})
        c.call('input',{'action':'paste','text':'Native café 🐸'})
        capture = str(c.call('capture',{}))
        assert 'value="Native café 🐸"' in capture,capture
        c.screenshot('11-native-controls.png')
        tray = bar_layout(c)['tray']
        assert len(tray)==1, tray
        slot = tray[0]
        c.call('input',{'action':'right_click','x':slot[0]+slot[2]//2,'y':slot[1]+slot[3]//2})
        c.screenshot('12-tray-menu.png')
        c.call('input',{'action':'key','combo':'Down'})
        c.call('input',{'action':'key','combo':'Return'})
        capture = str(c.call('capture',{}))
        assert 'Tray action activated' in capture,capture
        c.screenshot('12b-tray-action.png')
        c.call('windows',{'action':'close','window_id':primary['id']},error=True)
        capture = str(c.call('capture',{}))
        assert 'Confirm closing the test app' in capture,capture
        button = re.search(r'\[button\] Close app (-?\d+),(-?\d+) (\d+)x(\d+)',capture)
        assert button,capture
        x,y,w,h=map(int,button.groups())
        c.call('input',{'action':'click','x':x+w//2,'y':y+h//2})
        c.done(app)
        c.screenshot('12c-tray-disappeared.png')
        with Image.open(c.output/'12b-tray-action.png') as before, Image.open(c.output/'12c-tray-disappeared.png') as after:
            box = (slot[0]-4,slot[1]-4,slot[0]+slot[2]+4,slot[1]+slot[3]+4)
            assert ImageChops.difference(before.convert('RGB').crop(box),after.convert('RGB').crop(box)).getbbox(), 'tray icon remained after application exit'
    finally:
        if c.call('shell',{'action':'status','job_id':app['id']})['state']=='running':
            c.call('shell',{'action':'cancel','job_id':app['id']})


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
    assert report['info']['executables']['chromium'] == '/usr/bin/chromium'
    guide = c.call('state', {'action': 'guide'})
    assert hashlib.sha256(guide['skill'].encode()).hexdigest() == guide['sha256']
    cases = [('browser forms', browser), ('three-step browser wizard', wizard), ('public Selenium form', public_form), ('no save-password bubble', password_prompt), ('managed jobs and observer', jobs), ('desktop job menu', desktop_job_menu), ('tray counts match live jobs', tray_counts), ('Nix failure diagnostics', nix_failure), ('verified script execution', artifacts), ('artifact failure recovery', download_failures)]
    if options.suite == 'full':
        cases += [('package workspaces', workspaces), ('second repository tests', second_repository), ('Ketch installation and scraping', ketch), ('job durability and responsiveness', durability), ('native Toad build and screens', native), ('native controls and window identity', native_controls), ('legacy Xterm title', legacy_window)]
    elif options.suite == 'workspaces':
        cases = [('package workspaces', workspaces), ('second repository tests', second_repository), ('Ketch installation and scraping', ketch), ('job durability and responsiveness', durability)]
    elif options.suite == 'native':
        cases = [('native Toad build and screens', native), ('native controls and window identity', native_controls), ('legacy Xterm title', legacy_window)]
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
