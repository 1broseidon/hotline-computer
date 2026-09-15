#!/usr/bin/env python3
"""Crash only the container supplied by the acceptance runner and verify recovery."""
import argparse
import json
from pathlib import Path
import subprocess
import time
import urllib.request

from acceptance import Computer, execute


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--container', required=True)
    parser.add_argument('--url', required=True)
    parser.add_argument('--token-file', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    output = args.output / 'recovery'
    output.mkdir(exist_ok=True)
    token = args.token_file.read_text().strip()
    c = Computer(args.url, token, output)
    job = c.call('shell', {'action': 'start', 'command': 'bash', 'args': ['-c', 'printf "retained before restart\\n"; sleep 120'], 'label': 'Container restart acceptance'})
    deadline = time.monotonic()+5
    while 'retained before restart' not in c.call('shell', {'action': 'read', 'job_id': job['id']})['output']:
        assert time.monotonic()<deadline, 'job never produced its initial output'
        time.sleep(.05)
    subprocess.run(['docker', 'kill', args.container], check=True, stdout=subprocess.DEVNULL)
    started = time.monotonic()
    subprocess.run(['docker', 'start', args.container], check=True, stdout=subprocess.DEVNULL)
    port = subprocess.check_output(['docker', 'port', args.container, '8787/tcp'], text=True).strip().rsplit(':', 1)[1]
    url = 'http://127.0.0.1:' + port
    args.output.joinpath('url.txt').write_text(url)
    for _ in range(150):
        try:
            with urllib.request.urlopen(url+'/health', timeout=2):
                break
        except OSError:
            time.sleep(.1)
    else:
        raise AssertionError('computer did not recover after restart')
    startup = time.monotonic()-started
    c = Computer(url, token, output)
    recovered = c.call('shell', {'action': 'status', 'job_id': job['id']})
    assert recovered['state'] == 'interrupted', recovered
    assert 'retained before restart' in c.call('shell', {'action': 'read', 'job_id': job['id']})['output']
    # The full suite prepared these packages before the restart; the store keeps them.
    prepared = c.call('state', {'action': 'prepare', 'packages': ['python312', 'uv'], 'workspace': '/home/agent/qa/recovered'})
    assert prepared['ready'] and prepared['cached'], prepared
    assert 'recovered' in execute(c, 'python3', ['-c', 'import sqlite3; print("recovered")'], cwd='/home/agent/qa/recovered')
    c.call('shell', {'action': 'show'})
    c.screenshot('10-recovered-desktop.png')
    output.joinpath('results.json').write_text(json.dumps({'passed': True, 'startup_seconds': startup, 'job_state': recovered['state'], 'output_retained': True, 'environment_cached': True}, indent=2))


if __name__ == '__main__':
    main()
