#!/usr/bin/env python3
"""The two-condition proof, with no desk: one throwaway split computer.

Condition 1, the agent's work succeeds. Through the computer's own MCP tools,
as an agent calls them, with made-up credentials delivered by PUT /secrets v2:
  1. sign in to a site with a password and a TOTP code;
  2. post a form on the signed-in site;
  3. download a file from it;
  4. sign out and sign in again with a passkey;
  4b. with only that passkey granted, its site still opens in the signed-in
     browser (not the general one) and the passkey signs in there;
  5. call an API with a protected key through curl, git (a real clone over
     smart HTTP) and Node, in a workspace prepared with Nix as an agent would.
Condition 2, nothing leaks. On the same computer, afterwards: the full probe
suite (probes.py) must be all BLOCKED (SKIPPED only where a probe cannot run
by itself), and a canary scan must find no secret value in the job logs, the
agent's home (scan_home.py, run as a job), the mounted workspace (read on the
host) or any tool result this run received.

Fixtures, all local and test only (no public-domain fixture exists yet):
- The site is jobs/site.py on http://localhost:8600 inside the computer: a
  login's site must be https or localhost, and the signed-in browser trusts
  only real roots. It holds hashes and a public key, never a secret.
- The API and the git service are on the host bridge over HTTPS (fixture.py's
  CA), so the image must be the probe build (--features probe-roots: the only
  difference from the release binary is that the key proxy also trusts that
  CA upstream).

usage: two_condition.py --runtime container --image hotline-computer:0.13.0-rc.1-probe
                        [--n3-from IMAGE] [--prefix hl-secrets-rc] [--out DIR] [--keep]
Exit 0 only if every step succeeded, every probe is BLOCKED or SKIPPED and
the scan found nothing."""
import argparse
import base64
import hashlib
import hmac
import json
import os
import pathlib
import re
import secrets
import shutil
import struct
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import canaries as K  # noqa: E402
import probes as P  # noqa: E402
import run as R  # noqa: E402
from fixture import FixtureCA  # noqa: E402

HERE = pathlib.Path(__file__).resolve().parent
SITE_PORT = 8600
SITE = f"http://localhost:{SITE_PORT}"


# --- made-up credentials for this run ------------------------------------------

def totp(seed, step):
    key = base64.b32decode(seed + "=" * (-len(seed) % 8))
    digest = hmac.new(key, struct.pack(">Q", step), hashlib.sha1).digest()
    o = digest[19] & 15
    return "%06d" % ((struct.unpack(">I", digest[o:o + 4])[0] & 0x7FFFFFFF) % 1000000)


def passkey(tmp):
    """A throwaway P-256 key: (PKCS#8 base64 for the vault entry, x hex, y hex)."""
    key = os.path.join(tmp, "pk.pem")
    subprocess.run(["openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256", "-out", key], check=True, capture_output=True)
    der = subprocess.run(["openssl", "pkcs8", "-topk8", "-nocrypt", "-in", key, "-outform", "DER"], check=True, capture_output=True).stdout
    pub = subprocess.run(["openssl", "pkey", "-in", key, "-pubout", "-outform", "DER"], check=True, capture_output=True).stdout[-65:]
    os.remove(key)
    return base64.b64encode(der).decode(), pub[1:33].hex(), pub[33:].hex()


class Run:
    def __init__(self):
        self.steps = []

    def step(self, name, ok, detail):
        self.steps.append({"step": name, "ok": bool(ok), "detail": detail})
        print(f"{'PASS' if ok else 'FAIL':<5} {name}: {detail}", flush=True)
        return ok


# --- the API and git service the protected key is bound to ----------------------

