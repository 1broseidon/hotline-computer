#!/usr/bin/env python3
"""Browser checks with an explicit fake screen and socket, not container QA."""
import io
import json
import mimetypes
import struct
import threading
import time
import traceback
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

from PIL import Image, ImageDraw
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / 'qa/0.11.0-look'
RESULTS = {'kind': 'Local viewer fixtures, not Docker scenes', 'checks': {}, 'browser_errors': []}
FILES = {'existing.txt': b'kept'}
POSTS = []
REQUESTS = []
DENY_FILES = False
OFFLINE = False


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def reply(self, code, body, mime='application/json'):
        self.send_response(code)
        self.send_header('Content-Type', mime)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        parsed = urlsplit(self.path)
        query = parse_qs(parsed.query)
        REQUESTS.append(parsed.path)
        if parsed.path == '/':
            self.reply(200, (ROOT / 'src/viewer.html').read_bytes(), 'text/html')
        elif parsed.path.startswith('/assets/fonts/'):
            path = ROOT / 'assets/fonts' / Path(parsed.path).name
            self.reply(200, path.read_bytes(), mimetypes.guess_type(str(path))[0] or 'font/woff2')
        elif parsed.path == '/health':
            self.reply(503 if OFFLINE else 200, b'ok', 'text/plain')
        elif parsed.path in ('/ws', '/files', '/files/download'):
            if query.get('token') != ['qa-only'] or DENY_FILES:
                self.reply(401, b'{"error":"unauthorized"}')
            elif parsed.path == '/ws':
                self.reply(503, b'no fixture websocket', 'text/plain')
            elif parsed.path == '/files/download':
                name = Path(query['path'][0]).name
                self.reply(200, FILES[name], 'application/octet-stream')
            else:
                self.reply(200, json.dumps({'home': '/home/agent', 'path': '/home/agent', 'entries': [
                    {'name': name, 'size': len(data), 'is_dir': False, 'modified': 1792540800}
                    for name, data in FILES.items()]}).encode())
        else:
            self.reply(404, b'{}')
    def do_POST(self):
        query = parse_qs(urlsplit(self.path).query)
        body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
        name = Path(query['path'][0]).name
        only = query.get('create_only') == ['true']
        POSTS.append({'name': name, 'create_only': only})
        time.sleep(.1)
        if only and name in FILES:
            self.reply(409, b'{"error":"destination already exists"}')
        else:
            FILES[name] = body
            self.reply(200, b'{}')


image = Image.new('RGB', (1920, 1080), '#0b0b0d')
draw = ImageDraw.Draw(image)
draw.rectangle((16, 56, 1264, 1064), fill='#f5f5f2')
draw.rectangle((1280, 56, 1904, 1064), fill='#111114')
draw.text((64, 110), 'VIEWER TEST FIXTURE. No container screen is available.', fill='#202020')
encoded = io.BytesIO()
image.save(encoded, format='PNG')
FRAME = struct.pack('<6H', 0, 0, 1920, 1080, 1920, 1080) + encoded.getvalue()


class Socket:
    def __init__(self, page, frame=True):
        self.page = page
        self.frame = frame
        self.pongs = True
        self.messages = []
        self.connections = []
        self.ws = None
        page.route_web_socket('**/ws?token=qa-only', self.open)
    def open(self, ws):
        self.ws = ws
        records = []
        self.connections.append(records)
        state = {'t': 'state', 'holder': 'none', 'driving': False, 'running': 2, 'completed': 1, 'failed': 1,
                 'agent': {'name': 'Tad', 'task': 'pnpm dev', 'last_action_ms': 1}}
        initial = True
        def receive(raw):
            nonlocal initial
            message = json.loads(raw)
            records.append(message)
            self.messages.append(message)
            if initial:
                initial = False
                ws.send(json.dumps(state))
                if self.frame: ws.send(FRAME)
            if message['t'] == 'ping' and self.pongs:
                ws.send(json.dumps({'t': 'pong', 'id': message['id']}))
            if message['t'] == 'control':
                state['driving'] = message['take']
                state['holder'] = 'you' if message['take'] else 'none'
                ws.send(json.dumps(state))
        ws.on_message(receive)
    def send(self, message): self.ws.send(json.dumps(message))


def check(name, work):
    try:
        work()
        RESULTS['checks'][name] = 'passed'
    except Exception:
        RESULTS['checks'][name] = traceback.format_exc()
        print(name + ': FAILED', flush=True)


