#!/usr/bin/env python3
"""A split computer on a fresh Apple container volume for its home: the
volume comes up root's with an empty lost+found. Boot, run a job, save a
snapshot, empty the home, load it back, then boot the same volume again.
Throwaway container and volume (hl-secrets-rc2-*), made-up content only.
usage: fresh_volume.py IMAGE"""
import json
import os
import secrets
import subprocess
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "../../../tests/protected"))
import run as R  # noqa: E402

image = sys.argv[1]
rt = "container"
tag = secrets.token_hex(3)
name, vol = f"hl-secrets-rc2-vol-{tag}", f"hl-secrets-rc2-home-{tag}"
token = secrets.token_hex(24)
steps = []


def step(what, ok, detail):
    steps.append(ok)
    print(f"{'PASS' if ok else 'FAIL':<5} {what}: {detail}", flush=True)


def home_seen_by_root():
    """The volume as a throwaway root shell sees it: owner, mode, entries, ACL."""
    script = ("import os; s = os.stat('/home/agent'); acl = None\n"
              "try: acl = os.getxattr('/home/agent', 'system.posix_acl_access').hex()\n"
              "except OSError as e: acl = 'none (' + e.strerror + ')'\n"
              "print(s.st_uid, oct(s.st_mode & 0o7777), sorted(os.listdir('/home/agent')), acl)")
    r = R.run([rt, "run", "--rm", "--name", f"{name}-look", "--user", "0", "--cap-drop=ALL", "--cap-add", "CAP_SETUID",
               "--cap-add", "CAP_SETGID", "--entrypoint", "/usr/bin/python3", "-v", f"{vol}:/home/agent", image, "-c", script])
    return r.stdout.strip().splitlines()[-1] if r.stdout.strip() else r.stderr.strip()[-200:]


print("# containers before (none touched):\n" + R.run(R.LIST[rt]).stdout)
R.run([rt, "volume", "create", vol])
try:
    print("# fresh volume:", home_seen_by_root())
    port = R.free_port()
    made = R.create(rt, name, image, True, port, {"HOTLINE_COMPUTER_TOKEN": token}, [f"{vol}:/home/agent"])
    up = made.returncode == 0 and R.up(port)
    logs = R.run([rt, "logs", name]).stdout
    step("a split boot on the fresh volume comes up", up,
         next((l for l in logs.splitlines() if "fresh volume" in l), "no line about the home") if up else logs[-300:])
    if not up:
        sys.exit(1)
    c = R.Computer(port, token)
    out, code = c.sh("mkdir -p ~/src && echo kept-by-the-job > ~/src/notes.txt && id -un && stat -c '%U %a' ~ && ls -A ~ | tr '\\n' ' '")
    step("a job runs and writes in the home", code == 0 and "agent" in out, out.strip().replace("\n", " | "))
    saved, err = c.tool("state", action="snapshot_save", name="fresh")
    step("snapshot_save", not err and '"saved":true' in saved.replace(" ", ""), saved.strip()[:120])
    listed, _ = c.sh("tar tzf ~/.hotline/snapshots/fresh.tar.gz | grep -c lost+found; tar tzf ~/.hotline/snapshots/fresh.tar.gz | grep -c src/notes.txt")
    step("the snapshot holds the job's file and no lost+found", listed.split() == ["0", "1"], "counts lost+found, notes: " + " ".join(listed.split()))
    c.sh("rm -r ~/src")
    loaded, err = c.tool("state", action="snapshot_load", name="fresh")
    back, _ = c.sh("cat ~/src/notes.txt")
    step("snapshot_load brings it back", not err and back.strip() == "kept-by-the-job", loaded.strip()[:100] + " / " + back.strip())
    R.run([rt, "stop", name])
    print("# after the first boot:", home_seen_by_root())
    R.run([rt, "start", name])
    again = R.up(port)
    c = R.Computer(port, token)  # a new boot, a new MCP session
    out, code = c.sh("cat ~/src/notes.txt") if again else ("", None)
    step("the same volume boots again, the home as the agent left it", again and out.strip() == "kept-by-the-job", out.strip() or "did not come up")
finally:
    R.remove(rt, name)
    R.run([rt, "volume", "rm", vol])
    print(f"# removed {name} and {vol}")
sys.exit(0 if steps and all(steps) else 1)
