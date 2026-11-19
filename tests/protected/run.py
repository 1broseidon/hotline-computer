#!/usr/bin/env python3
"""Protected-secrets probes: does a granted secret still leave, or get read back?

usage: run.py --runtime docker|podman|container --image REF --expect leak|blocked
              [--split] [--delivery v1|readable|protected]
              [--only ID,ID] [--except ID,ID] [--keep] [--prefix hl-secrets-w5]

--split creates the container with 0.50's flags (--user 0 --cap-add SETUID
--cap-add SETGID); without it, as 0.49 does (legacy mode). --delivery v1
(the default) sends the body a desk before 0.50 sends; readable and protected
send a v2 body with CANARY_VAR and GH_TOKEN readable:true, or bound to an
HTTPS fixture on the host bridge (fixture.py; the image must be a probe build
with --features probe-roots, which trusts the fixture's CA upstream).
A probe that should work and does not (a protected key never reaching its
service) is BROKEN and counts as differing from --expect.

Starts one throwaway container (hl-secrets-w5-*) from IMAGE, delivers made-up
secrets only (canaries.py), runs every probe and prints one line per probe:
id, what it tries, LEAKED or BLOCKED (SKIPPED when it cannot run by itself).
Exits 1 if any result differs from --expect. Ids in --except are shown but not
counted. "Leaves the computer" probes talk only to a listener this script runs
on the host's container bridge address; nothing is sent anywhere public.
The container is removed with the runtime's own rm. Other containers are
listed first and never touched.
"""
import argparse
import base64
import http.server
import os
import pathlib
import re
import secrets
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import canaries as K  # noqa: E402
import probes as P  # noqa: E402
from fixture import FixtureCA  # noqa: E402
from mcpc import Computer  # noqa: E402

HERE = pathlib.Path(__file__).resolve().parent
LIST = {"docker": ["docker", "ps", "-a"], "podman": ["podman", "ps", "-a"], "container": ["container", "list", "--all"]}


class Listener:
    """The host side of every "leaves the computer" probe: records requests."""

    def __init__(self, addr, tls=None):
        self.rows = []
        self.pages = {}  # path -> HTML a probe serves to the browser (A3's references)
        # path prefix -> fn(method, path, headers, body bytes) -> (status, headers, bytes):
        # the two-condition run's API and git service.
        self.routes = {}
        me = self

        class H(http.server.BaseHTTPRequestHandler):
            def _h(self):
                n = int(self.headers.get("Content-Length") or 0)
                raw = self.rfile.read(n) if n else b""
                body = raw.decode(errors="replace")
                auth = self.headers.get("Authorization", "")
                text = f"{self.path}\n{auth}\n{body}"
                m = re.match(r"Basic (\S+)", auth)
                if m:
                    try:
                        text += "\n" + base64.b64decode(m.group(1)).decode(errors="replace")
                    except Exception:
                        pass
                me.rows.append((self.path, text))
                for prefix, route in list(me.routes.items()):
                    if self.path.startswith(prefix):
                        status, headers, reply = route(self.command, self.path, self.headers, raw)
                        self.send_response(status)
                        for k, v in headers.items():
                            self.send_header(k, v)
                        self.send_header("Content-Length", str(len(reply)))
                        self.end_headers()
                        self.wfile.write(reply)
                        return
                if self.path in me.pages:
                    page = me.pages[self.path].encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "text/html; charset=utf-8")
                    self.send_header("Cache-Control", "no-store")
                    self.send_header("Content-Length", str(len(page)))
                    self.end_headers()
                    self.wfile.write(page)
                    return
                # Every reply has a length: over TLS a reply that ends with the
                # connection is a truncation to a strict client (Python's
                # server closes without close_notify), and git never sees the 401.
                if self.path.startswith("/j3") and not auth:
                    self.send_response(401)
                    self.send_header("WWW-Authenticate", 'Basic realm="x"')
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                    return
                reply = (auth if self.path.startswith("/echo") else "ok").encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(reply)))
                self.end_headers()
                self.wfile.write(reply)

            do_GET = do_POST = do_PUT = _h

            def log_message(self, *a):
                pass

        self.server = http.server.ThreadingHTTPServer((addr, 0), H)
        if tls:
            import ssl
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            context.load_cert_chain(*tls)
            self.server.socket = context.wrap_socket(self.server.socket, server_side=True)
        self.port = self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def heard(self, prefix, only=None):
        return any(P.has_canary(t, only) for path, t in list(self.rows) if path.startswith(prefix))


