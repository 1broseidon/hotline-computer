#!/usr/bin/env python3
"""Small opt-in smoke check for generic preparation against a running Computer."""
import argparse
import json
import os
from pathlib import Path

from acceptance import Computer, execute


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    c = Computer(args.url, os.environ['HOTLINE_COMPUTER_TOKEN'], args.output)
    root = '/home/agent/src/generic-environment-smoke'

    def prepare(workspace, **source):
        result = c.call('state', {'action': 'prepare', 'workspace': workspace, **source})
        if not result['ready']:
            c.done(result['job'], 300)
        return result

    first = prepare(root+'/packages', packages=['hello', 'jq'])
    output = execute(c, 'bash', ['-c', 'hello; printf \'{"answer":42}\\n\' | jq -r .answer'], cwd=root+'/packages')
    assert 'Hello, world!' in output and '42' in output, output
    reused = prepare(root+'/reused', packages=['jq', 'hello'])
    assert reused['ready'] and reused['cached'], reused
    assert prepare(root+'/packages')['cached']

    # A bad replacement must leave the working environment usable.
    failed = c.call('state', {'action': 'prepare', 'workspace': root+'/packages', 'packages': ['hotlineSmokePackageDoesNotExist']})
    job = failed['job']
    for _ in range(30):
        job = c.call('shell', {'action': 'wait', 'job_id': job['id'], 'wait_ms': 1000})
        if job['state'] != 'running':
            break
    assert job['state'] == 'failed', job
    failure_output = c.call('shell', {'action': 'read', 'job_id': job['id'], 'max_output': 1048576})['output']
    assert 'hotlineSmokePackageDoesNotExist' in failure_output and 'error:' in failure_output, failure_output
    assert 'Hello, world!' in execute(c, 'hello', [], cwd=root+'/packages')

    pin = c.call('state', {'action': 'info'})['nixpkgs']
    flake = '''{
      inputs.nixpkgs.url = "github:NixOS/nixpkgs/@PIN@";
      outputs = { self, nixpkgs }: {
        devShells = nixpkgs.lib.genAttrs [ "aarch64-linux" "x86_64-linux" ] (system:
          let pkgs = import nixpkgs { inherit system; };
          in { dev = pkgs.mkShell {
            packages = [ pkgs.hello ];
            shellHook = "export PROJECT_PROOF=from-repository; echo Repository hook ready";
          }; });
      };
    }'''.replace('@PIN@', pin)
    repo = root+'/repository'
    c.call('files', {'action': 'put', 'path': repo+'/flake.nix', 'content': flake})
    prepared = prepare(repo, flake='.#dev')
    hook_output = c.call('shell', {'action': 'read', 'job_id': prepared['job']['id'], 'max_output': 1048576})['output']
    assert 'Repository hook ready' in hook_output, hook_output
    lock = c.call('files', {'action': 'get', 'path': repo+'/flake.lock'})
    c.call('files', {'action': 'put', 'path': repo+'/child/file.txt', 'content': ''})
    output = execute(c, 'bash', ['-c', 'hello; test "$PROJECT_PROOF" = from-repository; printf "%s\\n" "$PROJECT_PROOF"'], cwd=repo+'/child')
    assert 'Hello, world!' in output and 'from-repository' in output, output
    c.call('files', {'action': 'put', 'path': repo+'/flake.nix', 'content': flake.replace('from-repository', 'changed-hook')})
    prepare(repo)
    assert c.call('files', {'action': 'get', 'path': repo+'/flake.lock'}) == lock
    output = execute(c, 'bash', ['-c', 'test "$PROJECT_PROOF" = changed-hook; echo "Generic environments ready for review"'], cwd=repo)
    assert 'ready for review' in output, output
    c.call('shell', {'action': 'show'})
    c.screenshot('generic-environments.png')
    summary = {'passed': ['package preparation and execution', 'package cache reuse', 'saved definition reuse', 'failed replacement retains prior environment', 'named repository flake and shell hook', 'subdirectory inheritance', 'changed hook re-evaluation with unchanged lock'], 'version': c.call('state', {'action': 'info'})['version'], 'workspace': root, 'first_cached': first['cached']}
    (args.output/'summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    print(json.dumps(summary, indent=2))


if __name__ == '__main__':
    main()
