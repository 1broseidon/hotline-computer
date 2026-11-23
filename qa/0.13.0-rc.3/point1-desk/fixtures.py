#!/usr/bin/env python3
"""TEST ONLY: point 1's API and git service on the host bridge, over HTTPS
with a leaf from the run's CA: the earlier two-condition run's api() and
bare_repo() (tests/protected/two_condition.py), kept running for as long as
the desk run takes. GET /api/user answers only the real key; /git/ is git
http-backend behind Basic auth whose password must be the real key.

usage: fixtures.py STATE_DIR   (reads STATE_DIR/state.json for the key and the
CA; writes STATE_DIR/fixtures.json with the port, and appends one line per
request to STATE_DIR/seen.jsonl: service, user agent, accepted, never a value)"""
import json
import os
import pathlib
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2] / "tests" / "protected"))
import run as R  # noqa: E402
import two_condition as T  # noqa: E402

S = pathlib.Path(sys.argv[1])
state = json.loads((S / "state.json").read_text())
host = state["bridge"]
leaf = (str(S / "ca" / "api.pem"), str(S / "ca" / "api.key"))
bound = R.Listener(host, tls=leaf)
git_root = S / "git"
if not (git_root / "demo.git").exists():
    git_root.mkdir(exist_ok=True)
    T.bare_repo(str(git_root))


class Seen(list):
    def append(self, row):
        super().append(row)
        with open(S / "seen.jsonl", "a") as f:
            f.write(json.dumps({"t": time.time(), "service": row[0], "agent": row[1], "accepted": row[2]}) + "\n")


T.api(bound, state["canaries"]["GH_TOKEN"], str(git_root), Seen())
(S / "fixtures.json").write_text(json.dumps({"api": f"https://{host}:{bound.port}", "port": bound.port, "pid": os.getpid()}))
print(f"API and git on https://{host}:{bound.port}", flush=True)
while True:
    time.sleep(3600)
