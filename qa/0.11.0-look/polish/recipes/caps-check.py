import json, pathlib, sys, time
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import mcp
def wins(): return json.loads(mcp.text("windows", {"action": "list"}))
def find(part): return next((w for w in wins() if part in w["class"].lower()), None)
def until(fn, t=8):
    end = time.time() + t
    while time.time() < end and not fn(): time.sleep(0.1)
    time.sleep(0.8)
mcp.text("browser", {"action": "navigate", "url": "data:text/html,<body style='margin:0;background:%23fff'>white page"})
mcp.text("shell", {"action": "show"})
until(lambda: find("hotlineterminal") and find("chromium")["bounds"][2] == 1248)
mcp.text("shell", {"action": "start", "label": "an installed app", "command": "alacritty", "args": []})
until(lambda: find("alacritty.alacritty"))
mcp.shot("caps-1-app-over-browser.png")
mcp.text("windows", {"action": "focus", "window_id": find("chromium")["id"]})
until(lambda: find("chromium")["focused"])
mcp.shot("caps-2-browser-over-app.png")
mcp.text("windows", {"action": "focus", "window_id": find("alacritty.alacritty")["id"]})
mcp.text("windows", {"action": "close", "window_id": find("hotlineterminal")["id"]})
until(lambda: find("hotlineterminal") is None and find("alacritty.alacritty")["bounds"][2] == 1888)
mcp.shot("caps-3-expanded.png")
# A click right at a cap must still reach the app beneath the cut corner.
print(json.dumps([(w["class"].split(".")[0], w["bounds"], w["focused"]) for w in wins()]))
