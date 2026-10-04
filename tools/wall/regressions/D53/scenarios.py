"""Unprivileged live scenarios; fixture acceptance only, never a model review."""
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

def check(expected, actual, okay):
    print('EXPECTED:', expected, flush=True)
    print('ACTUAL:', actual, flush=True)
    if not okay:
        raise SystemExit(1)

def cancel_member():
    setup()
    worker = lane()
    seal(worker)
    review, reviewer = start_review()
    result = subprocess.run(['ha', 'thread', 'cancel', 'wall', worker['id'], '--reason', 'Fixture cancellation'],
                            text=True, capture_output=True)
    check('member cancellation during review is refused with the review-cancel path',
          result.stdout + result.stderr, result.returncode != 0 and 'cancel' in result.stdout + result.stderr)
    note = H / 'note.md'
    note.write_text('Change the fixture after this review.\n')
    result = subprocess.run(['ha', 'thread', 'prompt', 'wall', worker['id'], '--text-file', str(note)],
                            text=True, capture_output=True)
    check('member follow-up during review is refused explicitly', result.stdout + result.stderr,
          result.returncode != 0 and 'cancel' in result.stdout + result.stderr)
    # Pause automatic allocation across the two ordinary cancellation commands.
    command('ha', 'pause', 'wall')
    command('ha', 'review', 'cancel', 'wall')
    command('ha', 'thread', 'cancel', 'wall', worker['id'], '--reason', 'Fixture cancellation')
    command('ha', 'resume', 'wall')
    check('cancelled review and member leave the remote unchanged', remote_head(), remote_head() == review['base'])
    dump()
    return worker, review

def cancel_projection():
    worker, _ = cancel_member()
    command('ha', 'review', 'wall')
    task = next(t for t in read_records('tasks') if worker['id'] in t['attempts'])
    result = subprocess.check_output(['ha', 'task', 'show', 'wall', task['id']], text=True)
    current = g.target(worker['id'])
    active = [r['id'] for r in reviews() if r['phase'] not in ['complete', 'cancelled', 'rejected']]
    print('INJECTION:', json.dumps({'status': current['status'], 'cancellation_reason': current['cancellation_reason'],
                                   'active_reviews': active}), flush=True)
    check('a cancelled sealed lane names a new attempt or explicit task retirement',
          result, current['status'] == 'resolved' and not active
          and 'start a new attempt or drop the task' in result
          and 'review the repository pile' not in result)
    retained = [e for e in read_records('events') if e.get('thread') == worker['id']
                and 'done' in e.get('payload', {})]
    check('cancellation retains the seal and report artifact', retained,
          bool(retained) and bool(retained[-1]['payload']['done']['artifact']))
    command('ha', 'task', 'drop', 'wall', task['id'], '--reason', 'Fixture explicit retirement')
    dropped = next(t for t in read_records('tasks') if t['id'] == task['id'])
    check('the offered retirement action succeeds and retains history', dropped,
          bool(dropped.get('dropped')) and retained[-1] in read_records('events'))

if __name__ == '__main__':
    globals()[sys.argv[1].replace('-', '_')]()
