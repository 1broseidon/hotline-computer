#!/usr/bin/env python3
"""Run against a fresh `make run` computer. --static needs no container.

Uses the release's original PNG file capture; image blocks are limited to 1568px.
Install host dependencies in a venv: pip install pillow playwright
For S4: playwright install chromium
"""
import argparse
import base64
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / 'qa/0.11.0-look'
ORIGIN = 'http://127.0.0.1:' + os.environ.get('PORT', '8787')
RESULTS = {}


def contrast(foreground, background):
    def luminance(color):
        values = [int(color[i:i+2], 16) / 255 for i in (0, 2, 4)]
        return sum(weight * (v / 12.92 if v <= .04045 else ((v + .055) / 1.055) ** 2.4)
                   for weight, v in zip((.2126, .7152, .0722), values))
    a, b = sorted((luminance(foreground), luminance(background)))
    return (b + .05) / (a + .05)


def static_checks():
    for name, fg, bg in [('muted/bar', '8e8e95', '111114'),
                         ('muted/surface', '8e8e95', '17171b'),
                         ('ink-2/chip', 'b4b4ba', '19191e')]:
        measured = contrast(fg, bg)
        RESULTS[name] = round(measured, 3)
        assert measured >= 4.5, (name, measured)
    for face in ('Geist-Regular', 'Geist-Medium', 'Geist-SemiBold', 'GeistMono-Regular', 'GeistMono-Medium'):
        for extension in ('ttf', 'woff2'):
            assert (ROOT / 'assets/fonts' / f'{face}.{extension}').stat().st_size > 1000


class Client:
    def __init__(self):
        self.token = (ROOT / '.token').read_text().strip()
        self.headers = {'Authorization': 'Bearer ' + self.token, 'Content-Type': 'application/json',
                        'Accept': 'application/json, text/event-stream', 'X-Computer-Holder': 'look-qa'}
        self.sequence = 0
        self.session = None
        self.rpc('initialize', {'protocolVersion': '2025-03-26', 'capabilities': {},
                               'clientInfo': {'name': 'look-qa', 'version': '1'}})
        self.rpc('notifications/initialized', notification=True)

    def rpc(self, method, params=None, notification=False):
        self.sequence += 1
        payload = {'jsonrpc': '2.0', 'method': method}
        if not notification:
            payload['id'] = self.sequence
        if params is not None:
            payload['params'] = params
        headers = dict(self.headers)
        if self.session:
            headers['Mcp-Session-Id'] = self.session
        request = urllib.request.Request(ORIGIN + '/mcp', data=json.dumps(payload).encode(), headers=headers)
        with urllib.request.urlopen(request, timeout=100) as response:
            self.session = response.headers.get('Mcp-Session-Id', self.session)
            if response.status == 202:
                return None
            if 'text/event-stream' in response.headers.get('Content-Type', ''):
                for line in response:
                    if line.startswith(b'data:') and line[5:].strip():
                        reply = json.loads(line[5:])
                        if reply.get('id') == self.sequence:
                            return reply
                raise RuntimeError('MCP stream ended before its reply')
            return json.loads(response.read())

    def call(self, name, args):
        started = time.monotonic()
        reply = self.rpc('tools/call', {'name': name, 'arguments': args})
        result = reply.get('result', {})
        assert 'error' not in reply and not result.get('isError'), reply
        texts = [item['text'] for item in result['content'] if item['type'] == 'text']
        value = '\n'.join(texts)
        # Traces record safe fixture actions and timing, never the bearer.
        with (OUT / 'scene-trace.jsonl').open('a') as stream:
            stream.write(json.dumps({'tool': name, 'action': args.get('action', args.get('mode')),
                                     'seconds': round(time.monotonic() - started, 3)}) + '\n')
        try:
            return json.loads(value)
        except json.JSONDecodeError:
            return value

    def capture(self, filename, region=None):
        path = '/home/agent/.hotline/look-qa/' + filename
        args = {'mode': 'png', 'path': path, 'settle_ms': 1000}
        if region:
            args['region'] = region
        self.call('capture', args)
        encoded = self.call('files', {'action': 'get', 'path': path})
        assert encoded.startswith('encoding=base64\n'), 'expected an original PNG'
        (OUT / filename).write_bytes(base64.b64decode(encoded.split('\n', 2)[2]))
        from PIL import Image
        return Image.open(OUT / filename).convert('RGB')

    def start(self, label, command, args, **extra):
        return self.call('shell', {'action': 'start', 'label': label, 'command': command, 'args': args, **extra})


def pixel(image, point, color, tolerance):
    actual = image.getpixel(point)
    expected = tuple(int(color[i:i+2], 16) for i in (0, 2, 4))
    RESULTS[f'pixel {point} {color}'] = list(actual)
    assert all(abs(a-b) <= tolerance for a, b in zip(actual, expected)), (point, actual, expected)


