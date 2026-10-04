#!/usr/bin/env python3
"""Plain sandbox scenarios; no provider calls or lifecycle record edits."""
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import time
import tomllib

HOME = Path(os.environ['HOME'])
if not str(HOME).startswith('/home/wall-') or os.geteuid() == 0:
    raise RuntimeError('run as the numbered sandbox account')
sys.path.insert(0, str(HOME / 'tools'))
import guest  # installed, read-only wall helper

P = HOME / '.herdr-ade/wall'
S = P / '.state'


def run(*args, check=True):
    print('+ ' + shlex.join(map(str, args)), flush=True)
    result = subprocess.run(list(map(str, args)), text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT)
    print(result.stdout, end='', flush=True)
    if check:
        result.check_returncode()
    return result


def rows(kind):
    return [tomllib.loads(p.read_text()) for p in sorted((S / kind).glob('*.toml'))]


def wait(label, test, seconds=180):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = test()
        if value:
            print('OBSERVED', label, json.dumps(value), flush=True)
            return value
        time.sleep(1)
    raise RuntimeError('timeout: ' + label)


def goal():
    path = S / 'goal-check.json'
    return json.loads(path.read_text()) if path.exists() else {}


def open_project():
    guest.open_project(strict=True)
    wait('goal check owed', lambda: goal().get('generation', 0))


def goal_wait():
    open_project()
    run('ha', 'plan', 'set', 'wall', '--does', 'Deliver the scope Rolf chooses')
    # This is the literal wait shape advertised by the goal-check notice/help.
    run('ha', 'plan', 'check', 'wall', 'wait', 'Rolf', '--condition',
        'Choose one or two fixture files', '--evidence',
        'Scope choice needed before defining tasks')
    context = run('ha', 'context', 'wall').stdout
    run('ha', 'overview', 'wall')
    before = goal()
    time.sleep(8)
    after = goal()
    print('RECORD goal-check.json:', json.dumps(after, indent=2), flush=True)
    print('EXPECTED: an explicit goal-only wait remains visible as waiting for Rolf, '
          'with its condition, until answered or superseded.', flush=True)
    broken = ('Goal check wait retired' in context and
              'Wait for Rolf' not in context and
              before['generation'] == after['generation'] and
              after['disposition']['kind'] == 'wait')
    print('ACTUAL:', 'new wait immediately hidden as retired; disposition still suppresses '
          'another goal wake' if broken else context, flush=True)
    # Answer is a real public command; preserve its outcome for separate coverage.
    run('ha', 'plan', 'check', 'wall', 'answer', 'Rolf', '--evidence', 'Choose two files')
    print('ANSWER RECORD:', json.dumps(goal(), indent=2), flush=True)
    return 1 if broken else 0


def setup_tasks(observe_notice=False):
    open_project()
    if observe_notice:
        delivered = wait('empty-plan goal nudge delivered', lambda: goal() if goal().get('delivered_at') else None, 200)
        coordinator = json.loads((S / 'coordinator.json').read_text())
        assert delivered['binding'].startswith(coordinator['pane_id'] + ':')
        run('herdr', 'pane', 'read', coordinator['pane_id'], '--lines', '80', '--source', 'recent')
        time.sleep(20)
        assert goal()['delivered_at'] == delivered['delivered_at']
        print('NOTICE: correct coordinator binding; no repeated goal delivery during 20 seconds of unchanged evidence.', flush=True)
    config = HOME / '.config/herdr-ade/config.toml'
    config.write_text(config.read_text().replace('[adapters.pi]',
        '[[routing.rules]]\nworkflow = "reviewer"\nrecipe = "wall_lane"\n\n[adapters.pi]'))
    (HOME / 'control.json').write_text(json.dumps({'hold': [], 'gate_review': True}))
    run('ha', 'plan', 'set', 'wall', '--does', 'Two exact fixture files reviewed and published')
    for title in ['First fixture', 'Second fixture']:
        run('ha', 'task', 'add', 'wall', '--title', title, '--request',
            (HOME / 'request').read_text(), '--acceptance', 'Scripted fault plumbing observed',
            '--repo', HOME / 'repo')
    run('ha', 'plan', 'step', 'add', 'wall', 'First fixture', '--task', 'job-0001')
    run('ha', 'plan', 'step', 'add', 'wall', 'Second fixture', '--task', 'job-0002', '--after', 's-1')
    (HOME / 'task.md').write_text('Read the frozen brief, commit the exact scratch fixture, report and seal.\n')


def start(job):
    return run('ha', 'thread', 'start', 'wall', '--job', job, '--task-file', HOME / 'task.md',
               '--recipe', 'wall_lane', '--machine', 'local', check=False)


