import json, pathlib, sys, time
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import mcp
MAIN, FULL = [16, 56, 1248, 1008], [16, 56, 1888, 1008]
SHELL_SLOT, OBS_ABOVE, OBS_FULL = [1280, 744, 624, 320], [1280, 56, 624, 672], [1280, 56, 624, 1008]
def wins(): return json.loads(mcp.text("windows", {"action": "list"}))
def find(part): return next((w for w in wins() if part in w["class"].lower()), None)
ok = True
def check(label, expect, t=6):
    global ok
    end = time.time() + t
    while True:
        got = {k: (find(k) or {}).get("bounds") for k in expect}
        if got == expect or time.time() > end: break
        time.sleep(0.1)
    time.sleep(0.4); got = {k: (find(k) or {}).get("bounds") for k in expect}
    good = got == expect; ok &= good
    print(("PASS " if good else "FAIL ") + label + ("" if good else f": {got}"))
def bar_layout():
    out = mcp.text("shell", {"action": "exec", "label": "read the bar", "command": "xprop", "args": ["-root", "-notype", "_HOTLINE_BAR_LAYOUT"]})
    stdout = json.loads(out)["stdout"]
    raw = stdout[stdout.index("= ") + 2:].strip()
    return json.loads(raw[1:-1].replace('\\"', '"'))
def click_place():
    # The shell's title settles after it opens, which moves its pill; wait
    # until two reads of the bar agree.
    rect, last = None, bar_layout()["shell_place"]["rect"]
    while rect != last:
        time.sleep(0.5); rect, last = last, bar_layout()["shell_place"]["rect"]
    mcp.text("input", {"action": "click", "x": rect[0] + rect[2] // 2, "y": rect[1] + rect[3] // 2})
def menu(key):
    mcp.text("input", {"action": "click", "x": 22, "y": 20}); time.sleep(0.3)
    mcp.text("input", {"action": "key", "combo": key})

mcp.text("browser", {"action": "navigate", "url": "data:text/html,<title>App</title>app"})
mcp.text("shell", {"action": "show"})
menu("t")
check("shell opens in its column slot", {"hotlineshell": SHELL_SLOT, "hotlineterminal": OBS_ABOVE, "chromium": MAIN})
mcp.shot("shell-0-bar-docked.png", [0, 0, 1920, 40])
click_place()
check("bar toggle moves shell to the app pane", {"hotlineshell": MAIN, "hotlineterminal": OBS_FULL, "chromium": MAIN})
print("  shell focused:", find("hotlineshell")["focused"], "maximized:", find("hotlineshell")["maximized"])
mcp.shot("shell-1-in-pane.png")
mcp.shot("shell-1-bar-pane.png", [0, 0, 1920, 40])
mcp.text("windows", {"action": "tile"})
check("agent auto-tile leaves the expanded shell alone", {"hotlineshell": MAIN, "hotlineterminal": OBS_FULL, "chromium": MAIN})
click_place()
check("bar toggle docks shell back", {"hotlineshell": SHELL_SLOT, "hotlineterminal": OBS_ABOVE, "chromium": MAIN})
mcp.text("windows", {"action": "maximize", "window_id": find("hotlineshell")["id"]})
check("windows maximize also moves the shell to the pane", {"hotlineshell": MAIN, "hotlineterminal": OBS_FULL})
mcp.text("windows", {"action": "close", "window_id": find("hotlineterminal")["id"]})
check("observer closes: shell and browser take the full area", {"hotlineshell": FULL, "chromium": FULL, "hotlineterminal": None})
mcp.text("shell", {"action": "show"})
check("observer returns: both back to the pane", {"hotlineshell": MAIN, "chromium": MAIN, "hotlineterminal": OBS_FULL})
mcp.text("windows", {"action": "maximize", "window_id": find("hotlineshell")["id"], "unmaximize": True})
check("restore docks it again", {"hotlineshell": SHELL_SLOT, "hotlineterminal": OBS_ABOVE, "chromium": MAIN})
print("ALL PASS" if ok else "SOME FAILED")
