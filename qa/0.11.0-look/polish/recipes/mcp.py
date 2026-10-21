"""A small MCP client for driving a running hotline-computer by hand.

Defaults match qa/0.11.0-look/recipes/run-apple.py: port 18787 and the
bearer in the repository's .token. Override with HC_URL / HC_TOKEN_FILE.
"""
import base64, json, os, pathlib, sys, time, urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[4]
URL = os.environ.get("HC_URL", "http://127.0.0.1:18787") + "/mcp"
TOKEN = pathlib.Path(os.environ.get("HC_TOKEN_FILE", ROOT / ".token")).read_text().strip()
HOLDER = os.environ.get("HC_HOLDER", "Tad")
OUT = pathlib.Path(os.environ.get("HC_OUT", pathlib.Path(__file__).resolve().parent.parent / "local"))
OUT.mkdir(parents=True, exist_ok=True)
_opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def _rpc(payload, session=None):
    headers = {
        "Authorization": "Bearer " + TOKEN,
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "X-Computer-Holder": HOLDER,
    }
    if session:
        headers["Mcp-Session-Id"] = session
    req = urllib.request.Request(URL, data=json.dumps(payload).encode(), headers=headers)
    with _opener.open(req, timeout=120) as r:
        sid = r.headers.get("Mcp-Session-Id")
        if r.status == 202:
            return None, sid
        if "text/event-stream" in r.headers.get("Content-Type", ""):
            for line in r:
                if line.startswith(b"data:") and line[5:].strip():
                    msg = json.loads(line[5:])
                    if msg.get("id") == payload.get("id"):
                        return msg, sid
        return json.loads(r.read()), sid


_session = None


def session():
    global _session
    if _session is None:
        _, sid = _rpc({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "drive", "version": "1"}}})
        _rpc({"jsonrpc": "2.0", "method": "notifications/initialized"}, sid)
        _session = sid
    return _session


_id = 10


def call(tool, args=None):
    global _id
    _id += 1
    msg, _ = _rpc({"jsonrpc": "2.0", "id": _id, "method": "tools/call",
                   "params": {"name": tool, "arguments": args or {}}}, session())
    if "error" in msg:
        raise RuntimeError(msg["error"])
    res = msg["result"]
    out = []
    for c in res.get("content", []):
        if c.get("type") == "text":
            out.append(c["text"])
        elif c.get("type") == "image":
            out.append({"image": c["data"]})
    if res.get("isError"):
        raise RuntimeError(f"{tool} {args}: {out}")
    return out


def text(tool, args=None):
    return "\n".join(x for x in call(tool, args) if isinstance(x, str))


def shot(name, region=None):
    """Full-resolution PNG of the desktop (or a region), copied out of the computer."""
    inside = f"/home/agent/.hotline/shots/{name}"
    text("files", {"action": "put", "path": inside, "content": ""})
    args = {"mode": "png", "path": inside}
    if region:
        args["region"] = region
    text("capture", args)
    reply = text("files", {"action": "get", "path": inside})
    data = reply.split("\n", 2)[2] if reply.startswith("encoding=base64") else ""
    (OUT / name).write_bytes(base64.b64decode(data))
    return OUT / name


if __name__ == "__main__":
    tool = sys.argv[1]
    args = json.loads(sys.argv[2]) if len(sys.argv) > 2 else {}
    for x in call(tool, args):
        print(x if isinstance(x, str) else "<image>")
