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


def drop_review():
    print('EXPECTED: dropping a sealed task during review must prevent its later publication', flush=True)
    setup()
    worker = lane()
    seal(worker)
    review, reviewer = start_review()
    task = next(t for t in read_records('tasks') if worker['id'] in t['attempts'])
    result = subprocess.run(['ha', 'task', 'drop', 'wall', task['id'], '--reason', 'Fixture no longer wanted'],
                            text=True, capture_output=True)
    print(result.stdout + result.stderr, flush=True)
    dropped = next(t for t in read_records('tasks') if t['id'] == task['id'])
    if result.returncode != 0 and not dropped.get('dropped'):
        print('ACTUAL: task drop refused before recording success; no false retirement', flush=True)
        return
    assert dropped['dropped'], 'task drop was not recorded'
    print('INJECTION', json.dumps({'dropped': dropped['dropped'], 'lane_status': g.target(worker['id'])['status']}), flush=True)
    fixture_merge(reviewer)
    g.poll('review outcome', lambda: any(r['phase'] in ['complete', 'cancelled', 'rejected'] or r.get('attention') for r in reviews()))
    dump()
    result = subprocess.run(['git', '--git-dir', str(H / 'remote.git'), 'cat-file', '-e',
                             f'main:scripted-{worker["id"]}.txt'])
    print('EXPECTED: a dropped task is not subsequently published by an in-flight review', flush=True)
    print('ACTUAL: dropped task file published =', result.returncode == 0, flush=True)
    if result.returncode == 0:
        raise SystemExit(1)


def check(expected, actual, okay):
    print('EXPECTED:', expected, flush=True)
    print('ACTUAL:', actual, flush=True)
    if not okay:
        raise SystemExit(1)


def batch():
    setup()
    workers = [lane(), lane()]
    # Real concurrent sealing helpers; both lanes already committed their fixture.
    processes = [subprocess.Popen(['ha', 'done'], cwd=w['worktree_path'],
                 env=dict(os.environ, HERDR_PANE_ID=w['pane_id'])) for w in workers]
    assert all(p.wait() == 0 for p in processes)
    first = seal(workers[0])
    again = seal(workers[0])
    check('an unchanged reseal retains the seal identity', [first['id'], again['id']], first['id'] == again['id'])
    review, reviewer = start_review()
    third = lane()
    seal(third)
    frozen = next(r for r in reviews() if r['id'] == review['id'])
    check('a seal during review does not enter its frozen membership',
          [m['thread'] for m in frozen['members']],
          {m['thread'] for m in frozen['members']} == {w['id'] for w in workers})
    command('ha', 'overview', 'wall')
    fixture_merge(reviewer)
    wait_review(review['id'], 'complete')
    result = subprocess.run(['git', '--git-dir', str(H / 'remote.git'), 'cat-file', '-e',
                             f'main:scripted-{third["id"]}.txt'])
    check('the later file is not published by the earlier review', result.returncode, result.returncode != 0)
    later, reviewer = start_review(after=review['id'])
    check('the next review retains the later seal', [m['thread'] for m in later['members']],
          [m['thread'] for m in later['members']] == [third['id']])
    fixture_merge(reviewer)
    wait_review(later['id'], 'complete')
    dump()


def push_rejection():
    setup()
    worker = lane()
    seal(worker)
    review, reviewer = start_review()
    base = remote_head()
    git = ['git', '--git-dir', str(H / 'remote.git')]
    tree = subprocess.check_output(git + ['rev-parse', base + '^{tree}'], text=True).strip()
    divergent = subprocess.check_output(git + ['-c', 'user.name=Wall', '-c', 'user.email=wall@example.invalid',
                    'commit-tree', tree, '-p', base, '-m', 'Concurrent remote change'], text=True).strip()
    command(*git, 'update-ref', 'refs/heads/main', divergent, base)
    fixture_merge(reviewer)
    blocked = g.poll('push refusal', lambda: next((r for r in reviews() if r.get('attention')), None))
    dump()
    check('rejected publication keeps the seal and pending publication with an explanation',
          {k: blocked[k] for k in ['phase', 'fast_forward', 'push', 'install', 'attention']},
          blocked['phase'] == 'landing' and blocked['fast_forward'] and not blocked['push']
          and not blocked['install'] and remote_head() == divergent)
    command('ha', 'ticker', 'stop')
    print('INJECTION: ticker stopped after local merge, before publication/install', flush=True)
    command(*git, 'update-ref', 'refs/heads/main', base, divergent)
    command('ha', 'ticker', 'start')
    complete = wait_review(review['id'], 'complete')
    check('ticker restart resumes the exact reviewed candidate', remote_head(),
          remote_head() == complete['verdict']['candidate'])
    dump()


def reject(evidence_only=False):
    setup()
    worker = lane()
    seal(worker)
    review, reviewer = start_review()
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=reviewer['worktree_path'], text=True).strip()
    Path(reviewer['thread_dir'], 'report.md').write_text(
        f'+++\nreview = "{review["id"]}"\nverdict = "REJECT"\ncandidate = "{head}"\n'
        + ('evidence_only = true\n' if evidence_only else '')
        + '+++\nFixture review input incomplete.\n')
    seal(reviewer)
    rejected = wait_review(review['id'], 'rejected')
    check('REJECT never publishes a member', remote_head(), remote_head() == review['base'])
    dump()
    if evidence_only:
        command('ha', 'review', 'retry', 'wall')
        later, new_reviewer = start_review(after=review['id'])
        check('evidence-only REJECT preserves the exact member seal for retry',
              later['members'], later['members'] == rejected['members'])
        fixture_merge(new_reviewer)
        wait_review(later['id'], 'complete')
    else:
        # The member needs a follow-up, rather than silently reviewing the same failed seal.
        command('ha', 'review', 'wall')
        check('merits REJECT defers the old seal', [(r['id'], r['phase']) for r in reviews()],
              all(r['phase'] == 'rejected' for r in reviews()))
    dump()


