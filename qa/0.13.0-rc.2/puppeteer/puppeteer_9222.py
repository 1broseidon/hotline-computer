#!/usr/bin/env python3
"""Puppeteer on the general browser's DevTools port (9222) in a split computer:
a job installs puppeteer-core with npm (Node 24 from nodejs.org, as the J7
probe brings it), connects to http://127.0.0.1:9222, opens a page served by
another job, reads it, types into it, takes a screenshot, and checks that the
signed-in browser's tabs are not among the targets it sees. On the split
computer, a passkey request from the page Puppeteer opened is refused at once.
usage: puppeteer_9222.py IMAGE [split|legacy]"""
import json
import os
import secrets
import sys
import tempfile
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "../../../tests/protected"))
import canaries as K  # noqa: E402
import probes as P  # noqa: E402
import run as R  # noqa: E402

image = sys.argv[1]
split = (sys.argv[2] if len(sys.argv) > 2 else "split") == "split"
rt = "container"
name = f"hl-secrets-rc2-pptr-{secrets.token_hex(3)}"
token = secrets.token_hex(24)
port = R.free_port()
print("# containers before (none touched):\n" + R.run(R.LIST[rt]).stdout)
made = R.create(rt, name, image, split, port, {"HOTLINE_COMPUTER_TOKEN": token})
ok = False
try:
    assert made.returncode == 0 and R.up(port), made.stderr
    c = R.Computer(port, token)
    raw, kind = K.body("readable") if split else K.body("v1")
    print("PUT /secrets", c.http("PUT", "/secrets", raw=raw, headers={"Content-Type": kind})[0])
    print("capabilities", c.http("GET", "/capabilities?nonce=pp")[2].decode()[:160])
    # A login's site open in the signed-in browser, to check it is not reachable on 9222.
    c.sh("mkdir -p ~/site ~/pp")
    c.tool("files", action="put", path="site/index.html",
           content="<html><head><title>Puppeteer fixture</title></head><body><h1>Hello from 8001</h1><input id=q></body></html>")
    c.tool("shell", action="start", command="cd ~/site && exec python3 -m http.server 8001 --bind 127.0.0.1", wait_ms=500)
    c.tool("shell", action="start", command="cd ~/site && exec python3 -m http.server 8000 --bind 127.0.0.1", wait_ms=500)
    time.sleep(1.5)
    print("signed-in tab:", c.tool("browser", action="navigate", url="http://localhost:8000/")[0][:120])
    print("9222 before any general-browser call:", c.sh("curl -s -m4 -o /dev/null -w '%{http_code}' http://127.0.0.1:9222/json/version")[0])
    print("general tab:", c.tool("browser", action="navigate", url="http://localhost:8001/")[0][:120])
    print("9222 after:", c.sh("curl -s -m4 -o /dev/null -w '%{http_code}' http://127.0.0.1:9222/json/version")[0])
    ctx = type("C", (), {"c": c})()
    node = P.node(ctx)
    npm = node[:-len("node")] + "npm" if node != "node" else "npm"
    out = c.sh(f"cd ~/pp && PATH=$HOME/probe/node/bin:$PATH {npm} install --no-audit --no-fund puppeteer-core@24 2>&1 | tail -3", wait_ms=300000)[0]
    print("npm:", out.strip()[-200:])
    script = r"""
const puppeteer = require('puppeteer-core');
(async () => {
  const browser = await puppeteer.connect({browserURL: 'http://127.0.0.1:9222', defaultViewport: null});
  const version = await browser.version();
  const urls = (await browser.targets()).map(t => t.url());
  const page = await browser.newPage();
  await page.goto('http://localhost:8001/', {waitUntil: 'load'});
  const title = await page.title();
  const h1 = await page.$eval('h1', e => e.textContent);
  await page.type('#q', 'typed by puppeteer');
  const typed = await page.$eval('#q', e => e.value);
  const shot = await page.screenshot({encoding: 'base64'});
  const started = Date.now();
  const passkey = await page.evaluate(() => navigator.credentials.get({publicKey: {challenge: new Uint8Array(32), rpId: 'localhost', timeout: 10000}})
    .then(() => 'signed', (e) => e.name + ': ' + e.message));
  const passkeyMs = Date.now() - started;
  await page.close();
  await browser.disconnect();
  console.log(JSON.stringify({version, title, h1, typed, shotBytes: shot.length, signedInTabVisible: urls.some(u => u.includes('localhost:8000')), targets: urls.length, passkey, passkeyMs}));
})().catch(e => { console.log(JSON.stringify({error: String(e)})); process.exit(1); });
"""
    c.tool("files", action="put", path="pp/check.js", content=script)
    res = c.sh(f"cd ~/pp && {node} check.js", wait_ms=60000)[0]
    print("puppeteer:", res.strip()[-600:])
    r = json.loads(res.strip().splitlines()[-1])
    ok = (r.get("title") == "Puppeteer fixture" and r.get("h1") == "Hello from 8001" and r.get("typed") == "typed by puppeteer"
          and r.get("shotBytes", 0) > 1000 and (not split or (r.get("signedInTabVisible") is False and r.get("passkeyMs", 99999) < 5000
                               and "signed-in browser" in r.get("passkey", ""))))
    print("RESULT", "PASS" if ok else "FAIL", "split" if split else "legacy")
finally:
    R.remove(rt, name)
    print("# removed", name)
sys.exit(0 if ok else 1)