def scene4(client):
    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        RESULTS['S4'] = 'not run: Playwright unavailable'
        return
    with sync_playwright() as playwright:
        try:
            browser = playwright.chromium.launch(headless=True)
        except Exception as error:
            RESULTS['S4'] = 'not run: headless Chrome unavailable (' + str(error).splitlines()[0] + ')'
            return
        context = browser.new_context(viewport={'width': 1440, 'height': 900}, device_scale_factor=1)
        page = context.new_page()
        page.goto(ORIGIN + '/#' + client.token)
        page.wait_for_function("document.querySelector('#status').textContent.startsWith('Watching ')")
        page.wait_for_function("document.querySelector('#screen').width > 1")
        page.evaluate('document.fonts.ready')
        RESULTS['viewer font'] = page.locator('body').evaluate('(e) => getComputedStyle(e).fontFamily')
        assert 'Geist' in RESULTS['viewer font']
        page.screenshot(path=str(OUT / 's4-watching.png'))
        page.get_by_role('button', name='Take control', exact=True).click()
        page.wait_for_function('document.querySelector("#bar").classList.contains("driving")')
        page.screenshot(path=str(OUT / 's4-driving.png'))
        page.get_by_role('button', name='Give it back', exact=True).click()
        page.close()
        bad = context.new_page()
        bad.goto(ORIGIN + '/')
        bad.locator('#placeholder-text').filter(has_text='This link is missing its key.').wait_for()
        bad.screenshot(path=str(OUT / 's4-badtoken.png'))
        bad.close()
        phone = browser.new_context(viewport={'width': 390, 'height': 844}, is_mobile=True, has_touch=True)
        page = phone.new_page()
        page.goto(ORIGIN + '/#' + client.token)
        page.wait_for_function("document.querySelector('#screen').width > 1")
        page.evaluate('document.fonts.ready')
        page.screenshot(path=str(OUT / 's4-phone.png'))
        phone.close()
        context.close()
        browser.close()
        RESULTS['S4'] = 'passed'


def scenes():
    c = Client()
    windows = c.call('windows', {'action': 'list'})
    assert not windows, 'S1 needs a fresh make run desktop; existing windows are left untouched'
    c.call('files', {'action': 'put', 'path': '/home/agent/.hotline/look-qa/login.html',
                     'content': (Path(__file__).parent / 'login.html').read_text()})
    empty = c.capture('s1-empty.png')
    assert empty.size == (1920, 1080), empty.size
    pixel(empty, (960, 20), '111114', 3)
    pixel(empty, (960, 39), '222227', 3)
    server = c.start('login fixture', 'python3', ['-m', 'http.server', '8000'], cwd='/home/agent/.hotline/look-qa')
    try:
        # Poll fixture readiness inside the computer, without trusting a sleep.
        c.call('shell', {'action': 'exec', 'command': 'python3', 'args': ['-c',
            "import time,urllib.request\nfor i in range(100):\n try:\n  urllib.request.urlopen('http://localhost:8000/login.html').close();break\n except OSError: time.sleep(.1)\nelse: raise SystemExit('fixture did not start')"], 'label': 'fixture readiness'})
        # Starting a job opens the observer; close it for the browser-only scene.
        for window in c.call('windows', {'action': 'list'}):
            if 'hotlineterminal' in window['class'].lower():
                c.call('windows', {'action': 'close', 'window_id': window['id']})
        c.call('browser', {'action': 'navigate', 'url': 'http://localhost:8000/login.html'})
        c.capture('s2-browser.png')
        done = c.start('pnpm test', 'sh', ['-c', 'printf "All tests passed\\n"'])
        c.call('shell', {'action': 'wait', 'job_id': done['id'], 'wait_ms': 5000})
        dev = c.start('pnpm dev', 'sleep', ['600'])
        failed = c.start('lint', 'sh', ['-c', 'printf "lint failed\\n"; exit 3'])
        c.call('shell', {'action': 'wait', 'job_id': failed['id'], 'wait_ms': 5000})
        c.call('shell', {'action': 'show'})
        c.call('windows', {'action': 'tile'})
        busy = c.capture('s3-busy.png')
        c.capture('s3-bar.png', [0, 0, 1920, 40])
        pixel(busy, (8, 540), '0b0b0d', 3)
        windows = c.call('windows', {'action': 'list'})
        left = min(windows, key=lambda w: w['bounds'][0])['bounds']
        pixel(busy, (left[0] - 1, left[1] + left[3] // 2), '2a2a30', 6)
        scene4(c)
        for generic, expected in [('sans-serif', 'Geist'), ('monospace', 'Geist Mono')]:
            result = c.call('shell', {'action': 'exec', 'command': 'fc-match', 'args': ['-f', '%{family}', generic], 'label': 'font check'})
            RESULTS['fc-match ' + generic] = result.get('stdout', '')
            assert expected in RESULTS['fc-match ' + generic], result
        c.call('shell', {'action': 'cancel', 'job_id': dev['id']})
    finally:
        c.call('shell', {'action': 'cancel', 'job_id': server['id']})
    with (OUT / 'contract.log').open('w') as log:
        result = subprocess.run(['make', 'contract'], cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
    RESULTS['make contract'] = result.returncode
    assert result.returncode == 0, 'see contract.log'


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--static', action='store_true', help='only contrast and vendored asset checks')
    args = parser.parse_args()
    OUT.mkdir(exist_ok=True)
    try:
        static_checks()
        if args.static:
            RESULTS['S1/S2/S3/S4'] = 'not run: no Docker'
            RESULTS['fc-match / make contract'] = 'not run: no Docker'
        else:
            scenes()
    finally:
        destination = OUT / ('static-checks.json' if args.static else 'scene-checks.json')
        destination.write_text(json.dumps(RESULTS, indent=2) + '\n')
        print(json.dumps(RESULTS, indent=2))
