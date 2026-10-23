#!/usr/bin/env python3
"""Compare protected launch surfaces with the spec's starting revision."""
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[3]
BASE = '7d75305d59b1d09f334c4d7d1149a9312beb7022'
RESULTS = {}


def before(name):
    return subprocess.check_output(['git', 'show', BASE + ':' + name], cwd=ROOT).decode()


def current(name):
    return (ROOT / name).read_text()


for name in ['Cargo.lock', 'src/browser.rs', 'src/passkeys.rs', 'src/logins.rs', 'src/lease.rs']:
    assert before(name) == current(name), name
    RESULTS[name] = 'unchanged'
cargo = current('Cargo.toml').replace('["damage", "shm", "xtest", "shape"]', '["damage", "shm", "xtest"]').replace('# Geist rather than X core bitmap fonts.', '# DejaVu Sans rather than X core bitmap fonts.')
assert cargo == before('Cargo.toml')
RESULTS['Cargo.toml'] = 'only the specified x11rb shape feature and stale font comment changed'
old = json.loads(before('assets/chromium-policy.json'))
new = json.loads(current('assets/chromium-policy.json'))
assert new.pop('BrowserThemeColor') == '#17171b'
old.pop('_comment'); new.pop('_comment')
assert old == new
RESULTS['Chromium policy'] = 'only theme and comment changed'
for heading, end in [('## Secrets', '## Build and run'), ('## The tools', '## The desktop')]:
    def section(text): return text.split(heading, 1)[1].split(end, 1)[0]
    assert section(before('README.md')) == section(current('README.md'))
RESULTS['README security and tool wording'] = 'unchanged'
def descriptors(text): return text.split('pub fn descriptors', 1)[1].split('fn schema', 1)[0]
assert descriptors(before('src/tools/mod.rs')) == descriptors(current('src/tools/mod.rs'))
RESULTS['MCP descriptors and required arguments'] = 'unchanged (8 tools)'
def authentication(text):
    body = text.split('async fn authenticate', 1)[1].split('\n}\n', 1)[0]
    return re.sub(r'\s+', '', body)
old_auth = authentication(before('src/serve.rs'))
new_auth = authentication(current('src/serve.rs')).replace('||request.uri().path().starts_with("/assets/fonts/")', '')
assert old_auth == new_auth
RESULTS['Bearer gate'] = 'only the specified public font prefix added'
assert before('src/secrets.rs').split('pub async fn replace', 1)[0] == current('src/secrets.rs').split('pub async fn replace', 1)[0]
RESULTS['Secret storage and redaction'] = 'unchanged; HTTP replacement only publishes a count'
(ROOT / 'qa/0.11.0-look/scope-checks.json').write_text(json.dumps(RESULTS, indent=2) + '\n')
print(json.dumps(RESULTS, indent=2))
