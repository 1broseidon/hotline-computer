"""A granted login is offered in the page by the Hotline extension and filled
on a click, as an agent would do it: by the accessibility tree and the
mouse. The values come from the computer's service for this origin only."""
import json, pathlib, re, sys, time, urllib.request
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import mcp

HERE = pathlib.Path(__file__).resolve().parent
ORIGIN = mcp.URL.rsplit("/mcp", 1)[0]
LOGIN = {"kind": "login", "sites": ["http://localhost:8124"], "username": "teammate@fieldnote.dev",
         "password": "correct-horse-battery-42", "totp": "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"}
req = urllib.request.Request(ORIGIN + "/secrets", method="PUT", headers={
    "Authorization": "Bearer " + mcp.TOKEN, "Content-Type": "application/json"},
    data=json.dumps({"FIELDNOTE": LOGIN}).encode())
assert mcp._opener.open(req).status == 204
mcp.text("files", {"action": "put", "path": "/home/agent/src/signin/index.html", "content": (HERE / "site/signin.html").read_text()})
mcp.text("shell", {"action": "start", "label": "Serve the sign-in page", "command": "python3",
                   "args": ["-m", "http.server", "8124", "--bind", "127.0.0.1"], "cwd": "/home/agent/src/signin"})
time.sleep(1)
mcp.text("browser", {"action": "navigate", "url": "http://localhost:8124/"})
time.sleep(1.5)
ref = next(l.split("]")[0][1:] for l in mcp.text("browser", {"action": "text"}).splitlines() if "[email]" in l)
mcp.text("browser", {"action": "click_ref", "ref": ref})
time.sleep(1)
browser = next(w for w in json.loads(mcp.text("windows", {"action": "list"})) if "chromium" in w["class"].lower() and ".hotline/browser" in w["class"])

def press(name):
    tree = "\n".join(x for x in mcp.call("capture", {"window": browser["id"], "mode": "tree"}) if isinstance(x, str))
    line = next((l for l in tree.splitlines() if "[push button]" in l or "[button]" in l if name in l), None)
    assert line, f"no button {name!r} in the tree:\n" + "\n".join(l for l in tree.splitlines() if "button" in l)[:2000]
    x, y, w, h = map(int, re.search(r" (-?\d+),(-?\d+) (\d+)x(\d+) states=", line).groups())
    mcp.text("input", {"action": "click", "x": x + w // 2, "y": y + h // 2})
    time.sleep(1)

press("Hotline: fill a login")
press("FIELDNOTE")
time.sleep(1)
got = json.loads(json.loads(mcp.text("browser", {"action": "eval", "js":
    "JSON.stringify({email: email.value, password: password.value.length, otp: otp.value, seen})"})))
print(json.dumps(got))
assert got["email"] == LOGIN["username"], got
assert got["password"] == len(LOGIN["password"]), got
assert len(got["otp"]) == 6 and got["otp"].isdigit(), got
assert all(n > 0 for n in got["seen"].values()), "the page saw input events"
mcp.shot("login-filled.png")
print("ok: filled from the extension")