class Ctx:
    def __init__(self, c, listener, host, ws, token, runtime, name, delivery="v1", bound=None, free=None):
        self.c, self.L, self.host, self.port, self.ws, self.token = c, listener, host, listener.port, ws, token
        self.runtime, self.name = runtime, name
        # v2 protected runs: the HTTPS fixture the variables are bound to, and
        # one on another port they are not (an unbound host).
        self.delivery, self.bound, self.free = delivery, bound, free
        self.split = False
        self.shared = False
        self.paged = False
        self.served = set()
        self.refs = {}
        # Set by main: what N3 needs to boot its own pair of containers.
        self.image = self.n3_from = self.prefix = self.job_source = None
        self.launch = self.remove = self.volume = self.unvolume = None

    def heard(self, prefix, only=None):
        return self.L.heard(prefix, only)

    def job(self, *a):
        return P.job(self, *a)

    def page(self):
        """Serve a small form on localhost:8000 inside the container and open it in the browser."""
        if not self.paged:
            html = ('<html><body><form action="http://%s:%d/form" method=get>'
                    '<input name=u type=text><input name=p type=password><button>go</button></form></body></html>'
                    % (self.host, self.port))
            self.c.sh("mkdir -p ~/site")
            self.c.tool("files", action="put", path="site/index.html", content=html)
            self.serve(8000)
            self.paged = True
        self.c.tool("browser", action="navigate", url="http://localhost:8000/")
        text, _ = self.c.tool("browser", action="text")
        self.refs = {kind: ref for ref, kind in re.findall(r"\[(e\d+)\] \[(text|password|button)\]", text)}

    def ref(self, kind):
        return self.refs[kind]

    def serve(self, port):
        """~/site on localhost:PORT inside the container (8000 is LOCALAPP's site; 8001 is no login's)."""
        if port not in self.served:
            self.c.sh("mkdir -p ~/site")
            self.c.tool("shell", action="start", command=f"cd ~/site && exec python3 -m http.server {port} --bind 127.0.0.1", wait_ms=500)
            self.served.add(port)
            time.sleep(1.5)


def run(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def create(rt, name, image, split, port, env=None, mounts=()):
    """`rt run` a throwaway computer as the desk creates one: as 0.49 does,
    or with 0.50's split flags. `env` names variables passed by name only, so
    no value is on a command line."""
    args = [rt, "run", "-d", "--name", name, "--cap-drop=ALL"]
    if split:
        # The desk's 0.50 create flags: boot as root, keep only what the
        # split needs (section 2 of the plan).
        cap = "CAP_" if rt == "container" else ""
        args += ["--user", "0", "--cap-add", f"{cap}SETUID", "--cap-add", f"{cap}SETGID"]
    if rt != "container":
        args += ["--security-opt", "no-new-privileges", "--pids-limit", "1024"]
    for key in env or {}:
        args += ["-e", key]
    args += ["--memory", "4g", "--shm-size", "1g", "-p", f"127.0.0.1:{port}:8787"]
    for mount in mounts:
        args += ["-v", mount]
    return run(args + [image], env=dict(os.environ, **(env or {})))


def up(port, seconds=90):
    for _ in range(seconds):
        try:
            if urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2).status == 200:
                return True
        except Exception:
            time.sleep(1)
    return False


