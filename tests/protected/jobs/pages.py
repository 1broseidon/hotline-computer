#!/usr/bin/env python3
"""TEST ONLY: the probes' little web server, run as a job on a localhost port
inside the computer: ~/site's files for GET, and the few POST routes the
sign-in probes need. It plays a site, so it may echo what a form sent:
/echo answers the posted `p` in the spellings a real page or URL can carry
it (percent, form, base64, HTML entities), /redirect sends it on in a URL,
and anything else answers a plain "done" page.

usage: python3 pages.py PORT"""
import base64
import html
import http.server
import os
import sys
import urllib.parse

ROOT = os.path.expanduser("~/site")


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *a, **kw):
        super().__init__(*a, directory=ROOT, **kw)

    def page(self, status, body, headers=()):
        data = ("<!doctype html><html><head><meta charset=utf-8><title>probe</title></head>"
                f"<body>{body}</body></html>").encode()
        self.send_response(status)
        for k, v in headers:
            self.send_header(k, v)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        form = urllib.parse.parse_qs(self.rfile.read(n).decode(errors="replace"))
        p = (form.get("p") or [""])[0]
        if self.path.startswith("/echo"):
            spelled = [urllib.parse.quote(p, safe=""), urllib.parse.quote(p, safe="").lower(),
                       urllib.parse.quote_plus(p), base64.b64encode(p.encode()).decode(),
                       html.escape(p, quote=True), "".join(f"&#{ord(c)};" for c in p)]
            # Shown as text: each spelling, escaped once more so the page
            # displays it exactly (an entity spelling shows as its entities).
            rows = "".join(f"<p>spelling {i}: {html.escape(s)}</p>" for i, s in enumerate(spelled))
            return self.page(200, "<h1>Echo</h1>" + rows)
        if self.path.startswith("/redirect"):
            return self.page(303, "see other", [("Location", "/shown?v=" + urllib.parse.quote_plus(p))])
        return self.page(200, "<h1>done</h1>")

    def log_message(self, *a):
        pass


http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
