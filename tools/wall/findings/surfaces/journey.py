#!/usr/bin/env python3
"""Single-lane Linux journey; reviewer checks only the exact wall fixture."""
from pathlib import Path
import subprocess
import sys
import tomllib
import json

sys.path.insert(0, str(Path.home() / 'tools'))
import guest

home = Path.home()
project = guest.PROJECT
if guest.records():
    raise RuntimeError('journey requires clean reset (ha new is part of reset)')
config = home / '.config/herdr-ade/config.toml'
config.write_text(config.read_text().replace('[adapters.pi]',
    '[[routing.rules]]\nworkflow = "reviewer"\nrecipe = "wall_lane"\n\n[adapters.pi]'))
(home / 'control.json').write_text(json.dumps({'hold': ['before-seal'], 'gate_review': True}))
guest.open_project(strict=True)
thread = guest.lane()['id']
guest.wait(thread, 'before-seal')
guest.run('ha', 'plan', 'set', 'wall', '--does', 'Land the scratch fixture')
guest.run('ha', 'plan', 'step', 'add', 'wall', 'Land fixture', '--task', 'job-0001')
(home / 'release/before-seal').touch()
guest.poll('done event', lambda: list((project / '.state/events').glob('*.toml')))
print('JOURNEY sealed: expect zero done steps, finished is not merged', flush=True)
guest.run('python3', home / 'surfaces.py')
guest.run('ha', 'review', 'wall')
def landed():
    reviews = [tomllib.loads(p.read_text()) for p in (project / '.state/reviews').glob('*.toml')]
    for review in reviews:
        if review.get('attention'):
            raise RuntimeError(review['attention'])
    return any(r['phase'] == 'complete' and r['fast_forward'] and r['push'] for r in reviews)
guest.poll('review and landing', landed, seconds=240)
head = subprocess.check_output(['git', '-C', home / 'repo', 'rev-parse', 'main'], text=True).strip()
pushed = subprocess.check_output(['git', '--git-dir', home / 'remote.git', 'rev-parse', 'main'], text=True).strip()
assert head == pushed, (head, pushed)
actual = subprocess.check_output(['git', '--git-dir', home / 'remote.git', 'show',
                                  f'main:scripted-{thread}.txt'], text=True)
assert actual == f'Scripted wall lane {thread}\n', actual
print('JOURNEY landed exact reviewed fixture on bare main:', pushed, flush=True)
guest.run('python3', home / 'surfaces.py')
guest.run('ha', 'archive', 'wall')
guest.run('ha', 'list', '--all')
guest.run('ha', 'overview', 'wall')
assert subprocess.run(['ha', 'open', 'wall'], check=False).returncode != 0
guest.run('ha', 'plan', 'show', 'wall')
# Use the existing allowlisted pre-delete evidence member, never export credentials.
temporary = home / '.surface-capture'
with temporary.open('wb') as output:
    guest.run('python3', home / 'tools/guest.py', 'evidence', stdout=output)
temporary.rename(home / 'gate-loop-before-delete.tar')
guest.run('ha', 'delete', 'wall')
assert not project.exists(), 'delete left project present'
print('JOURNEY PASS: new/open/first lane/review/bare landing/archive/delete unassisted', flush=True)