def journey():
    # Scripted coordinator has no editor UI: this driver follows ha context's
    # instructions, but cannot establish automatic prompt delivery to real pi.
    setup_tasks()
    run('ha', 'plan', 'show', 'wall')
    assert start('job-0002').returncode != 0
    run('ha', 'plan', 'sync', 'wall')
    premature = run('ha', 'plan', 'check', 'wall', 'close', '--task', 'job-0001',
                    '--evidence', 'Probe unfinished acceptance', check=False)
    assert premature.returncode != 0
    run('ha', 'plan', 'check', 'wall', 'wait', 'Rolf', '--task', 'job-0001',
        '--condition', 'Choose fixture scope', '--evidence', 'Scoped wait coverage')
    assert 'Wait for Rolf' in run('ha', 'context', 'wall').stdout
    run('ha', 'plan', 'check', 'wall', 'answer', 'Rolf', '--evidence', 'Proceed with both fixtures')
    assert 'Wait for Rolf' not in run('ha', 'context', 'wall').stdout
    start('job-0001').check_returncode()
    run('ha', 'plan', 'check', 'wall', 'action', 'job-0001', '--evidence', 'First authorized fixture started')
    wait('first done seal', lambda: [e['id'] for e in rows('events') if e.get('payload', {}).get('done')])
    # The ticker prints the enabling command, not this driver inventing it.
    hold = wait('printed pile next command', lambda: (S / 'pile-holds.json').exists() and
                'next: ha review' in (S / 'pile-holds.json').read_text() and
                json.loads((S / 'pile-holds.json').read_text())['current'])
    print('HOLDS:', hold, flush=True)
    run('ha', 'context', 'wall')
    run('ha', 'plan', 'sync', 'wall')
    # Capture every exact command substring in the real hold record.
    import re
    command = re.search(r'next: (ha review [^)]+)\)', '\n'.join(hold.values()))
    if not command:
        raise RuntimeError('no executable next command in pile hold')
    print('NEXT EXACT:', command.group(1), flush=True)
    run(*shlex.split(command.group(1)))
    wait('first landing', lambda: [r['id'] for r in rows('reviews') if r['phase'] == 'complete'], 240)
    run('ha', 'plan', 'sync', 'wall')
    run('ha', 'context', 'wall')
    # Goal notice authorizes the next request-backed action; prerequisite checked by real CLI.
    run('ha', 'plan', 'check', 'wall', 'action', 'job-0002', '--evidence', 'First fixture accepted; start dependent fixture')
    start('job-0002').check_returncode()
    wait('second automatic landing', lambda: len([r for r in rows('reviews') if r['phase'] == 'complete']) == 2, 240)
    run('ha', 'plan', 'sync', 'wall')
    run('ha', 'plan', 'show', 'wall')
    run('ha', 'plan', 'sync', 'wall')
    run('ha', 'task', 'list', 'wall')
    run('ha', 'plan', 'check', 'wall', 'close', '--task', 'job-0001', '--task', 'job-0002',
        '--evidence', 'Both exact fixture files independently checked by the fixture reviewer and published')
    run('ha', 'context', 'wall')
    for r in rows('reviews'):
        print('REVIEW:', json.dumps(r, indent=2), flush=True)
    local = run('git', '-C', HOME / 'repo', 'rev-parse', 'main').stdout.strip()
    remote = run('git', '--git-dir', HOME / 'remote.git', 'rev-parse', 'main').stdout.strip()
    assert local == remote
    print('EXPECTED: two sequential fixtures accepted, landed and published; completed plan stays done.', flush=True)
    print('ACTUAL: both piles complete, local main equals remote main. Installation is NOT established: '
          'scratch repository has no install target (install_required=false).', flush=True)
    return 0


def recovery():
    setup_tasks()
    config = HOME / '.config/herdr-ade/config.toml'
    config.write_text(config.read_text().replace('[routing]\n', '[routing]\nretries = 1\n'))
    (HOME / 'control.json').write_text(json.dumps({'hold': [], 'fail_at': 'working'}))
    start('job-0001').check_returncode()
    exhausted = wait('one automatic retry then budget exhaustion', lambda: next((r for r in rows('threads')
        if r['id'] == 't-0001' and r['status'] == 'failed' and not r['recovery_pending']
        and 'recovery_exhausted' in r['error']), None), 600)
    print('EXHAUSTED RECORD:', json.dumps(exhausted, indent=2), flush=True)
    run('ha', 'overview', 'wall')
    run('ha', 'context', 'wall')
    # The automatic retry must preserve recipe and consume exactly one budget unit.
    assert exhausted['attempt'] == 2
    assert exhausted['launch']['same_recipe_retries'] == 1
    assert exhausted['launch']['recipe_id'] == 'wall_lane'
    print('EXPECTED: budget exhausted after attempt 2; every literal next command is executable.', flush=True)
    import re
    commands = []
    for notice in exhausted.get('start_notices', []):
        for command in re.findall(r'next: (ha thread retry [^\n]+)', notice['line']):
            command = command.split(' — next: ')[0]
            if command not in commands:
                commands.append(command)
    # Recover with the public command after removing the deliberate process exit.
    (HOME / 'control.json').write_text(json.dumps({'hold': ['before-seal']}))
    failed_commands = []
    for command in commands:
        print('NEXT EXACT:', command, flush=True)
        result = run(*shlex.split(command), check=False)
        if result.returncode:
            failed_commands.append({'command': command, 'output': result.stdout})
    print('ACTUAL command failures:', json.dumps(failed_commands), flush=True)
    if not commands:
        raise RuntimeError('no printed retry command found')
    wait('manual retry starts after exhausted budget', lambda: next((r for r in rows('threads')
         if r['id'] == 't-0001' and r['attempt'] >= 3 and r['brief_submitted']), None), 240)
    print('RECOVERY: explicit printed retry launched original recipe past automatic budget.', flush=True)
    return 1 if failed_commands else 0


if __name__ == '__main__':
    sys.exit({'goal-wait': goal_wait, 'journey': journey, 'recovery': recovery,
              'notice-probe': lambda: setup_tasks(observe_notice=True)}[sys.argv[1]]())
