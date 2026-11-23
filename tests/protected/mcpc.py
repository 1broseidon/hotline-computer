"""A small MCP streamable-HTTP client and bearer-only HTTP helper for the
throwaway container, playing the agent (tools) or the desk (bearer routes).
Standard library only."""
import json
import urllib.error
import urllib.request


class Computer:
    def __init__(self, port, token):
        self.base = f"http://127.0.0.1:{port}"
        self.token = token
        self.sid = None
        self.n = 10

    def http(self, method, path, body=None, headers=None, raw=None, timeout=240):
        h = {"Authorization": f"Bearer {self.token}"}
        h.update(headers or {})
        data = raw
        if body is not None:
            data = json.dumps(body).encode()
            h["Content-Type"] = "application/json"
        req = urllib.request.Request(self.base + path, data=data, method=method, headers=h)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return r.status, dict(r.headers), r.read()
        except urllib.error.HTTPError as e:
            return e.code, dict(e.headers), e.read()

    @staticmethod
    def _parse(body):
        body = body.decode(errors="replace")
        if body.lstrip().startswith("{"):
            return json.loads(body)
        out = None
        for line in body.splitlines():
            if line.startswith("data:"):
                try:
                    out = json.loads(line[5:].strip())
                except Exception:
                    pass
        return out

    def _init(self):
        acc = {"Accept": "application/json, text/event-stream"}
        _, h, _ = self.http("POST", "/mcp", {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "protected-probes", "version": "0"}}}, acc)
        self.sid = {k.lower(): v for k, v in h.items()}.get("mcp-session-id")
        if self.sid:
            acc["Mcp-Session-Id"] = self.sid
        self.http("POST", "/mcp", {"jsonrpc": "2.0", "method": "notifications/initialized"}, acc)
        self.acc = acc

    def call(self, tool_name, _record=True, **args):
        """Call a tool; returns (content blocks, error_flag), exactly as the agent receives them.
        _record=False keeps it out of `record` (a page the harness itself served the canary on)."""
        if self.sid is None:
            self._init()
        self.n += 1
        _, _, b = self.http("POST", "/mcp", {"jsonrpc": "2.0", "id": self.n, "method": "tools/call",
                                             "params": {"name": tool_name, "arguments": args}}, self.acc)
        r = self._parse(b)
        if r is None:
            return [{"type": "text", "text": b.decode(errors="replace")[:2000]}], True
        res = r.get("result", r)
        blocks = list(res.get("content", [])) if isinstance(res, dict) else []
        err = bool(isinstance(res, dict) and res.get("isError")) or "error" in r
        if "error" in r:
            blocks.append({"type": "text", "text": json.dumps(r["error"])})
        if self.record is not None and _record:
            self.record.append((tool_name, blocks))
        return blocks, err

    record = None  # a list to keep every tool result in, for a canary scan

    def tool(self, tool_name, **args):
        """Call a tool; returns (text, error_flag). Image blocks come back as '<image>'."""
        blocks, err = self.call(tool_name, **args)
        text = [c["text"] if c.get("type") == "text" else f"<{c.get('type')} block>" for c in blocks]
        return "\n".join(text), err

    @staticmethod
    def image(blocks):
        """The first image a tool answered with (base64 PNG), or None."""
        return next((c.get("data") for c in blocks if c.get("type") == "image"), None)

    def sh(self, command, wait_ms=60000):
        """Run a shell line as a job and return (stdout, exit_code) as the agent sees them (redacted)."""
        text, _ = self.tool("shell", action="exec", command=command, wait_ms=wait_ms)
        try:
            r = json.loads(text)
            return (r.get("stdout") or "") + (r.get("stderr") or ""), r.get("exit_code")
        except Exception:
            return text, None
