#!/usr/bin/env python3
"""Measure a registry pull and startup; refuse to call a cached daemon cold."""
import argparse
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

from acceptance import Computer


def docker(*args):
    return subprocess.check_output(['docker', *args], text=True).strip()


def compressed_size(manifests, image_id):
    entries = manifests if isinstance(manifests, list) else [manifests]
    for entry in entries:
        manifest = entry.get('OCIManifest', entry.get('SchemaV2Manifest', {}))
        if manifest.get('config', {}).get('digest') == image_id:
            return sum(layer['size'] for layer in manifest['layers'])
    raise RuntimeError('registry manifest does not contain the pulled image configuration')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('image')
    parser.add_argument('output', type=Path)
    parser.add_argument('--revision', required=True)
    parser.add_argument('--channel', choices=['development', 'release'], required=True)
    parser.add_argument('--allow-cached', action='store_true', help='Record a warm smoke test instead of requiring an empty daemon.')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    cold = not docker('image', 'ls', '--quiet')
    if not cold and not args.allow_cached:
        raise RuntimeError('cold pull requires an empty Docker image store; use a fresh runner')
    report = {'image': args.image, 'cold': cold, 'passed': False}
    container = None
    credential = None
    try:
        started = time.monotonic()
        with (args.output / 'pull.log').open('w') as log:
            subprocess.run(['docker', 'pull', args.image], stdout=log, stderr=subprocess.STDOUT, check=True)
        report['pull_seconds'] = time.monotonic() - started
        image = json.loads(docker('image', 'inspect', args.image))[0]
        report.update(image_id=image['Id'], repo_digests=image['RepoDigests'], architecture=image['Architecture'], uncompressed_bytes=image['Size'])
        manifests = json.loads(docker('manifest', 'inspect', '--verbose', args.image))
        report['compressed_layer_bytes'] = compressed_size(manifests, image['Id'])
        (args.output / 'registry-manifests.json').write_text(json.dumps(manifests, indent=2))
        token = secrets.token_hex(24)
        with tempfile.NamedTemporaryFile(mode='w', delete=False) as file:
            credential = file.name
            os.chmod(credential, 0o600)
            file.write('HOTLINE_COMPUTER_TOKEN=' + token + '\n')
        started = time.monotonic()
        container = docker('run', '-d', '--cap-drop=ALL', '--security-opt', 'no-new-privileges', '--pids-limit', '1024', '--memory', '4g', '--shm-size', '1g', '-p', '127.0.0.1::8787', '--env-file', credential, args.image)
        report['container_start_seconds'] = time.monotonic() - started
        port = docker('port', container, '8787/tcp').rsplit(':', 1)[1]
        url = 'http://127.0.0.1:' + port
        deadline = time.monotonic() + 30
        while True:
            try:
                with urllib.request.urlopen(url + '/health', timeout=2) as response:
                    assert response.status == 200
                break
            except (urllib.error.URLError, TimeoutError, ConnectionError):
                if time.monotonic() >= deadline:
                    raise RuntimeError('pulled desktop was not healthy within 30 seconds')
                time.sleep(.1)
        report['ready_seconds'] = time.monotonic() - started
        computer = Computer(url, token, args.output)
        info = computer.call('state', {'action': 'info'})
        assert info['build'] == {'channel': args.channel, 'revision': args.revision}, info['build']
        guide = computer.call('state', {'action': 'guide'})
        assert guide['build'] == info['build'] and guide['sha256'] == info['skill_sha256'], guide
        (args.output / 'info.json').write_text(json.dumps(info, indent=2))
        (args.output / 'guide.json').write_text(json.dumps(guide, indent=2))
        computer.screenshot('cold-start.png')
        report['passed'] = True
    finally:
        (args.output / 'results.json').write_text(json.dumps(report, indent=2))
        if credential:
            os.unlink(credential)
        if container:
            with (args.output / 'container.log').open('w') as log:
                subprocess.run(['docker', 'logs', container], stdout=log, stderr=subprocess.STDOUT)
            subprocess.run(['docker', 'rm', '-f', container], check=True, stdout=subprocess.DEVNULL)
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
