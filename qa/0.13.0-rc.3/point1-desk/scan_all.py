"""TEST ONLY: point 1's canary scan, the same code on the host and as a job in
the computer (python3, standard library only).

Each canary is searched in these spellings: plain; base64 and base64url at
all three alignments (the canary inside a longer encoded value, as in a Basic
header); reversed; hex; percent-encoded; JSON-escaped; HTML-escaped. The
canaries arrive XOR-masked (MASK, then one masked value per canary, all hex),
so no spelling of any canary is on this program's command line, in its job's
record or in its log: what it finds was there before it ran.

usage: python3 scan_all.py MASKHEX MASKEDHEX... -- PATH...  [--proc]
Paths are files or directories (walked, symlinks not followed; .gz opened);
--proc adds every /proc/*/environ and cmdline this process can read. Prints
HIT <where> canary#<i> <spelling> per finding and one SCANNED summary line."""
import base64
import gzip
import html
import json
import os
import sys
import urllib.parse

LIMIT = 256 << 20
args = sys.argv[1:]
cut = args.index("--")
mask = bytes.fromhex(args[0])
values = [bytes(b ^ mask[i % len(mask)] for i, b in enumerate(bytes.fromhex(h))) for h in args[1:cut]]
rest = args[cut + 1:]
proc = "--proc" in rest
paths = [p for p in rest if p != "--proc"]


def b64_cores(v, url=False):
    """The base64 characters that depend on v alone, at each byte alignment."""
    out = []
    for k in range(3):
        enc = base64.b64encode(b"\0" * k + v + b"\0\0")
        start = -(-8 * k // 6)
        end = (8 * (k + len(v))) // 6
        core = enc[start:end]
        if url:
            core = core.translate(bytes.maketrans(b"+/", b"-_"))
        if len(core) >= 8:
            out.append(core)
    return out


needles = []
for i, v in enumerate(values):
    text = v.decode("utf-8", errors="replace")
    spell = [("plain", v), ("reversed", v[::-1]), ("hex", v.hex().encode()),
             ("percent", urllib.parse.quote(text, safe="").encode()),
             ("json", json.dumps(text)[1:-1].encode()), ("html", html.escape(text).encode())]
    spell += [("base64", c) for c in b64_cores(v)] + [("base64url", c) for c in b64_cores(v, True)]
    seen = set()
    for kind, n in spell:
        if n not in seen:
            seen.add(n)
            needles.append((i, kind, n))


def look(where, data):
    hits = 0
    for i, kind, n in needles:
        if n in data:
            print(f"HIT {where} canary#{i} {kind}", flush=True)
            hits += 1
    return hits


files = hits = skipped = 0
for top in paths:
    walk = [(os.path.dirname(top), [], [os.path.basename(top)])] if os.path.isfile(top) else os.walk(top, followlinks=False)
    for root, _, names in walk:
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
if proc:
    for pid in [p for p in os.listdir("/proc") if p.isdigit()]:
        for part in ("environ", "cmdline"):
            try:
                data = open(f"/proc/{pid}/{part}", "rb").read()
            except OSError:
                continue
            procs += 1
            hits += look(f"/proc/{pid}/{part}", data)
print(f"SCANNED files={files} skipped={skipped} proc_entries={procs} needles={len(needles)} hits={hits}", flush=True)
