import json, pathlib, sys, time
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import mcp
FULL, MAIN = [16, 56, 1888, 1008], [16, 56, 1248, 1008]
def wins(): return json.loads(mcp.text("windows", {"action": "list"}))
def find(part): return next((w for w in wins() if part in w["class"].lower()), None)
def wait(part, gone=False, bounds=None, t=6):
    end = time.time() + t
    while time.time() < end:
        w = find(part)
        if (w is None) == gone and (gone or bounds is None or w["bounds"] == bounds):
            time.sleep(0.4); w = find(part)  # must survive Chromium's own follow-up
            if (w is None) == gone and (gone or bounds is None or w["bounds"] == bounds):
                return w
        time.sleep(0.1)
    return find(part)
ok = True
def check(label, part, bounds, gone=False):
    global ok
    w = wait(part, gone=gone, bounds=bounds)
    got = None if w is None else (w["bounds"], w["maximized"])
    good = (w is None) if gone else (w is not None and w["bounds"] == bounds)
    ok &= good
    print(("PASS " if good else "FAIL ") + label + ": " + str(got))
def close(part): mcp.text("windows", {"action": "close", "window_id": find(part)["id"]})
def menu(key):
    mcp.text("input", {"action": "click", "x": 22, "y": 20}); time.sleep(0.3)
    mcp.text("input", {"action": "key", "combo": key})

mcp.text("browser", {"action": "navigate", "url": "data:text/html,<title>A</title>A"})
check("browser alone is full", "chromium", FULL)
mcp.text("shell", {"action": "show"})
check("observer opens -> browser left 2/3", "chromium", MAIN)
close("chromium"); check("browser closed", "chromium", None, gone=True)
menu("b"); check("menu reopens browser -> left 2/3", "chromium", MAIN)
close("chromium"); check("browser closed again", "chromium", None, gone=True)
mcp.text("browser", {"action": "navigate", "url": "data:text/html,<title>B</title>B"})
check("agent reopens browser -> left 2/3", "chromium", MAIN)
close("hotlineterminal"); check("observer closed", "hotlineterminal", None, gone=True)
check("column empty -> browser full again", "chromium", FULL)
mcp.text("shell", {"action": "show"})
check("observer back -> browser left 2/3", "chromium", MAIN)
mcp.text("windows", {"action": "maximize", "window_id": find("chromium")["id"]})
check("maximize with column open -> fills left pane only", "chromium", MAIN)
mcp.text("windows", {"action": "maximize", "window_id": find("chromium")["id"], "unmaximize": True})
mcp.text("shell", {"action": "start", "label": "an installed app", "command": "alacritty", "args": []})
check("a plain app opens in the left pane", "alacritty.alacritty", MAIN)
mcp.text("windows", {"action": "tile"})
check("auto tile keeps the browser left", "chromium", MAIN)
check("auto tile keeps the other app left", "alacritty.alacritty", MAIN)
check("auto tile keeps the observer right", "hotlineterminal", [1280, 56, 624, 1008])
mcp.shot("tile-final.png")
print("ALL PASS" if ok else "SOME FAILED")
