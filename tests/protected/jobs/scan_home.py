"""TEST ONLY: the canary scan a job can run, as the agent. Every file under
the home the agent can read (job logs, the browser profile, Downloads, the
mounted workspace, snapshots: gzip ones opened), and every /proc/*/environ
and cmdline it can read, is searched for each canary in its plain, base64 and
reversed spellings. The canaries arrive hex-encoded, a spelling nothing
searches for, so this job's own command line and log never match.

Prints one HIT line per finding (path, canary index, spelling) and a summary.
usage: python3 scan_home.py HEX [HEX ...]"""
import base64
import gzip
import os
import sys

LIMIT = 64 << 20
needles = []
for i, h in enumerate(sys.argv[1:]):
    v = bytes.fromhex(h)
    needles += [(i, "plain", v), (i, "base64", base64.b64encode(v)), (i, "reversed", v[::-1])]


def look(where, data):
    hits = 0
    for i, kind, n in needles:
        if n in data:
            print(f"HIT {where} canary#{i} {kind}")
            hits += 1
    return hits


files = hits = skipped = 0
home = os.path.expanduser("~")
for root, dirs, names in os.walk(home, followlinks=False):
    for name in names:
        path = os.path.join(root, name)
        try:
            if os.path.islink(path) or not os.path.isfile(path):
                continue
            if os.path.getsize(path) > LIMIT:
                skipped += 1
                continue
            data = open(path, "rb").read()
        except OSError:
            skipped += 1
            continue
        files += 1
        hits += look(path, data)
        if name.endswith(".gz"):
            try:
                hits += look(path + " (unpacked)", gzip.decompress(data))
            except Exception:
                pass
procs = 0
for pid in [p for p in os.listdir("/proc") if p.isdigit()]:
    for part in ("environ", "cmdline"):
        try:
            data = open(f"/proc/{pid}/{part}", "rb").read()
        except OSError:
            continue
        procs += 1
        hits += look(f"/proc/{pid}/{part}", data)
print(f"SCANNED files={files} skipped={skipped} proc_entries={procs} hits={hits}")
