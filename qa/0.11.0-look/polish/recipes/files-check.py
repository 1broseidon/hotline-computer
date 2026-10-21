"""The file manager opens from the folder handler, and a page's file input
opens the computer's picker through the desktop portal; an agent picks a file
by the picker's accessibility tree and keys, and the page receives it."""
import json, pathlib, sys, time
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import mcp
sh = lambda label, s: json.loads(mcp.text("shell", {"action":"exec","label":label,"command":"bash","args":["-c",s]}))
sh("make sample files", "mkdir -p ~/src/fieldnote/src ~/src/fieldnote/docs ~/Downloads && cd ~/src/fieldnote && printf '# Fieldnote\\n' > README.md && echo {} > package.json && printf 'export const a = 1\\n' > src/index.ts && head -c 48000 /dev/urandom > docs/diagram.png && head -c 1200000 /dev/urandom > ~/Downloads/release-notes.pdf && printf 'notes\\n' > ~/Downloads/meeting.md")
sh("open the file manager", "hotline-computer open ~/src/fieldnote")
page = "data:text/html,<title>Upload</title><body style=\"font:16px sans-serif;padding:40px\"><h2>Attach a document</h2><input type=file id=f accept=\".pdf,.md\"><p id=o></p><script>f.onchange=()=>o.textContent=\"Chosen: \"+[...f.files].map(x=>x.name+\" (\"+x.size+\" bytes)\").join(\", \")</script>"
mcp.text("browser", {"action":"navigate","url":page}); time.sleep(1)
ref = lambda: next(l.split("]")[0][1:] for l in mcp.text("browser", {"action":"text"}).splitlines() if "[file]" in l)
def picker():
    for _ in range(40):
        ws = [w for w in json.loads(mcp.text("windows", {"action":"list"})) if "hotlinepicker" in w["class"].lower()]
        if ws: return ws[0]
        time.sleep(0.25)
mcp.text("browser", {"action":"click_ref","ref":ref()})
win = picker(); time.sleep(1.5)
tree = "\n".join(x for x in mcp.call("capture", {"window": win["id"], "mode": "tree"}) if isinstance(x, str))
def node(role, name):
    line = next(l for l in tree.splitlines() if l.strip().startswith(f"[{role}] {name} "))
    x, y = map(int, line.split(f"{name} ")[1].split()[0].split(",")); w, h = map(int, line.split(f"{name} ")[1].split()[1].split("x"))
    return x + w // 2, y + h // 2
x, y = node("button", "Downloads"); mcp.text("input", {"action":"click","x":x,"y":y}); time.sleep(1)
tree = "\n".join(x for x in mcp.call("capture", {"window": win["id"], "mode": "tree"}) if isinstance(x, str))
x, y = node("list item", "release-notes.pdf"); mcp.text("input", {"action":"click","x":x,"y":y}); time.sleep(0.4)
mcp.text("input", {"action":"key","combo":"Return"}); time.sleep(2.5)
got = json.loads(mcp.text("browser", {"action":"eval","js":"o.textContent"}))
assert got == "Chosen: release-notes.pdf (1200000 bytes)", got
print("page says:", json.loads(mcp.text("browser", {"action":"eval","js":"o.textContent"})))
assert not [w for w in json.loads(mcp.text("windows", {"action":"list"})) if "hotlinepicker" in w["class"].lower()], "the answered picker closed"
print("pickers left:", [w["title"] for w in json.loads(mcp.text("windows", {"action":"list"})) if "hotlinepicker" in w["class"].lower()])
# A fresh picker, left open for the person.
mcp.text("browser", {"action":"eval","js":"o.textContent=''; f.value=''; 'ok'"})
mcp.text("browser", {"action":"click_ref","ref":ref()}); picker(); time.sleep(1.5)
mcp.shot("files-final.png")
