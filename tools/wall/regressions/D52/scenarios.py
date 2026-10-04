"""Unprivileged live scenarios; fixture acceptance only, never a model review."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import tomllib

sys.path.insert(0, str(Path.home() / 'tools'))
import guest as g

H = g.HOME
P = g.PROJECT


def command(*args, **kwargs):
    print('COMMAND', *map(str, args), flush=True)
    return subprocess.run(list(map(str, args)), check=True, **kwargs)

def read_records(kind):
    return [tomllib.loads(p.read_text()) for p in sorted((P / '.state' / kind).glob('*.toml'))]

def reviews():
    return read_records('reviews')

def control(**values):
    (H / 'control.json').write_text(json.dumps(values))

def setup():
    command('ha', '--version')
    command('sha256sum', H / 'bin/herdr-ade')
    config = H / '.config/herdr-ade/config.toml'
    config.write_text(config.read_text().replace('[adapters.pi]',
        '[[routing.rules]]\nworkflow = "reviewer"\nrecipe = "wall_lane"\n\n[adapters.pi]'))
    control(hold=['before-seal'])
    g.open_project(strict=True)

def lane():
    row = g.lane()
    g.wait(row['id'], 'before-seal')
    return row

def seal(row):
    command('ha', 'done', cwd=row['worktree_path'],
            env=dict(os.environ, HERDR_PANE_ID=row['pane_id']))
    return g.poll('seal', lambda: next((e for e in reversed(read_records('events'))
                  if e.get('thread') == row['id'] and 'done' in e.get('payload', {})), None))

def start_review(after=None):
    # Keep the script at mid-review; the test driver writes an exact fixture verdict.
    control(hold=['mid-review', 'before-seal'])
    command('ha', 'review', 'wall')
    review = g.poll('review allocation', lambda: next((r for r in reviews()
                    if r.get('reviewer') and r['id'] != after
                    and r['phase'] not in ['complete', 'rejected', 'cancelled']), None))
    g.poll('reviewer brief submission', lambda: g.target(review['reviewer']).get('brief_submitted'))
    g.wait(review['reviewer'], 'mid-review')
    return review, g.target(review['reviewer'])

def fixture_merge(reviewer):
    command('python3', H / 'tools/guest.py', 'gate-review', reviewer['id'], cwd=reviewer['worktree_path'])
    return seal(reviewer)

def wait_review(id, phase):
    return g.poll(phase, lambda: next((r for r in reviews() if r['id'] == id and r['phase'] == phase), None), seconds=100)

def remote_head():
    return subprocess.check_output(['git', '--git-dir', str(H / 'remote.git'), 'rev-parse', 'main'], text=True).strip()

def dump():
    print('REVIEWS', json.dumps(reviews(), indent=2), flush=True)
    command('ha', 'overview', 'wall')

def drop_review():
    print('EXPECTED: dropping a sealed task during review must prevent its later publication', flush=True)
    setup()
    worker = lane()
    seal(worker)
    review, reviewer = start_review()
    task = next(t for t in read_records('tasks') if worker['id'] in t['attempts'])
    retained_seal = next(e for e in reversed(read_records('events'))
                         if e.get('thread') == worker['id'] and 'done' in e.get('payload', {}))
    result = subprocess.run(['ha', 'task', 'drop', 'wall', task['id'], '--reason', 'Fixture no longer wanted'],
                            text=True, capture_output=True)
    print(result.stdout + result.stderr, flush=True)
    dropped = next(t for t in read_records('tasks') if t['id'] == task['id'])
    assert result.returncode == 0 and dropped['dropped'], 'task drop did not succeed'
    print('OBSERVATION', json.dumps({'dropped': dropped['dropped'], 'lane_status': g.target(worker['id'])['status']}), flush=True)
    current_review = next(r for r in reviews() if r['id'] == review['id'])
    if current_review['phase'] != 'cancelled':
        # Original failure scenario: the stale reviewer can still publish its seal.
        fixture_merge(reviewer)
        g.poll('review outcome', lambda: any(r['phase'] in ['complete', 'cancelled', 'rejected'] or r.get('attention') for r in reviews()))
    dump()
    result = subprocess.run(['git', '--git-dir', str(H / 'remote.git'), 'cat-file', '-e',
                             f'main:scripted-{worker["id"]}.txt'])
    print('EXPECTED: a dropped task is not subsequently published by an in-flight review', flush=True)
    print('ACTUAL: dropped task file published =', result.returncode == 0, flush=True)
    if result.returncode == 0:
        raise SystemExit(1)
    check('the frozen review is cancelled before the task drop returns',
          current_review['phase'], current_review['phase'] == 'cancelled')
    report_hash = retained_seal['payload']['done']['artifact']
    report = (P / '.state/artifacts' / report_hash).read_bytes()
    check('the seal and exact report remain durable history', retained_seal['id'],
          retained_seal in read_records('events')
          and hashlib.sha256(report).hexdigest() == report_hash)
    checkout = Path(worker['worktree_path'])
    check('unique dropped work stays in its checkout and branch', str(checkout),
          checkout.exists() and subprocess.check_output(
              ['git', 'rev-parse', worker['branch']], cwd=checkout, text=True).strip()
          == retained_seal['payload']['done']['sha'])
    remaining = lane()
    seal(remaining)
    rebuilt, next_reviewer = start_review(after=review['id'])
    check('the rebuilt pile contains only remaining work', rebuilt['members'],
          [m['thread'] for m in rebuilt['members']] == [remaining['id']])
    fixture_merge(next_reviewer)
    wait_review(rebuilt['id'], 'complete')
    result = subprocess.run(['git', '--git-dir', str(H / 'remote.git'), 'cat-file', '-e',
                             f'main:scripted-{worker["id"]}.txt'])
    check('later publication still excludes the dropped seal', result.returncode,
          result.returncode != 0)

def check(expected, actual, okay):
    print('EXPECTED:', expected, flush=True)
    print('ACTUAL:', actual, flush=True)
    if not okay:
        raise SystemExit(1)

if __name__ == '__main__':
    globals()[sys.argv[1].replace('-', '_')]()