def api(listener, key, git_root, seen):
    """GET /api/user answers only the real key; /git/ is git http-backend
    behind Basic auth whose password must be the real key."""
    def user(method, path, headers, body):
        auth = headers.get("Authorization", "")
        agent = headers.get("User-Agent", "")
        if auth != f"Bearer {key}":
            seen.append(("api", agent, False))
            return 401, {"Content-Type": "application/json"}, b'{"message":"Bad credentials"}'
        seen.append(("api", agent, True))
        return 200, {"Content-Type": "application/json"}, json.dumps({"login": "rc-user", "plan": "fixture"}).encode()

    def git(method, path, headers, body):
        auth = headers.get("Authorization", "")
        m = re.match(r"Basic (\S+)", auth)
        good = bool(m) and base64.b64decode(m.group(1)).decode(errors="replace").split(":", 1)[-1] == key
        if not good:
            seen.append(("git", headers.get("User-Agent", ""), False))
            return 401, {"WWW-Authenticate": 'Basic realm="fixture"'}, b""
        seen.append(("git", headers.get("User-Agent", ""), True))
        route, _, query = path[len("/git"):].partition("?")
        env = dict(os.environ, GIT_PROJECT_ROOT=git_root, GIT_HTTP_EXPORT_ALL="1", PATH_INFO=route,
                   QUERY_STRING=query, REQUEST_METHOD=method, CONTENT_TYPE=headers.get("Content-Type", ""),
                   CONTENT_LENGTH=str(len(body)), REMOTE_USER="rc-user")
        if headers.get("Content-Encoding"):
            env["HTTP_CONTENT_ENCODING"] = headers["Content-Encoding"]
        out = subprocess.run(["git", "http-backend"], input=body, env=env, capture_output=True).stdout
        head, _, rest = out.partition(b"\r\n\r\n")
        status, hdrs = 200, {}
        for line in head.decode().split("\r\n"):
            k, _, v = line.partition(": ")
            if k.lower() == "status":
                status = int(v.split()[0])
            elif k:
                hdrs[k] = v
        return status, hdrs, rest

    listener.routes["/api/"] = user
    listener.routes["/git/"] = git