def host_address(c):
    """The host side as the container sees it: its default gateway is the host bridge."""
    route = c.sh("cat /proc/net/route")[0]
    gw = next((l.split() for l in route.splitlines() if len(l.split()) > 2 and l.split()[1] == "00000000"), None)
    return os.environ.get("HOST_ADDR") or (socket.inet_ntoa(bytes.fromhex(gw[2])[::-1]) if gw else None)


def remove(rt, name):
    if rt == "container":
        run([rt, "stop", name])
    return run([rt, "rm"] + (["-f"] if rt != "container" else []) + [name])


def home_volume(rt, name, image):
    """A named volume for /home/agent, owned by the agent. Docker and Podman
    seed a new volume from the image; Apple container hands it over as root's,
    so a one-off root container gives it to the agent first (Docker and Podman also have the seed emptied: a seeded home is not the fresh desktop N3's restore step assumes), and takes away
    the filesystem's empty root-only lost+found, which no snapshot could read."""
    run([rt, "volume", "create", name])
    run([rt, "run", "--rm", "--name", name + "-chown", "--user", "0", "--entrypoint", "/bin/sh",
         "-v", f"{name}:/home/agent", image, "-c",
         """chown 1000:1000 /home/agent && chmod 0700 /home/agent && { rmdir /home/agent/lost+found 2>/dev/null; true; }"""
         + ("" if rt == "container" else " && find /home/agent -mindepth 1 -delete")])


def launcher(rt, token):
    """ctx.launch for probes that boot computers of their own (N3)."""
    def launch(name, image, split, home=None):
        port = free_port()
        made = create(rt, name, image, split, port, {"HOTLINE_COMPUTER_TOKEN": token},
                      [f"{home}:/home/agent"] if home else [])
        if made.returncode != 0:
            raise RuntimeError(f"could not start {name}: " + (made.stderr.strip() or made.stdout.strip())[:200])
        if not up(port):
            raise RuntimeError(f"{name} did not come up")
        return Computer(port, token), port
    return launch


