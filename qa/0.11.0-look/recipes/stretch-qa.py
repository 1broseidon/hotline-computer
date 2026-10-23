#!/usr/bin/env python3
"""Exercise the activity display against the Apple-container QA instance."""
import importlib.util
import json
from pathlib import Path
import time

spec = importlib.util.spec_from_file_location('look', Path(__file__).with_name('look-qa.py'))
look = importlib.util.module_from_spec(spec)
spec.loader.exec_module(look)
c = look.Client()
results = {}


def menu(key):
    c.call('input', {'action': 'click', 'x': 20, 'y': 20, 'capture_after': 'none'})
    c.call('input', {'action': 'key', 'combo': key, 'capture_after': 'none'})


def classes():
    return {next((name for name in ('chromium', 'hotlineterminal', 'hotlineactivity', 'hotlineshell')
                  if name in w['class'].lower()), w['class']): w
            for w in c.call('windows', {'action': 'list'})}


def expect_bounds(expected):
    deadline = time.monotonic() + 5
    while True:
        windows = classes()
        if all(name in windows and windows[name]['bounds'] == bounds for name, bounds in expected.items()):
            return windows
        assert time.monotonic() < deadline, windows
        time.sleep(.1)


try:
    # Populate enough calls to cross retention; payload canaries never belong
    # to the activity file, even when the underlying tool rejects the action.
    for _ in range(505):
        c.call('state', {'action': 'info'})
    c.call('files', {'action': 'put', 'path': '/home/agent/.hotline/look-qa/payload', 'content': 'ACTIVITY_FILE_PAYLOAD'})
    c.call('shell', {'action': 'exec', 'command': 'printf ACTIVITY_COMMAND_PAYLOAD', 'label': 'Activity log check'})
    c.call('browser', {'action': 'eval', 'js': "'ACTIVITY_JS_PAYLOAD'"})
    c.rpc('tools/call', {'name': 'browser', 'arguments': {'action': 'ACTIVITY_UNKNOWN_ACTION'}})
    entries = [json.loads(line) for line in c.call('files', {'action': 'get', 'path': '/home/agent/.hotline/activity.jsonl'}).splitlines()]
    assert len(entries) == 500, len(entries)
    encoded = json.dumps(entries)
    for payload in ('ACTIVITY_FILE_PAYLOAD', 'ACTIVITY_COMMAND_PAYLOAD', 'ACTIVITY_JS_PAYLOAD', 'ACTIVITY_UNKNOWN_ACTION'):
        assert payload not in encoded, payload
    assert any(e['target'] == 'Activity log check' for e in entries)
    assert any(e['tool'] == 'browser' and e['action'] == 'call' and not e['ok'] for e in entries)
    results['retention and payload exclusion'] = 'passed: 500 entries; rejected call retained without its payload'
    c.call('shell', {'action': 'show'})
    menu('v')
    expect_bounds({'hotlineterminal': [1280, 56, 624, 595], 'hotlineactivity': [1280, 667, 624, 397]})
    c.capture('s7-activity.png')
    menu('t')
    expected = {'hotlineterminal': [1280, 56, 624, 393], 'hotlineactivity': [1280, 465, 624, 263], 'hotlineshell': [1280, 744, 624, 320]}
    expect_bounds(expected)
    c.call('windows', {'action': 'tile'})
    windows = expect_bounds(expected)
    c.capture('s7-with-shell.png')
    results['60/40 and shell placement'] = expected
    c.call('windows', {'action': 'close', 'window_id': windows['hotlineshell']['id']})
    c.call('windows', {'action': 'close', 'window_id': windows['hotlineactivity']['id']})
    expect_bounds({'hotlineterminal': [1280, 56, 624, 1008]})
    results['close restores observer'] = 'passed'
finally:
    (look.OUT / 'phase7-scenes.json').write_text(json.dumps(results, indent=2) + '\n')
    print(json.dumps(results, indent=2))
