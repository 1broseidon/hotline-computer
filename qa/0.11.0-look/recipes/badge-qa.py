#!/usr/bin/env python3
"""Verify a real sealed-fill receipt and its empty X11 input region."""
import importlib.util
import json
from pathlib import Path
import time
import urllib.request

spec = importlib.util.spec_from_file_location('look', Path(__file__).with_name('look-qa.py'))
look = importlib.util.module_from_spec(spec)
spec.loader.exec_module(look)
c = look.Client()
results = {}
probe = Path(__file__).with_name('badge-inspect.py').read_text()


def badges():
    result = c.call('shell', {'action': 'exec', 'command': 'python3', 'args': ['-c', probe], 'label': 'Inspect receipt input region'})
    assert result['exit_code'] == 0, result
    return json.loads(result['stdout'])


def refs():
    c.call('browser', {'action': 'text'})
    return c.call('browser', {'action': 'eval', 'js': "Object.fromEntries([...document.querySelectorAll('input')].map(e=>[e.id,e.dataset.hotlineRef]))"})


def fill(reference, secret='FIELDNOTE.password'):
    return c.call('browser', {'action': 'fill', 'ref': reference, 'secret': secret})


server = None
try:
    c.call('files', {'action': 'put', 'path': '/home/agent/.hotline/look-qa/login.html', 'content': Path(__file__).with_name('login.html').read_text()})
    server = c.start('Badge login fixture', 'python3', ['-m', 'http.server', '8000'], cwd='/home/agent/.hotline/look-qa')
    c.call('shell', {'action': 'exec', 'command': 'python3', 'args': ['-c', "import time,urllib.request\nfor _ in range(50):\n try:\n  urllib.request.urlopen('http://localhost:8000/login.html').close();break\n except OSError:time.sleep(.1)\nelse:raise SystemExit(1)"], 'label': 'Login fixture readiness'})
    c.call('browser', {'action': 'navigate', 'url': 'http://localhost:8000/login.html'})
    fields = refs()
    request = urllib.request.Request(look.ORIGIN + '/secrets', method='PUT', headers=c.headers,
        data=json.dumps({'FIELDNOTE': {'kind': 'login', 'sites': ['http://localhost:8000'],
                         'username': 'qa@example.test', 'password': 'SyntheticBadgePassword-42'}}).encode())
    with urllib.request.urlopen(request) as response:
        assert response.status == 204
    c.call('browser', {'action': 'eval', 'js': "document.querySelector('form').addEventListener('submit',()=>document.body.dataset.receiptClicked='yes')"})
    started = time.monotonic()
    fill(fields['password'])
    filled = time.monotonic()
    c.capture('s5-badge.png')
    results['fill seconds'] = round(filled - started, 3)
    results['capture after fill seconds'] = round(time.monotonic() - filled, 3)
    assert results['capture after fill seconds'] < 2
    visible = badges()
    assert len(visible) == 1, visible
    badge = visible[0]
    assert badge['input_rectangles'] == 0 and badge['override_redirect'], badge
    results['X11 receipt'] = badge
    assert not any(w['title'] == 'Hotline sealed fill' for w in c.call('windows', {'action': 'list'}))
    focused = c.call('browser', {'action': 'eval', 'js': 'document.activeElement.id'})
    assert focused == 'password', focused
    assert c.call('browser', {'action': 'eval', 'js': "document.body.textContent.includes('Typed from sealed secret')"}) is False
    x, y, w, h = badge['bounds']
    c.call('input', {'action': 'click', 'x': x+40, 'y': y+h-5, 'capture_after': 'none'})
    assert c.call('browser', {'action': 'eval', 'js': 'document.body.dataset.receiptClicked'}) == 'yes'
    results['click passes through / focus / no DOM injection'] = 'passed'
    deadline = filled + 6
    while badges():
        assert time.monotonic() < deadline, 'receipt outlived its four-second timer'
        time.sleep(.1)
    elapsed = time.monotonic() - filled
    assert 3.6 <= elapsed < 5.5, elapsed
    results['disappeared after fill seconds'] = round(elapsed, 3)
    c.call('browser', {'action': 'navigate', 'url': 'http://127.0.0.1:8000/login.html'})
    fields = refs()
    refused = c.rpc('tools/call', {'name': 'browser', 'arguments': {'action': 'fill', 'ref': fields['password'], 'secret': 'FIELDNOTE.password'}})
    assert refused['result']['isError'], refused
    assert badges() == []
    results['rejected origin has no receipt'] = 'passed'
    entries = c.call('files', {'action': 'get', 'path': '/home/agent/.hotline/activity.jsonl'})
    assert 'SyntheticBadgePassword-42' not in entries
    assert 'from sealed secret FIELDNOTE.password' in entries
    results['activity receipt contains no secret value'] = 'passed'
finally:
    if server:
        c.call('shell', {'action': 'cancel', 'job_id': server['id']})
    (look.OUT / 'phase8-scenes.json').write_text(json.dumps(results, indent=2) + '\n')
    print(json.dumps(results, indent=2))
