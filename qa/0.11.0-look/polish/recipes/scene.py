"""Build the hero scene on a fresh computer and shoot it.

Usage: python3 scene.py PREFIX
"""
import json, pathlib, sys, time, urllib.request
import os
os.environ.setdefault("HC_HOLDER", "Tad")  # the demo's persona
import mcp

PREFIX = sys.argv[1] if len(sys.argv) > 1 else "x"
HERE = pathlib.Path(__file__).parent
ORIGIN = mcp.URL.rsplit("/mcp", 1)[0]


def start(label, command, args=None, **extra):
    return json.loads(mcp.text("shell", {"action": "start", "label": label, "command": command,
                                          "args": args or [], **extra}))


def exec_(label, command, args=None, **extra):
    return mcp.text("shell", {"action": "exec", "label": label, "command": command,
                              "args": args or [], **extra})


def refs():
    mcp.text("browser", {"action": "text"})
    return json.loads(mcp.text("browser", {"action": "eval", "js":
        "JSON.stringify(Object.fromEntries([...document.querySelectorAll('input,button')]"
        ".map(e=>[e.id,e.dataset.hotlineRef])))"}).strip().strip('"').replace('\\"', '"'))


def put_secret():
    req = urllib.request.Request(ORIGIN + "/secrets", method="PUT", headers={
        "Authorization": "Bearer " + mcp.TOKEN, "Content-Type": "application/json"},
        data=json.dumps({"FIELDNOTE": {"kind": "login", "sites": ["http://localhost:8000"],
                                       "username": "tad@yourteam.dev",
                                       "password": "correct-horse-battery-42"}}).encode())
    with mcp._opener.open(req) as r:
        assert r.status in (200, 204), r.status


# A site to sign in to, served inside the computer, and a pnpm on PATH.
mcp.text("files", {"action": "put", "path": "/home/agent/src/fieldnote/public/login.html",
                   "content": (HERE / "site/login.html").read_text()})
mcp.text("files", {"action": "put", "path": "/home/agent/.local/bin/pnpm",
                   "content": (HERE / "site/pnpm").read_text()})
exec_("make pnpm runnable", "chmod", ["+x", "/home/agent/.local/bin/pnpm"])
mcp.text("shell", {"action": "show"})
start("dev server", "python3", ["-m", "http.server", "8000"], cwd="/home/agent/src/fieldnote/public")
for _ in range(80):
    try:
        mcp._opener.open("http://localhost:8000/login.html", timeout=1).close(); break
    except Exception:
        time.sleep(0.1)
PNPM = "pnpm"
ENV = {"PATH": "/home/agent/.local/bin:/usr/local/bin:/usr/bin:/bin"}
APP = "/home/agent/src/fieldnote"
exec_("Unit tests", PNPM, ["test"], cwd=APP, env=ENV)
exec_("Lint", PNPM, ["lint"], cwd=APP, env=ENV)
start("End-to-end: sign in", PNPM, ["exec", "playwright", "test", "e2e/login"], cwd=APP, env=ENV)

mcp.text("browser", {"action": "navigate", "url": "http://localhost:8000/login.html"})
time.sleep(0.8)
# The Activity window, from the Hotline menu (key V).
mcp.text("input", {"action": "click", "x": 22, "y": 20})
time.sleep(0.3)
mcp.text("input", {"action": "key", "combo": "v"})
time.sleep(1.0)
mcp.text("windows", {"action": "tile"})
time.sleep(1.2)
put_secret()
r = refs()
mcp.text("browser", {"action": "click_ref", "ref": r["email"]})
mcp.text("browser", {"action": "fill", "ref": r["email"], "text": "tad@yourteam.dev"})
mcp.text("browser", {"action": "fill", "ref": r["password"], "secret": "FIELDNOTE.password"})
time.sleep(0.6)
mcp.shot(f"{PREFIX}-hero.png")
mcp.shot(f"{PREFIX}-bar.png", [0, 0, 1920, 40])
print(mcp.text("windows", {"action": "list"})[:2000])
