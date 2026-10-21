#!/usr/bin/env python3
"""Start an isolated Apple-container QA instance, with a fresh local bearer.

Build first: container build -c 4 -m 6g -t hotline-computer:look-0.11 .
An existing container with this name is deliberately not replaced.
"""
import json
import os
from pathlib import Path
import secrets
import subprocess
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / 'qa/0.11.0-look'
NAME = os.environ.get('NAME', 'hotline-look-011-qa')
PORT = os.environ.get('PORT', '18787')
IMAGE = os.environ.get('IMAGE', 'hotline-computer:look-0.11')

# Apple 1.5 has no no-new-privileges flag. Set it before exec, as uid 1000,
# and fail closed if the guest kernel refuses. Descendants inherit the bit.
ENTRY = '''import ctypes,os
libc=ctypes.CDLL(None,use_errno=True)
if libc.prctl(38,1,0,0,0): raise OSError(ctypes.get_errno(), 'PR_SET_NO_NEW_PRIVS')
os.execv('/usr/bin/hotline-computer',['hotline-computer','boot'])
'''

token = secrets.token_hex(24)
environment = dict(os.environ, HOTLINE_COMPUTER_TOKEN=token)
started = time.monotonic()
subprocess.run(['container', 'run', '-d', '--name', NAME, '--cap-drop', 'ALL',
                '--cpus', '4', '--memory', '4g', '--shm-size', '1g',
                '--ulimit', 'nproc=1024:1024', '-p', f'127.0.0.1:{PORT}:8787',
                '-e', 'HOTLINE_COMPUTER_TOKEN', '-e', 'TZ=UTC',
                '--entrypoint', 'python3', IMAGE, '-c', ENTRY],
               env=environment, check=True)
fd = os.open(ROOT / '.token', os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
with os.fdopen(fd, 'w') as stream:
    stream.write(token + '\n')
deadline = time.monotonic() + 90
while True:
    try:
        with urllib.request.urlopen(f'http://127.0.0.1:{PORT}/health', timeout=2) as response:
            assert response.status == 200
            break
    except (OSError, AssertionError):
        if time.monotonic() >= deadline:
            raise RuntimeError('QA instance did not become healthy within 90 seconds')
        time.sleep(.1)
result = {'runtime': 'Apple container 1.5.0', 'name': NAME, 'port': int(PORT),
          'image': IMAGE, 'run_to_health_seconds': round(time.monotonic() - started, 3),
          'memory_limit_bytes': 4 * 1024**3, 'shm_bytes': 1024**3,
          'capabilities': 'all dropped', 'no_new_privileges': 'prctl before boot',
          'process_limit': 'RLIMIT_NPROC=1024 per uid, not a cgroup PID limit',
          'host_mounts': [], 'published_address': '127.0.0.1'}
(OUT / 'apple-runtime.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result, indent=2))
