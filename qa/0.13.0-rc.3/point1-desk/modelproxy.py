#!/usr/bin/env python3
"""TEST ONLY: the model request log for point 1 (Shield's B5).

A reverse proxy on 127.0.0.1 between the agent and its model: the desk's
Ollama connection is made with this proxy as its base URL (Hotline Agent on a
local model), or Claude Code is pointed at it with ANTHROPIC_BASE_URL (the
desk hands its own environment to the harness). Every
request is logged whole, body and all, with its credential headers replaced
by "<redacted>"; every answer is streamed back to the agent unchanged and
logged whole too. This is what the model was sent and what it said, byte
for byte, not a reading of code.

usage: modelproxy.py PORT LOGDIR [UPSTREAM]   (default https://api.anthropic.com)
Writes LOGDIR/requests.jsonl ({n, t, method, path, headers, body}) and
LOGDIR/responses.jsonl ({n, t, status, headers, body})."""
import http.client
import http.server
import json
import os
import ssl
import sys
import threading
import time

import urllib.parse

PORT, LOGDIR = int(sys.argv[1]), sys.argv[2]
os.makedirs(LOGDIR, exist_ok=True)
_up = urllib.parse.urlsplit(sys.argv[3] if len(sys.argv) > 3 else "https://api.anthropic.com")
UPSTREAM = _up.netloc
SECRET_HEADERS = {"authorization", "x-api-key", "cookie", "proxy-authorization"}
HOP = {"connection", "keep-alive", "transfer-encoding", "te", "trailer", "upgrade", "proxy-connection", "host",
       "content-length", "accept-encoding"}
lock = threading.Lock()
counter = [0]
REQ = open(os.path.join(LOGDIR, "requests.jsonl"), "a")
RESP = open(os.path.join(LOGDIR, "responses.jsonl"), "a")
CTX = ssl.create_default_context()


def write(f, row):
    with lock:
        f.write(json.dumps(row) + "\n")
        f.flush()


class Proxy(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def _do(self):
        with lock:
            counter[0] += 1
            n = counter[0]
        length = int(self.headers.get("Content-Length") or 0)
        if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
            body = b""
            while True:
                size = int(self.rfile.readline().strip() or b"0", 16)
                if size == 0:
                    self.rfile.readline()
                    break
                body += self.rfile.read(size)
                self.rfile.readline()
        else:
            body = self.rfile.read(length) if length else b""
        shown = {k: ("<redacted>" if k.lower() in SECRET_HEADERS else v) for k, v in self.headers.items()}
        write(REQ, {"n": n, "t": time.time(), "method": self.command, "path": self.path, "headers": shown,
                    "body": body.decode("utf-8", errors="replace")})
        headers = {k: v for k, v in self.headers.items() if k.lower() not in HOP}
        headers["Host"] = UPSTREAM
        headers["Accept-Encoding"] = "identity"  # the log stays readable text
        up = (http.client.HTTPSConnection(_up.hostname, _up.port or 443, context=CTX, timeout=900) if _up.scheme == "https"
              else http.client.HTTPConnection(_up.hostname, _up.port or 80, timeout=900))
        try:
            up.request(self.command, self.path, body=body or None, headers=headers)
            r = up.getresponse()
        except Exception as e:
            write(RESP, {"n": n, "t": time.time(), "status": 502, "error": repr(e)})
            self.send_response(502)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        self.send_response(r.status, r.reason)
        for k, v in r.getheaders():
            if k.lower() not in HOP:
                self.send_header(k, v)
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()
        got = bytearray()
        while True:
            chunk = r.read1(65536) if hasattr(r, "read1") else r.read(65536)
            if not chunk:
                break
            got += chunk
            try:
                self.wfile.write(b"%x\r\n%s\r\n" % (len(chunk), chunk))
                self.wfile.flush()
            except OSError:
                break
        try:
            self.wfile.write(b"0\r\n\r\n")
            self.wfile.flush()
        except OSError:
            pass
        write(RESP, {"n": n, "t": time.time(), "status": r.status,
                     "headers": {k: v for k, v in r.getheaders() if k.lower() != "set-cookie"},
                     "body": bytes(got).decode("utf-8", errors="replace")})
        up.close()

    do_GET = do_POST = do_PUT = do_DELETE = do_PATCH = do_HEAD = _do


http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Proxy).serve_forever()
