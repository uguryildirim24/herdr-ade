"""Instance-scoped live repro utilities. Never addresses ubuntu's ADE root."""
import json
from pathlib import Path
import shlex
import subprocess
import time

REPO = Path(__file__).resolve().parents[4]
WALL = REPO / 'tools/wall/wall'
INSTANCE = None


def set_instance(number):
    global INSTANCE
    INSTANCE = number


def wall_command(*args):
    return ['sudo', str(WALL), *(['--instance', str(INSTANCE)] if INSTANCE else []),
            *map(str, args)]


def wall(*args, check=True, timeout=180):
    command = wall_command(*args)
    print('+', shlex.join(command), flush=True)
    result = subprocess.run(command, text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, timeout=timeout)
    print(result.stdout, end='', flush=True)
    if check:
        result.check_returncode()
    return result


def enter(command, box=False, check=True, timeout=180):
    return wall('enter', *(['--box'] if box else []), command,
                check=check, timeout=timeout)


def py(code, box=False, check=True):
    return enter('python3 -c ' + shlex.quote(code), box, check)


def quiet_py(code, box=False):
    command = wall_command('enter', *(['--box'] if box else []),
                           'python3 -c ' + shlex.quote(code))
    result = subprocess.run(command, text=True, capture_output=True, timeout=45)
    if result.returncode:
        raise RuntimeError(result.stdout + result.stderr)
    return json.loads(result.stdout)


def until(check, label, seconds=150):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        last = check()
        if last:
            print('OBSERVED', label, json.dumps(last), flush=True)
            return last
        time.sleep(1)
    raise RuntimeError(f'timeout waiting for {label}: {last!r}')


def records(box=False, kind='threads'):
    assert kind in ('threads', 'ops', 'reviews', 'lanes', 'events')
    return quiet_py('import pathlib,os,json,tomllib; '
                    f'p=pathlib.Path(os.environ["HOME"])/".herdr-ade/wall/.state/{kind}"; '
                    'print(json.dumps([tomllib.loads(f.read_text()) for f in sorted(p.glob("*.toml"))]))', box)


def record(thread='t-0001'):
    return next(r for r in records() if r['id'] == thread)


def control(value, box=False):
    py('import pathlib,os; '
       f'(pathlib.Path(os.environ["HOME"])/"control.json").write_text({json.dumps(value)!r})', box)


def reset(value=None, box=False):
    wall('reset')
    enter('ha --version; sha256sum "$HOME/bin/herdr-ade"; ha overview wall')
    if value is not None:
        control(value, box)
    # ha open is asynchronous in this build. Wait before guest.open_project's
    # old D27 fallback, which otherwise mistakes a pending start for failure.
    enter('ha open wall')
    until(lambda: quiet_py('import subprocess,json; '
          'r=json.loads(subprocess.check_output(["herdr","agent","list"])); '
          'print(json.dumps(any(a.get("name")=="hp-wall-coordinator" '
          'for a in r["result"]["agents"])))'), 'scripted coordinator ready')
    enter('python3 "$HOME/tools/guest.py" open')


def start(box=False):
    enter('python3 "$HOME/tools/guest.py" ' + ('remote-lane' if box else 'lane'))
    return records()[-1]


def phase(thread, name, box=False):
    until(lambda: record(thread)['pane_id'], 'lane placed')
    if box:
        until(lambda: any(r['thread'] == thread and r['pane_id'] == record(thread)['pane_id']
                          for r in records(True, 'lanes')), 'box lane card provisioned')
    enter('python3 "$HOME/tools/guest.py" wait ' + shlex.join([thread, name]), box)


def fault(kind, thread='t-0001', box=False, extra=()):
    args = [kind]
    if kind.startswith('kill-'):
        args += [thread]
    args += list(extra)
    if box:
        args += ['--box']
    return wall('fault', *args)


def snapshot():
    for box in (False, True):
        enter('ha overview wall; python3 "$HOME/tools/guest.py" records; '
              'herdr pane list; herdr agent list; ha ticker status; '
              'git -C "$HOME/repo" worktree list --porcelain; '
              'git --git-dir="${HOME%/box}/remote.git" for-each-ref', box, check=False)
        py('import pathlib,os; p=pathlib.Path(os.environ["HOME"])/".herdr-ade/wall/.state"; '
           '[(print(str(f)),print(f.read_text())) for k in ("ops","reviews","events") '
           'for f in sorted((p/k).glob("*.toml"))]', box)


def evidence(destination):
    wall('evidence', destination)


def seal_again(thread='t-0001', box=False):
    r = record(thread)
    token = f'wall/{thread}/{r["attempt"]}/{r["launch"]["brief_hash"]}'
    return enter('cd ' + shlex.quote(r.get('box_worktree') or r['worktree_path']) + '; '
                 f'HERDR_PANE_ID={shlex.quote(r["pane_id"])} '
                 f'HERDR_ADE_LAUNCH={shlex.quote(token)} ha done', box, check=False)
