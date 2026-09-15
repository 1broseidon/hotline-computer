#!/usr/bin/env python3
"""Crash the container supplied by the acceptance runner and verify recovery,
then replace it with a fresh container on the same volumes, as an upgrade or
a hibernate cycle does, and verify that what lives in the home came along."""
import argparse
import json
import os
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

    # A new container on the same volumes: the home keeps the jobs, the
    # prepared workspace and its cache; the store keeps the packages.
    described = json.loads(subprocess.check_output(['docker', 'inspect', args.container], text=True))[0]
    mounts = []
    for mount in described['Mounts']:
        if mount['Type'] == 'volume':
            mounts += ['--mount', 'type=volume,source=%s,target=%s' % (mount['Name'], mount['Destination'])]
        else:
            mounts += ['-v', '%s:%s' % (mount['Source'], mount['Destination'])]
    assert any(m.endswith('target=/home/agent') for m in mounts), mounts
    subprocess.run(['docker', 'rm', '-f', args.container], check=True, stdout=subprocess.DEVNULL)
    replacement = subprocess.check_output(['docker', 'run', '-d', '--cap-drop=ALL', '--security-opt', 'no-new-privileges', '--pids-limit', '1024', '--memory', '4g', '--shm-size', '1g', '-p', '127.0.0.1:0:8787', '-e', 'TOAD_COMPUTER_TOKEN=' + token, *mounts, described['Config']['Image']], text=True).strip()
    args.output.joinpath('recreated-container-id.txt').write_text(replacement)
    try:
        port = subprocess.check_output(['docker', 'port', replacement, '8787/tcp'], text=True).strip().rsplit(':', 1)[1]
        url = 'http://127.0.0.1:' + port
        for _ in range(300):
            try:
                with urllib.request.urlopen(url+'/health', timeout=2):
                    break
            except OSError:
                time.sleep(.1)
        else:
            raise AssertionError('recreated computer did not become healthy')
        c = Computer(url, token, output)
        kept = c.call('shell', {'action': 'status', 'job_id': job['id']})
        assert kept['state'] == 'interrupted', kept
        assert 'retained before restart' in c.call('shell', {'action': 'read', 'job_id': job['id']})['output']
        again = c.call('state', {'action': 'prepare', 'workspace': '/home/agent/qa/recovered'})
        assert again['ready'] and again['cached'], again
        assert 'recovered' in execute(c, 'python3', ['-c', 'import sqlite3; print("recovered")'], cwd='/home/agent/qa/recovered')
        c.call('shell', {'action': 'show'})
        c.screenshot('11-recreated-desktop.png')
    finally:
        with args.output.joinpath('recreated-container.log').open('w') as log:
            subprocess.run(['docker', 'logs', replacement], stdout=log, stderr=subprocess.STDOUT)
        if os.environ.get('TOAD_ACCEPTANCE_KEEP', '0') != '1':
            subprocess.run(['docker', 'rm', '-f', replacement], check=True, stdout=subprocess.DEVNULL)
    output.joinpath('results.json').write_text(json.dumps({'passed': True, 'startup_seconds': startup, 'job_state': recovered['state'], 'output_retained': True, 'environment_cached': True, 'recreated_on_same_volumes': True}, indent=2))


if __name__ == '__main__':
    main()