def bare_repo(root):
    work = os.path.join(root, "work")
    subprocess.run(["git", "init", "-q", "-b", "main", work], check=True)
    pathlib.Path(work, "README.md").write_text("Fixture repository for the two-condition run.\n")
    git = ["git", "-C", work, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.test"]
    subprocess.run(git + ["add", "README.md"], check=True)
    subprocess.run(git + ["commit", "-q", "-m", "Fixture commit for the two-condition run"], check=True)
    subprocess.run(["git", "clone", "-q", "--bare", work, os.path.join(root, "demo.git")], check=True)
    subprocess.run(["git", "-C", os.path.join(root, "demo.git"), "config", "http.receivepack", "false"], check=True)


# --- condition 1 ---------------------------------------------------------------

def ref_for(text, kind, words=""):
    for m in re.finditer(r"\[(e\d+)\] \[(\w+)\]([^\n]*)", text):
        if m.group(2) == kind and words.lower() in m.group(3).lower():
            return m.group(1)
    return None


def look(c, want, seconds=20):
    """browser text until `want` shows: a tab mid sign-in is hidden until it loads a new page."""
    text = ""
    for _ in range(seconds * 2):
        text, _ = c.tool("browser", action="text")
        if want in text:
            return text
        time.sleep(0.5)
    return text


def agent_work(c, run, host, api_port, out, deliver):
    sh = c.sh
    # The site, started as a job (the computer's only way to run one).
    c.tool("shell", action="start", command=f"exec python3 ~/probe/site.py ~/probe/site.json", label="Fixture site (test only)", wait_ms=500)
    for _ in range(20):
        if "200" in sh(f"curl -s -o /dev/null -w '%{{http_code}}' {SITE}/")[0]:
            break
        time.sleep(0.5)

    # 1. Password and TOTP.
    c.tool("browser", action="navigate", url=SITE + "/login")
    text = look(c, "Password")
    u, p, b = ref_for(text, "text"), ref_for(text, "password"), ref_for(text, "button", "sign in")
    filled = [c.tool("browser", action="fill", ref=u, secret="SITE.username")[1],
              c.tool("browser", action="fill", ref=p, secret="SITE.password")[1]]
    c.tool("browser", action="click_ref", ref=b)
    text = look(c, "Two-step verification")
    code, verify = ref_for(text, "text"), ref_for(text, "button", "verify")
    filled.append(c.tool("browser", action="fill", ref=code, secret="SITE.code")[1])
    c.tool("browser", action="click_ref", ref=verify)
    text = look(c, "Signed in as")
    run.step("sign in with password and TOTP", "Signed in as rc-user (password and code)" in text and not any(filled),
             (re.search(r"Signed in as[^\n]*", text) or [text[:80]])[0])

    # 2. A post on the signed-in site.
    c.tool("browser", action="navigate", url=SITE + "/post")
    text = look(c, "New post")
    c.tool("browser", action="fill", ref=ref_for(text, "text"), text="Release check from the two-condition run")
    c.tool("browser", action="click_ref", ref=ref_for(text, "button", "publish"))
    text = look(c, "Post 1:")
    run.step("post a form on the signed-in site", "Post 1: Release check from the two-condition run" in text,
             (re.search(r"Post 1:[^\n]*", text) or [text[:80]])[0])

    # 3. A download.
    c.tool("browser", action="navigate", url=SITE + "/files")
    text = look(c, "report.csv")
    c.tool("browser", action="click_ref", ref=ref_for(text, "link", "report.csv"))
    got = ""
    for _ in range(30):
        got = sh("cat ~/Downloads/report.csv 2>/dev/null")[0]
        if got:
            break
        time.sleep(1)
    c.tool("browser", action="text")  # the next look tells the agent about the download
    run.step("download a file from the signed-in site", got == "id,amount\n1,42\n", f"~/Downloads/report.csv: {got!r}")

    # 4. Sign out, then a passkey.
    c.tool("browser", action="navigate", url=SITE + "/logout")
    look(c, "Signed out")
    c.tool("browser", action="navigate", url=SITE + "/passkey")
    text = look(c, "Sign in with a passkey")
    c.tool("browser", action="click_ref", ref=ref_for(text, "button", "passkey"))
    text = look(c, "Signed in as", 30)
    run.step("sign in with a passkey", "Signed in as rc-user (passkey)" in text,
             (re.search(r"(Signed in as|Passkey)[^\n]*", text) or [text[:80]])[0])
    events = json.loads(sh(f"curl -s {SITE}/_events")[0] or "{}")
    run.step("the site saw each step succeed",
             all(any(e["event"] == want for e in events.get("events", [])) for want in
                 ("password accepted", "code accepted", "signed in by password and code", "posted",
                  "downloaded report.csv", "passkey verified", "signed in by passkey")),
             ", ".join(e["event"] for e in events.get("events", [])))

    # 4b. Only the passkey granted: no login names the site, so its RP id is
    # what sends it to the signed-in browser. Withdrawing the login signs the
    # site out, so this sign-in is the passkey's alone.
    status, said = deliver(["SITEKEY"], 2)
    time.sleep(2)  # the signed-in browser notices the withdrawn login within a second
    c.tool("browser", action="navigate", url=SITE + "/passkey")
    text = look(c, "Sign in with a passkey")
    tabs = json.loads(c.tool("browser", action="tabs")[0] or "[]")
    here = [t for t in tabs if t.get("signedIn") and str(t.get("url", "")).startswith(SITE)]
    general = [t for t in tabs if not t.get("signedIn") and str(t.get("url", "")).startswith(SITE)]
    c.tool("browser", action="click_ref", ref=ref_for(text, "button", "passkey"))
    text = look(c, "Signed in as", 30)
    events = json.loads(sh(f"curl -s {SITE}/_events")[0] or "{}")
    by_passkey = sum(e["event"] == "signed in by passkey" for e in events.get("events", []))
    run.step("with only a passkey granted, its site opens in the signed-in browser and the passkey signs in",
             status == 204 and here and not general and "Signed in as rc-user (passkey)" in text and by_passkey >= 2,
             f"PUT {status} {said[:60]!r}; signed-in tabs on the site {len(here)}, general {len(general)}; "
             + (re.search(r"(Signed in as|Passkey)[^\n]*", text) or [text[:80]])[0] + f"; {by_passkey} passkey sign-ins")
    status, said = deliver(None, 3)
    run.step("the whole set delivered again", status == 204, f"HTTP {status} {said[:80]!r}")

    # 5. The API with a protected key, in a workspace prepared as an agent would.
    ws = "/home/agent/workspace/api"
    url = f"https://{host}:{api_port}"
    c.tool("files", action="put", path="workspace/api/.hotline/manifest.json",
           content=json.dumps({"packages": ["nodejs_24"]}))
    prep, _ = c.tool("state", action="prepare", workspace=ws)
    deadline = time.time() + 1200
    while '"ready":true' not in prep.replace(" ", "") and time.time() < deadline:
        m = re.search(r'"job"\s*:\s*\{[^}]*"id"\s*:\s*"([^"]+)"', prep) or re.search(r'"job_id"\s*:\s*"([^"]+)"', prep)
        if m:
            c.tool("shell", action="wait", job_id=m.group(1), wait_ms=50000)
        prep, _ = c.tool("state", action="manifest", workspace=ws)
    ready = '"ready":true' in prep.replace(" ", "")
    run.step("prepare a workspace with Node (Nix)", ready, "ready" if ready else prep[:160])
    node = "node" if ready else P.node(type("C", (), {"c": c})())
    where = {"cwd": ws} if ready else {}

    def job(line):
        text, _ = c.tool("shell", action="exec", command=line, wait_ms=45000, **where)
        try:
            r = json.loads(text)
            return r.get("exit_code"), (r.get("stdout") or "") + (r.get("stderr") or "")
        except Exception:
            return None, text

    code_, said = job(f'curl -fsS {url}/api/user -H "Authorization: Bearer $GH_TOKEN" | tee curl-user.json')
    run.step("call the API with the protected key: curl", code_ == 0 and '"login": "rc-user"' in said, said.strip()[:100])
    helper = "!f(){ echo username=x-access-token; echo password=$GH_TOKEN; }; f"
    code_, said = job(f"git -c credential.helper='{helper}' clone -q {url}/git/demo.git demo && git -C demo log --oneline")
    run.step("clone over HTTPS with the protected key: git", code_ == 0 and "Fixture commit" in said, said.strip()[-100:])
    code_, said = job(f"{node} -e \"fetch('{url}/api/user',{{headers:{{authorization:'Bearer '+process.env.GH_TOKEN}}}})"
                      ".then(async r=>{console.log(r.status, await r.text()); process.exit(r.ok?0:1)})"
                      ".catch(e=>{console.log('error',(e.cause&&e.cause.code)||e.message);process.exit(1)})\"")
    run.step("call the API with the protected key: node", code_ == 0 and "rc-user" in said, said.strip()[:100])
    code_, said = job('printf %s "$GH_TOKEN" | head -c 4; echo; env | grep -c "^HTTPS_PROXY=" ')
    run.step("the job held a stand-in, not the key", K.GH not in said and said.startswith("ghp_"),
             "the job's $GH_TOKEN starts " + said.split()[0] if said.split() else said)


# --- condition 2: the scan -----------------------------------------------------

def spellings(values):
    for i, v in enumerate(values):
        b = v.encode()
        yield i, "plain", b
        yield i, "base64", base64.b64encode(b)
        yield i, "reversed", b[::-1]


def scan_bytes(where, data, values):
    return [f"{where} canary#{i} {kind}" for i, kind, n in spellings(values) if n in data]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runtime", default="container", choices=list(R.LIST))
    ap.add_argument("--image", required=True, help="a probe build (--features probe-roots) of the release image")
    ap.add_argument("--n3-from", default="", help="N3's 0.12 image (default: --image as 0.49 creates it)")
    ap.add_argument("--prefix", default="hl-secrets-rc")
    ap.add_argument("--out", default="")
    ap.add_argument("--keep", action="store_true")
    a = ap.parse_args()
    rt = a.runtime
    out = pathlib.Path(a.out or tempfile.mkdtemp(prefix=a.prefix + "-2c-"))
    out.mkdir(parents=True, exist_ok=True)
    run = Run()

    print(f"# containers already running ({rt}); none of them is touched:")
    print(R.run(R.LIST[rt]).stdout.rstrip() or "(none)")
    tmp = tempfile.mkdtemp(prefix=a.prefix + "-2c-keys-")
    name = f"{a.prefix}-2c-{secrets.token_hex(3)}"
    token = secrets.token_hex(24)
    port = R.free_port()
    ws = pathlib.Path(tempfile.mkdtemp(prefix=a.prefix + "-2c-ws-"))
    os.chmod(ws, 0o777)
    ca = FixtureCA()
    listeners = []
    try:
        made = R.create(rt, name, a.image, True, port,
                        {"HOTLINE_COMPUTER_TOKEN": token, "HOTLINE_KEYPROXY_PROBE_ROOT": ca.pem},
                        [f"{ws}:/home/agent/workspace"])
        if made.returncode != 0 or not R.up(port):
            print("the computer did not come up:", made.stderr.strip()[:200])
            return 2
        c = R.Computer(port, token)
        c.record = []
        host = R.host_address(c)
        listener = R.Listener(host)
        leaf = ca.leaf(host)
        bound, free = R.Listener(host, tls=leaf), R.Listener(host, tls=leaf)
        listeners = [listener, bound, free]
        origin = f"https://{host}:{bound.port}"
        seen = []
        git_root = os.path.join(tmp, "git")
        os.makedirs(git_root)
        bare_repo(git_root)
        api(bound, K.GH, git_root, seen)

        # The made-up credentials of this run, and the probes' canaries.
        password = "hl-rc-pw-" + secrets.token_hex(6)
        seed = base64.b32encode(secrets.token_bytes(20)).decode().rstrip("=")
        private, x, y = passkey(tmp)
        cred = base64.b64encode(secrets.token_bytes(16)).decode()
        body = json.loads(K.body("protected", origin)[0])
        body["secrets"]["SITE"] = {"kind": "login", "sites": [SITE], "username": "rc-user", "password": password, "totp": seed}
        body["secrets"]["SITEKEY"] = {"kind": "passkey", "rpId": "localhost", "credentialId": cred,
                                      "userHandle": base64.b64encode(b"rc-user-1").decode(), "privateKey": private, "userName": "rc-user"}
        canaries = K.ALL + [K.TOTP_SEED, password, seed, private]
        status, _, said = c.http("PUT", "/secrets", raw=json.dumps(body).encode(), headers={"Content-Type": K.V2})

        def deliver(names, generation):
            """PUT /secrets v2 with only the entries `names`, or the whole set again when None."""
            sent = {"v": 2, "generation": generation,
                    "secrets": {k: v for k, v in body["secrets"].items() if names is None or k in names}}
            got, _, reply = c.http("PUT", "/secrets", raw=json.dumps(sent).encode(), headers={"Content-Type": K.V2})
            return got, reply
        _, _, caps = c.http("GET", "/capabilities?nonce=2c")
        print(f"# {a.image} as {name}: {caps.decode()[:200]}")
        print(f"# API and git on {origin} (bound), site on {SITE} in the computer, workspace {ws}")
        if not run.step("deliver the made-up secrets with PUT /secrets v2", status == 204, f"HTTP {status} {said[:80]!r}"):
            return 1

        step = int(time.time()) // 30
        salt = secrets.token_hex(8)
        conf = {"port": SITE_PORT, "username": "rc-user", "pw_salt": salt,
                "pw_hash": hashlib.sha256((salt + password).encode()).hexdigest(),
                "totp_hashes": [hashlib.sha256(totp(seed, s).encode()).hexdigest() for s in range(step - 2, step + 120)],
                "cred_id": cred, "pub_x": x, "pub_y": y, "report": "id,amount\n1,42\n"}
        c.sh("mkdir -p ~/probe")
        c.tool("files", action="put", path="probe/site.py", content=(HERE / "jobs/site.py").read_text())
        c.tool("files", action="put", path="probe/site.json", content=json.dumps(conf))
        c.tool("files", action="put", path="probe/job.py", content=(HERE / "jobs/job.py").read_text())
        c.tool("files", action="put", path="probe/scan_home.py", content=(HERE / "jobs/scan_home.py").read_text())

        print("## condition 1: the agent's work")
        agent_work(c, run, host, bound.port, out, deliver)
        good = [s for s in seen if s[2]]
        run.step("the API and git service received the real key (host side)",
                 any(s[0] == "api" and "curl" in s[1] for s in good) and any(s[0] == "git" for s in good)
                 and any(s[0] == "api" and "node" in s[1].lower() for s in good),
                 f"{len(good)} keyed requests accepted ({', '.join(sorted({s[0] + ':' + s[1].split('/')[0] for s in good}))}), "
                 f"{len(seen) - len(good)} refused (git's first, unauthenticated ask)")

        print("## condition 2: every probe, on this same computer")
        ctx = R.Ctx(c, listener, host, ws, token, rt, name, "protected", bound, free)
        ctx.split = True
        ctx.image, ctx.n3_from, ctx.prefix = a.image, a.n3_from or a.image, a.prefix
        ctx.job_source = (HERE / "jobs/job.py").read_text()
        launch = R.launcher(rt, token)

        def recorded(*args, **kw):  # N3's own computers' answers are this run's tool results too
            other, other_port = launch(*args, **kw)
            other.record = c.record
            return other, other_port
        ctx.launch = recorded
        ctx.remove = lambda n: R.remove(rt, n)
        ctx.volume = lambda v: R.home_volume(rt, v, ctx.n3_from)
        ctx.unvolume = lambda v: R.run([rt, "volume", "rm", v])
        results, bad = R.probe_all(ctx, "blocked")
        run.step("every probe BLOCKED (SKIPPED where it cannot run by itself)", bad == 0,
                 f"{sum(r[1] == 'BLOCKED' for r in results)} blocked, {sum(r[1] == 'SKIPPED' for r in results)} skipped, "
                 f"{sum(r[1] == 'LEAKED' for r in results)} leaked, {sum(r[1] == 'BROKEN' for r in results)} broken")

        print("## condition 2: the canary scan")
        # A control: a made-up value planted in the home, the workspace, a job
        # log and a tool result. Each scan must find it, and only it.
        control = "hl-rc-scan-control-" + secrets.token_hex(4)
        c.sh(f"printf %s {control} > ~/probe/control.txt && cp ~/probe/control.txt ~/workspace/control.txt && cat ~/probe/control.txt")
        values = canaries + [control]
        last = len(values) - 1

        def clean(hits):
            """(no real canary found, the control found)."""
            return (not [h for h in hits if f"canary#{last} " not in h + " "],
                    any(f"canary#{last} " in h + " " for h in hits))
        hexes = " ".join(v.encode().hex() for v in values)
        text, _ = c.tool("shell", action="exec", command=f"python3 ~/probe/scan_home.py {hexes}", wait_ms=45000,
                         label="Canary scan of the home")
        try:
            r = json.loads(text)
            said = (r.get("stdout") or "") + (r.get("stderr") or "")
        except Exception:
            said = text
        summary = re.search(r"SCANNED .*", said)
        hits = [h[4:] for h in re.findall(r"^HIT .*", said, re.M)]
        none, seen_control = clean(hits)
        logs = [h for h in hits if "/.hotline/jobs/" in h and f"canary#{last} " in h + " "]
        run.step("canary scan: job logs, the home, readable /proc", summary and none and seen_control and logs,
                 (summary.group(0) if summary else said[-120:]) + f"; control found in {len(hits)} places, {len(logs)} of them job logs"
                 + ("" if none else "; FOUND: " + "; ".join(h for h in hits if f"canary#{last} " not in h + " ")[:400]))
        found = []
        for path in ws.rglob("*"):
            if path.is_file() and not path.is_symlink():
                found += scan_bytes(str(path.relative_to(ws)), path.read_bytes(), values)
        none, seen_control = clean(found)
        run.step("canary scan: the mounted workspace on the host", none and seen_control,
                 f"{sum(1 for p in ws.rglob('*') if p.is_file())} files, control found"
                 + ("" if none else "; FOUND: " + "; ".join(h for h in found if f"canary#{last} " not in h + " ")[:400]))
        found = []
        for n, (tool, blocks) in enumerate(c.record):
            for block in blocks:
                if block.get("type") == "text":
                    found += scan_bytes(f"result {n} ({tool})", block.get("text", "").encode(), values)
        none, seen_control = clean(found)
        run.step("canary scan: every tool result of this run", none and seen_control,
                 f"{len(c.record)} results, control found"
                 + ("" if none else "; FOUND: " + "; ".join(h for h in found if f"canary#{last} " not in h + " ")[:400]))
        (out / "tool-results.json").write_text(json.dumps(
            [(tool, [b if b.get("type") == "text" else {"type": b.get("type")} for b in blocks]) for tool, blocks in c.record], indent=1))
        return 0 if all(s["ok"] for s in run.steps) else 1
    finally:
        (out / "report.json").write_text(json.dumps({"image": a.image, "steps": run.steps}, indent=2))
        print("## summary")
        for s in run.steps:
            print(f"| {s['step']} | {'PASS' if s['ok'] else 'FAIL'} |")
        print(f"# report in {out}")
        for listener in listeners:
            listener.server.shutdown()
        ca.remove()
        shutil.rmtree(tmp, ignore_errors=True)
        if not a.keep:
            r = R.remove(rt, name)
            print(f"# removed {name}" if r.returncode == 0 else f"# could not remove {name}: {r.stderr.strip()}")
        else:
            print(f"# kept {name} (port {port}, token {token})")


if __name__ == "__main__":
    sys.exit(main())
