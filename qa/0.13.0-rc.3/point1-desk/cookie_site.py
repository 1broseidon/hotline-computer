"""TEST ONLY: point 1's cookie-share site. HTTPS on :443 in a fixture
container of its own on the container network, as https://<ip>.sslip.io (a
public name that resolves to that address; the name is all that is looked
up). Its certificate is from the run's CA, which the run image's Chromium
trusts through one test-only policy file.

A person is signed in here when their browser sends the session cookie the
person shared with the teammate through the desk (computer.cookies.push).
The site holds only the cookie's SHA-256, never the value, logs nothing and
writes nothing but /_events, which has no values in it.

usage: python3 cookie_site.py DIR   (DIR/conf.json, then DIR/leaf.pem and
DIR/leaf.key, which the harness writes once it knows this container's address)"""
import hashlib
import http.server
import json
import os
import ssl
import sys
import time

DIR = sys.argv[1]
while not (os.path.exists(os.path.join(DIR, "leaf.key")) and os.path.exists(os.path.join(DIR, "conf.json"))):
    time.sleep(0.5)
time.sleep(0.5)
CONF = json.load(open(os.path.join(DIR, "conf.json")))
EVENTS = []


def page(title, body):
    return (f"<!doctype html><html><head><meta charset=utf-8><title>{title}</title></head>"
            f"<body><h1>{title}</h1>{body}</body></html>")


class Site(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def send(self, status, body, kind="text/html; charset=utf-8", headers=None):
        data = body.encode()
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def me(self):
        cookies = dict(c.strip().split("=", 1) for c in (self.headers.get("Cookie") or "").split(";") if "=" in c)
        value = cookies.get(CONF["cookie"])
        return value is not None and hashlib.sha256(value.encode()).hexdigest() == CONF["sha256"]

    def do_GET(self):
        path = self.path.split("?")[0]
        if path == "/_events":
            return self.send(200, json.dumps({"events": EVENTS}), "application/json")
        signed = self.me()
        EVENTS.append({"at": time.time(), "event": f"GET {path} " + ("signed in by shared cookie" if signed else "signed out")})
        if path == "/":
            if signed:
                return self.send(200, page("Cookie site", f"<p>Signed in to the cookie site as {CONF['user']} (shared cookies)</p>"
                                                          "<p><a href=/account>Account</a></p>"))
            return self.send(200, page("Cookie site", "<p>Signed out of the cookie site</p>"))
        if path == "/account":
            if not signed:
                return self.send(303, "", headers={"Location": "/"})
            return self.send(200, page("Account", f"<p>Account: {CONF['user']}</p><p>Plan: {CONF['plan']}</p>"))
        self.send(404, page("Not found", ""))


server = http.server.ThreadingHTTPServer(("0.0.0.0", 443), Site)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(os.path.join(DIR, "leaf.pem"), os.path.join(DIR, "leaf.key"))
server.socket = context.wrap_socket(server.socket, server_side=True)
print("cookie site up", flush=True)
server.serve_forever()