def evidence_reject():
    reject(evidence_only=True)


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
    check('a cancelled sealed lane does not tell its task to review an unavailable pile',
          result, not (current['status'] == 'resolved' and not active and 'review the repository pile' in result))


def changed_reseal():
    setup()
    worker = lane()
    original = seal(worker)
    review, reviewer = start_review()
    checkout = Path(worker['worktree_path'])
    (checkout / 'correction.txt').write_text('A later fixture correction.\n')
    command('git', 'add', 'correction.txt', cwd=checkout)
    command('git', 'commit', '-m', 'Later fixture correction', cwd=checkout)
    newer = seal(worker)
    check('changed work creates a distinct durable seal', [original['id'], newer['id']], original['id'] != newer['id'])
    fixture_merge(reviewer)
    blocked = g.poll('changed member hold', lambda: next((r for r in reviews() if r.get('attention')), None))
    check('the frozen candidate does not land after a member changes', blocked['attention'],
          'changed during review' in blocked['attention'] and remote_head() == review['base'])
    dump()


def withdrawals():
    setup()
    task_file = H / 'task.md'
    task_file.write_text('Commit the scripted fixture and seal.\n')
    command('ha', 'thread', 'start', 'wall', '--task-file', task_file, '--title', 'Withdrawal fixture',
            '--repo', H / 'repo', '--recipe', 'wall_lane', '--request', (H / 'request').read_text(),
            '--acceptance', 'Scripted fault plumbing observed', '--acceptance', 'Obsolete fixture condition')
    worker = g.records()[-1]
    g.poll('worker brief', lambda: g.target(worker['id']).get('brief_submitted'))
    worker = g.target(worker['id'])
    g.wait(worker['id'], 'before-seal')
    event = seal(worker)
    review, reviewer = start_review()
    task = next(t for t in read_records('tasks') if worker['id'] in t['attempts'])
    command('ha', 'task', 'drop', 'wall', task['id'], '--acceptance', '2', '--reason', 'Fixture condition replaced')
    checkout = reviewer['worktree_path']
    command('git', 'merge', '--no-edit', review['members'][0]['sha'], cwd=checkout)
    filename = f'scripted-{worker["id"]}.txt'
    actual = subprocess.check_output(['git', 'show', f'HEAD:{filename}'], cwd=checkout, text=True)
    assert actual == f'Scripted wall lane {worker["id"]}\n'
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=checkout, text=True).strip()
    report = f'+++\nreview = "{review["id"]}"\nverdict = "MERGE"\ncandidate = "{head}"\n'
    for criterion, condition, established in [(1, task['acceptance'][0], 'true'), (2, task['acceptance'][1], 'false')]:
        report += (f'[[acceptance]]\nthread = "{worker["id"]}"\nevent = "{event["id"]}"\n'
                   f'criterion = {criterion}\ncondition = {json.dumps(condition)}\nestablished = {established}\n'
                   f'evidence = "Exact scripted fixture file checked; obsolete row intentionally not established"\n')
    Path(reviewer['thread_dir'], 'report.md').write_text(report + '+++\nFixture-only judgment.\n')
    seal(reviewer)
    complete = wait_review(review['id'], 'complete')
    check('a withdrawn condition does not block the still-required fixture criterion', complete['phase'],
          remote_head() == head)
    dump()


def hold_bound():
    # Real elapsed time: no edited records, host clock changes or simulated clock.
    setup()
    ready, older = lane(), lane()
    command('ha', 'review', 'wall')
    control(hold=['mid-review', 'before-seal'])
    event = seal(ready)
    began = time.monotonic()
    holds_file = P / '.state/pile-holds.json'
    held = g.poll('older working lane hold', lambda: json.loads(holds_file.read_text()).get('current')
                  if holds_file.exists() else None)
    check('the ready pile explains the older working lane hold', held,
          any(older['id'] in reason for reason in held.values()))
    command('ha', 'overview', 'wall')
    print('OBSERVATION: waiting for the actual twenty-minute bound', event['created'], flush=True)
    time.sleep(max(0, 1140 - (time.monotonic() - began)))
    check('the older lane still holds before twenty minutes', reviews(), not reviews())
    later = lane()
    command('ha', 'thread', 'cancel', 'wall', later['id'], '--reason', 'Later non-member stopped around hold bound')
    review = g.poll('twenty-minute automatic release', lambda: next((r for r in reviews() if r.get('reviewer')), None), seconds=180)
    elapsed = time.monotonic() - began
    g.poll('reviewer brief', lambda: g.target(review['reviewer']).get('brief_submitted'))
    g.wait(review['reviewer'], 'mid-review')
    check('the ready seal enters review without the still-running older lane',
          {'elapsed_seconds': elapsed, 'members': review['members'], 'holds': json.loads(holds_file.read_text())},
          [m['event'] for m in review['members']] == [event['id']])
    command('ha', 'thread', 'cancel', 'wall', older['id'], '--reason', 'Bound observation complete')
    fixture_merge(g.target(review['reviewer']))
    wait_review(review['id'], 'complete')
    dump()


if __name__ == '__main__':
    globals()[sys.argv[1].replace('-', '_')]()