def probe_all(ctx, expect, only=(), skip=()):
    """Run every probe (or `only` those) against ctx; print one line each.
    Returns ([(id, word, ok)], how many differ from `expect`)."""
    want_leak = expect == "leak"
    results, bad = [], 0
    print(f"{'ID':<10} {'RESULT':<8} {'':<2} WHAT")
    for pid, what, fn in P.PROBES:
        if only and pid not in only:
            continue
        try:
            leaked, detail = fn(ctx)
        except Exception as e:  # a probe that cannot run is reported, never silently BLOCKED
            leaked, detail = None, f"probe error: {type(e).__name__}: {e}"
        broken = leaked == "broken"
        word = "BROKEN" if broken else "SKIPPED" if leaked is None else "LEAKED" if leaked else "BLOCKED"
        ok = (not broken and (leaked is None or leaked == want_leak)) or pid in skip
        if not ok:
            bad += 1
        mark = "  " if leaked is None else ("ok" if ok else "!!")
        note = " (excepted)" if pid in skip and leaked is not None and leaked != want_leak else ""
        print(f"{pid:<10} {word:<8} {mark} {what}: {detail}{note}", flush=True)
        results.append((pid, word, ok))
    counts = {w: sum(1 for r in results if r[1] == w) for w in ("LEAKED", "BLOCKED", "SKIPPED", "BROKEN")}
    print(f"# {counts['LEAKED']} leaked, {counts['BLOCKED']} blocked, {counts['SKIPPED']} skipped, "
          f"{counts['BROKEN']} broken; {bad} differ from EXPECT={expect}")
    return results, bad


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runtime", required=True, choices=list(LIST))
    ap.add_argument("--image", required=True)
    ap.add_argument("--expect", required=True, choices=["leak", "blocked"])
    ap.add_argument("--only", default="")
    ap.add_argument("--except", dest="skip", default="")
    ap.add_argument("--keep", action="store_true", help="leave the container for inspection")
    ap.add_argument("--split", action="store_true", help="start it as the 0.50 desk does: --user 0 with SETUID and SETGID")
    ap.add_argument("--delivery", default="v1", choices=["v1", "readable", "protected"])
    ap.add_argument("--prefix", default="hl-secrets-w5")
    ap.add_argument("--n3-from", default="", help="the image whose single-user home N3 upgrades (default: --image, booted as 0.49 does)")
    a = ap.parse_args()
    rt = a.runtime

    print(f"# containers already running ({rt}); none of them is touched:")
    print(run(LIST[rt]).stdout.rstrip() or "(none)")
    name = a.prefix + "-" + secrets.token_hex(3)
    token = secrets.token_hex(24)
    port = free_port()
    ws = pathlib.Path(tempfile.mkdtemp(prefix=a.prefix + "-ws-"))
    os.chmod(ws, 0o777)
    env = {"HOTLINE_COMPUTER_TOKEN": token}
    ca = None
    if a.delivery == "protected":
        ca = FixtureCA()
        env["HOTLINE_KEYPROXY_PROBE_ROOT"] = ca.pem
    made = create(rt, name, a.image, a.split, port, env, [f"{ws}:/home/agent/workspace"])
    if made.returncode != 0:
        print("could not start the container:", made.stderr.strip() or made.stdout.strip())
        return 2
    listener = None
    fixtures = []
    results = []
    try:
        c = Computer(port, token)
        if not up(port):
            print("the computer did not come up")
            return 2
        host = host_address(c)
        if not host:
            print("cannot tell the host's bridge address; set HOST_ADDR")
            return 2
        listener = Listener(os.environ.get("LISTEN_ADDR") or host)
        bound = free = None
        origin = None
        if a.delivery == "protected":
            leaf = ca.leaf(host)
            bound = Listener(os.environ.get("LISTEN_ADDR") or host, tls=leaf)
            free = Listener(os.environ.get("LISTEN_ADDR") or host, tls=leaf)
            fixtures = [bound, free]
            origin = f"https://{host}:{bound.port}"
        raw, content_type = K.body(a.delivery, origin)
        status, _, answer = c.http("PUT", "/secrets", raw=raw, headers={"Content-Type": content_type})
        if status != 204:
            print("secrets delivery refused:", status, answer.decode(errors="replace")[:300])
            return 2
        _, _, caps = c.http("GET", "/capabilities?nonce=probe")
        print(f"# /capabilities: {caps.decode(errors='replace')[:200]}")
        print(f"# image {a.image} as {name} ({'split' if a.split else 'legacy'}, delivery {a.delivery}); listener on {host}:{listener.port}"
              + (f"; HTTPS fixture {origin} (bound), :{free.port} (unbound)" if bound else "")
              + f"; expecting {a.expect.upper()}")
        c.sh("mkdir -p ~/probe")
        c.tool("files", action="put", path="probe/job.py", content=(HERE / "jobs/job.py").read_text())
        ctx = Ctx(c, listener, host, ws, token, rt, name, a.delivery, bound, free)
        ctx.split = a.split
        ctx.image, ctx.n3_from, ctx.prefix = a.image, a.n3_from or a.image, a.prefix
        ctx.job_source = (HERE / "jobs/job.py").read_text()
        ctx.launch = launcher(rt, token)
        ctx.remove = lambda n: remove(rt, n)
        ctx.volume = lambda v: home_volume(rt, v, ctx.n3_from)
        ctx.unvolume = lambda v: run([rt, "volume", "rm", v])
        only = {x for x in a.only.split(",") if x}
        skip = {x for x in a.skip.split(",") if x}
        results, bad = probe_all(ctx, a.expect, only, skip)
        return 1 if bad else 0
    finally:
        if listener:
            listener.server.shutdown()
        for fixture in fixtures:
            fixture.server.shutdown()
        if ca:
            ca.remove()
        if not a.keep:
            r = remove(rt, name)
            print(f"# removed {name}" if r.returncode == 0 else f"# could not remove {name}: {r.stderr.strip()}")
        else:
            print(f"# kept {name} (port {port}, token {token})")


if __name__ == "__main__":
    sys.exit(main())
