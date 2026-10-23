"""Connect as each harness (a real MCP handshake, no holder header) and shoot
the bar through that same session, so nothing else renames the agent."""
import base64, json, os, pathlib, time, urllib.request
ROOT = pathlib.Path(__file__).resolve().parents[4]
URL = os.environ.get("HC_URL", "http://127.0.0.1:18787") + "/mcp"
TOKEN = pathlib.Path(os.environ.get("HC_TOKEN_FILE", ROOT / ".token")).read_text().strip()
OUT = pathlib.Path(os.environ.get("HC_OUT", pathlib.Path(__file__).resolve().parent.parent / "local")); OUT.mkdir(parents=True, exist_ok=True)
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
def post(body, sid=None):
    h = {"Authorization": "Bearer " + TOKEN, "Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if sid: h["Mcp-Session-Id"] = sid
    with opener.open(urllib.request.Request(URL, json.dumps(body).encode(), h), timeout=60) as r:
        sid = r.headers.get("Mcp-Session-Id") or sid
        if r.status == 202: return sid, None
        if "text/event-stream" in r.headers.get("Content-Type", ""):
            for line in r:
                if line.startswith(b"data:") and line[5:].strip():
                    msg = json.loads(line[5:])
                    if msg.get("id") == body.get("id"): return sid, msg
        return sid, json.loads(r.read())
def connect(name, title=None):
    info = {"name": name, "version": "1"} | ({"title": title} if title else {})
    sid, _ = post({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": info}})
    post({"jsonrpc": "2.0", "method": "notifications/initialized"}, sid)
    return sid
def call(sid, tool, args):
    _, msg = post({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": tool, "arguments": args}}, sid)
    return "".join(c.get("text", "") for c in msg["result"]["content"])
def shot(sid, name, region):
    inside = f"/home/agent/.hotline/shots/{name}"
    call(sid, "files", {"action": "put", "path": inside, "content": ""})
    call(sid, "capture", {"mode": "png", "path": inside, "region": region})
    reply = call(sid, "files", {"action": "get", "path": inside})
    (OUT / name).write_bytes(base64.b64decode(reply.split("\n", 2)[2]))
for name, title in [("claude-code", None), ("codex-mcp-client", "Codex"), ("cursor-vscode", None), ("opencode", None),
                    ("github-copilot-cli", None), ("windsurf-client", None), ("Cline", None), ("Roo Code", None),
                    ("zed", "Zed"), ("kilo-code", None), ("gemini-cli-mcp-client", None)]:
    sid = connect(name, title); time.sleep(0.6)
    shot(sid, f"harness-{name.replace(' ', '-').lower()}.png", [0, 0, 300, 40])
    print("shot", name)