def main():
    global DENY_FILES, OFFLINE
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    origin = 'http://127.0.0.1:' + str(server.server_port)
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        context = browser.new_context(viewport={'width': 1440, 'height': 900})
        page = context.new_page()
        page.set_default_timeout(4000)
        page.on('pageerror', lambda error: RESULTS['browser_errors'].append(str(error)))
        page.add_init_script("Object.defineProperty(navigator,'platform',{get:()=> 'MacIntel'})")
        page.clock.install()
        fixture = Socket(page)
        page.goto(origin + '/#qa-only')
        page.wait_for_function("document.querySelector('#screen').width === 1920")
        page.evaluate('document.fonts.ready')

        def watching():
            assert page.locator('#status').inner_text() == 'Watching Tad'
            assert page.locator('#job-count').evaluate('(e)=>getComputedStyle(e).color') == 'rgb(255, 143, 128)'
            assert 'Geist' in page.locator('body').evaluate('(e)=>getComputedStyle(e).fontFamily')
            assert page.evaluate("document.fonts.check('13px Geist')")
            frame = page.locator('#screen').bounding_box()
            assert frame['y'] + frame['height'] <= 828, frame
            assert not any(m['t'] == 'control' for m in fixture.messages)
            page.screenshot(path=str(OUT / 'viewer-fixture-watching.png'))
        check('watching, fonts, fit and failure color', watching)

        def pointer():
            fixture.send({'t':'pointer','x':500,'y':300,'button':1,'down':True,'intent':'clicking'})
            page.wait_for_function("document.querySelector('#intent').classList.contains('shown')")
            assert page.locator('#intent-label').inner_text() == 'Tad · clicking'
            page.clock.fast_forward(2100)
            assert not page.locator('#intent').evaluate("e=>e.classList.contains('shown')")
        check('intent and two-second fade', pointer)

        def driving():
            page.get_by_role('button', name='Take control', exact=True).click()
            page.wait_for_function("document.querySelector('#status').textContent === \"You're driving\"")
            assert page.locator('#keys').is_visible()
            page.screenshot(path=str(OUT / 'viewer-fixture-driving.png'))
            page.keyboard.press('Meta+a')
            page.wait_for_timeout(30)
            keys = [m for m in fixture.messages if m['t'] == 'key']
            assert {'t':'key','key':'Control','down':True} in keys, keys
            assert not any(m['key'] == 'Meta' for m in keys), keys
            page.keyboard.press('Control+Alt+Shift')
            page.wait_for_function("document.querySelector('#status').textContent === 'Watching Tad'")
            assert fixture.messages[-1] == {'t':'control','take':False}, fixture.messages[-5:]
        check('control, Cmd mapping and emergency give-back', driving)

        def scale():
            page.get_by_role('button', name='Settings', exact=True).click()
            page.get_by_role('button', name='1:1 pixels').click()
            assert page.locator('#screen').bounding_box()['width'] == 1920
            page.get_by_role('button', name='1:1 pixels').click()
            page.get_by_role('button', name='Settings', exact=True).click()
        check('native pixel scale', scale)

        def touch():
            page.get_by_role('button', name='Take control', exact=True).click()
            session = context.new_cdp_session(page)
            session.send('Emulation.setTouchEmulationEnabled', {'enabled':True})
            session.send('Input.dispatchTouchEvent', {'type':'touchStart','touchPoints':[{'x':400,'y':300}]})
            session.send('Input.dispatchTouchEvent', {'type':'touchCancel','touchPoints':[]})
            page.wait_for_timeout(30)
            buttons = [m for m in fixture.messages if m['t'] == 'button']
            assert buttons[-2:] == [{'t':'button','b':1,'down':True},{'t':'button','b':1,'down':False}], buttons
            session.send('Emulation.setTouchEmulationEnabled', {'enabled':False})
            session.detach()
        check('touch cancellation releases the button', touch)

        def errors():
            fixture.send({'t':'error','error':'Fixture error','driving':True})
            page.get_by_role('alert').filter(has_text='Fixture error').wait_for()
            assert int(page.locator('#toast').evaluate('(e)=>getComputedStyle(e).zIndex')) > int(page.locator('#files').evaluate('(e)=>getComputedStyle(e).zIndex'))
            page.clock.fast_forward(6100)
            assert page.locator('#toast').evaluate("e=>e.classList.contains('quiet')")
            fixture.send({'t':'error','error':'Next input clears this','driving':True})
            page.wait_for_function("document.querySelector('#toast').textContent === 'Next input clears this'")
            page.keyboard.press('a')
            assert page.locator('#toast').evaluate("e=>e.classList.contains('quiet')")
            page.get_by_role('button', name='Give it back', exact=True).click()
        check('errors expire and clear on input', errors)

        def files():
            page.get_by_role('button', name='Files', exact=True).click()
            page.get_by_role('button', name='Download existing.txt', exact=True).wait_for()
            assert page.locator('#browse').get_attribute('aria-expanded') == 'true'
            assert '2026' in page.get_by_role('button', name='Download existing.txt').inner_text()
            page.once('dialog', lambda dialog: dialog.dismiss())
            page.locator('#files-pick').set_input_files({'name':'existing.txt','mimeType':'text/plain','buffer':b'changed'})
            page.wait_for_function("!document.querySelector('.upload')")
            assert POSTS == [{'name':'existing.txt','create_only':True}], POSTS
            assert FILES['existing.txt'] == b'kept'
            page.once('dialog', lambda dialog: dialog.accept())
            page.locator('#files-pick').set_input_files({'name':'existing.txt','mimeType':'text/plain','buffer':b'changed'})
            page.wait_for_function("!document.querySelector('.upload')")
            assert POSTS[-2:] == [{'name':'existing.txt','create_only':True},{'name':'existing.txt','create_only':False}], POSTS
            assert FILES['existing.txt'] == b'changed'
            page.get_by_role('button', name='Close Files').click()
            page.evaluate("""() => { const transfer = new DataTransfer(); transfer.items.add(new File(['drop'], 'dropped.txt', {type:'text/plain'})); window.dispatchEvent(new DragEvent('drop',{dataTransfer:transfer,cancelable:true})); }""")
            page.get_by_role('button', name='Download dropped.txt', exact=True).wait_for()
            assert FILES['dropped.txt'] == b'drop'
            with page.expect_download() as download:
                page.get_by_role('button', name='Download dropped.txt', exact=True).click()
            assert download.value.suggested_filename == 'dropped.txt'
            page.get_by_role('button', name='Close Files').click()
        check('files, safe overwrite, global drop and download', files)

        def refused():
            global DENY_FILES
            DENY_FILES = True
            page.get_by_role('button', name='Files', exact=True).click()
            page.wait_for_function("document.querySelector('#toast').textContent.includes('Open it again from Hotline')")
            DENY_FILES = False
            page.get_by_role('button', name='Close Files').click()
        check('file authorization errors are readable', refused)

        def stalled():
            fixture.pongs = False
            page.clock.fast_forward(11000)
            page.wait_for_function("document.querySelector('#bar').classList.contains('stalled')")
            assert 'stalled' in page.locator('#latency').inner_text()
            fixture.pongs = True
            page.clock.fast_forward(5000)
            page.wait_for_function("!document.querySelector('#bar').classList.contains('stalled')")
        check('ping latency, stalled state and recovery', stalled)

        def reconnect():
            page.get_by_role('button', name='Take control', exact=True).click()
            fixture.ws.close(code=1012, reason='fixture restart')
            page.wait_for_function("document.querySelector('#status').textContent === 'Reconnecting…'")
            page.clock.fast_forward(1000)
            page.wait_for_function("document.querySelector('#status').textContent === 'Watching Tad'")
            assert len(fixture.connections) == 2
            assert not any(m['t'] == 'control' for m in fixture.connections[-1]), fixture.connections[-1]
        check('reconnect stays view-only', reconnect)

        phone_context = browser.new_context(viewport={'width':390,'height':844}, is_mobile=True, has_touch=True)
        phone = phone_context.new_page(); phone.set_default_timeout(4000)
        phone.on('pageerror', lambda error: RESULTS['browser_errors'].append(str(error)))
        Socket(phone)
        def narrow():
            phone.goto(origin + '/#qa-only')
            phone.wait_for_function("document.querySelector('#screen').width === 1920")
            phone.evaluate('document.fonts.ready')
            bounds = phone.locator('#bar').bounding_box()
            assert bounds['x'] >= 11 and bounds['x'] + bounds['width'] <= 379, bounds
            assert phone.evaluate('document.documentElement.scrollWidth') == 390
            phone.screenshot(path=str(OUT / 'viewer-fixture-phone.png'))
            phone.get_by_role('button', name='Take control', exact=True).click()
            bounds = phone.locator('#bar').bounding_box()
            assert bounds['x'] >= 11 and bounds['x'] + bounds['width'] <= 379, bounds
            assert phone.locator('#screen').evaluate('(e)=>getComputedStyle(e).touchAction') == 'none'
        check('phone watching and driving fit without horizontal overflow', narrow)

        bad = context.new_page(); bad.set_default_timeout(4000); bad.clock.install()
        def badtoken():
            count = REQUESTS.count('/ws')
            bad.goto(origin + '/')
            bad.get_by_text('This link is missing its key.', exact=False).last.wait_for()
            bad.clock.fast_forward(31000)
            assert REQUESTS.count('/ws') == count + 1, REQUESTS
            bad.screenshot(path=str(OUT / 'viewer-fixture-badtoken.png'))
        check('missing token stops retrying', badtoken)

        empty = context.new_page(); empty.set_default_timeout(4000); Socket(empty, frame=False)
        def waiting():
            empty.goto(origin + '/#qa-only')
            empty.get_by_text('Waiting for the screen…', exact=True).wait_for()
            assert not empty.locator('#screen').is_visible()
        check('connected without frames has a placeholder', waiting)

        offline = context.new_page(); offline.set_default_timeout(5000)
        def unavailable():
            global OFFLINE
            OFFLINE = True
            offline.goto(origin + '/#qa-only')
            offline.get_by_text("The computer isn't running.", exact=True).last.wait_for()
            OFFLINE = False
        check('unavailable computer has a clear message', unavailable)
        context.close(); phone_context.close(); browser.close()
    server.shutdown(); server.server_close()
    assert not RESULTS['browser_errors'], RESULTS['browser_errors']


if __name__ == '__main__':
    try:
        main()
    finally:
        (OUT / 'viewer-checks.json').write_text(json.dumps(RESULTS, indent=2) + '\n')
        print(json.dumps(RESULTS, indent=2))
    raise SystemExit(0 if all(result == 'passed' for result in RESULTS['checks'].values()) else 1)
