#!/usr/bin/python3
"""Unprivileged tools. All paths/identities are resolved inside this sandbox."""
import json
import os
import pwd
from pathlib import Path
import subprocess
import sys
import tarfile
import time
import tomllib

from instance import AUTH, Instance

INSTANCE = Instance(int(os.environ.get('WALL_INSTANCE', '0')))
HOME = Path(os.environ['HOME'])
ROOT = HOME / '.herdr-ade'
PROJECT = ROOT / 'wall'


def run(*args, **kwargs):
    return subprocess.run(list(map(str, args)), check=True, **kwargs)


def records():
    rows = [tomllib.loads(p.read_text()) for p in sorted((PROJECT / '.state/threads').glob('*.toml'))]
    for file in sorted((PROJECT / '.state/lanes').glob('*.toml')):
        card = tomllib.loads(file.read_text())
        worktree = Path(card['box_worktree'])
        rows.append(dict(card, id=card['thread'], worktree_path=str(worktree),
                         thread_dir=str(worktree / '.herdr-project' / f"wall-{card['thread']}")))
    return rows


def target(value):
    return next(r for r in records() if r['id'] == value)


def confined_file(value):
    path = (PROJECT / value).resolve(strict=True)
    if not path.is_relative_to(PROJECT.resolve()) or not path.is_file():
        raise ValueError('target must be a regular record inside the sandbox project')
    return path


def boot():
    (HOME / '.config/herdr').mkdir(parents=True, exist_ok=True)
    with (HOME / 'server-start.log').open('ab') as output:
        subprocess.Popen(['herdr', 'server'], stdout=output, stderr=output,
                         stdin=subprocess.DEVNULL, start_new_session=True)
    for _ in range(100):
        ready = subprocess.run(['herdr','workspace','list'], stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, check=False)
        if ready.returncode == 0:
            break
        time.sleep(0.1)
    else:
        raise RuntimeError('sandbox herdr did not publish its socket')
    run('ha', 'ticker', 'start')


def remove_empty_provider(value):
    # Setup creates an empty OpenCode override even when this Codex-only
    # sandbox declares no DeepSeek recipe. Pi rejects that empty provider.
    providers = value.get('providers', {})
    if providers.get('opencode-go') == {'modelOverrides': {}}:
        del providers['opencode-go']
        return True
    return False


def init():
    (HOME / 'bin/git').symlink_to(HOME / 'tools/slow-git')
    (HOME / 'worktrees').mkdir()
    (HOME / 'build').mkdir()
    config = HOME / '.config/herdr-ade'
    config.mkdir(parents=True, exist_ok=True)
    config.joinpath('config.toml').write_text('''[doctor]
min_free_disk_gb = 0.01

[routing]
default = "wall_lane"
[[routing.rules]]
workflow = "coordinator"
recipe = "wall_coordinator"

[adapters.pi]
binary = "wall-pi"
coordinator = true
ready_timeout_ms = 30000
[adapters.pi.hook]
shape = "pi"
path = ".pi/herdr-ade-hooks.json"
events = ["Stop", "UserPromptSubmit"]
prompt_event = "UserPromptSubmit"
[adapters.pi.doctor]
readiness = "command"
args = ["--wall-check", "{args}"]

[recipes.wall_lane]
kind = "pi"
args = ["--wall-lane"]
ready_timeout_ms = 30000
plain = "the scripted wall lane"
[recipes.wall_coordinator]
kind = "pi"
args = ["--wall-coordinator"]
ready_timeout_ms = 30000
plain = "the scripted wall coordinator"
[recipes.wall_real]
kind = "pi"
provider = "openai-codex"
args = ["--provider", "openai-codex", "--model", "gpt-6.1-astra", "--thinking", "max", "--no-skills"]
ready_timeout_ms = 300000
plain = "the real wall lane"
''')
    (ROOT / 'pi').mkdir(parents=True)
    (ROOT / 'pi/npm').symlink_to(HOME / 'pi-package', target_is_directory=True)
    # Setup is production code: independent pin, hook/guard and empty auth.
    # The shim opts into script mode only for explicitly scripted recipes.
    run('git', 'config', '--global', 'user.name', 'Wall scripted lane')
    run('git', 'config', '--global', 'user.email', 'wall@localhost')
    run('git', 'config', '--global', 'init.defaultBranch', 'main')
    run('git', 'config', '--global', '--add', 'safe.directory', INSTANCE.home / 'remote.git')
    run('git', 'init', '--bare', HOME / 'remote.git')
    run('git', 'clone', HOME / 'remote.git', HOME / 'repo')
    repo = HOME / 'repo'
    repo.joinpath('README.md').write_text('# Isolated wall scratch repository\n')
    repo.joinpath('.gitignore').write_text('.herdr-project/\n.worktrees/\n')
    run('git', '-C', repo, 'add', 'README.md', '.gitignore')
    run('git', '-C', repo, 'commit', '-m', 'Wall baseline', env=dict(os.environ,
        GIT_AUTHOR_DATE='2026-10-04T00:00:00Z', GIT_COMMITTER_DATE='2026-10-04T00:00:00Z'))
    run('git', '-C', repo, 'push', '-u', 'origin', 'main')
    boot()
    run('herdr-pi','setup', env=dict(os.environ, NPM_CONFIG_OFFLINE='true'))
    # Only credentials are shared; hooks, models, sessions and config stay private.
    auth = ROOT / 'pi/agent/auth.json'
    if auth.exists():
        raise ValueError('setup unexpectedly created credentials')
    auth.symlink_to(AUTH / 'auth.json')
    models = ROOT / 'pi/agent/models.json'
    value = json.loads(models.read_text())
    if remove_empty_provider(value):
        print('FINDING: setup emitted an empty opencode-go provider; removed only that empty '
              'declaration from the Codex-only sandbox so real pi can reach login.', flush=True)
        models.write_text(json.dumps(value, indent=2) + '\n')
    (HOME / '.local/bin').mkdir(parents=True)
    (HOME / '.local/bin/pi').symlink_to(ROOT / 'pi/bin/pi')
    wrapper = ROOT / 'pi/bin/pi'
    wrapper.write_text(wrapper.read_text().replace('#!/bin/sh\n',
        '#!/bin/sh\n# Sandbox-only scripted dispatch; all real pi args keep the production wrapper.\n'
        'case "$*" in *--wall-*) exec "$HOME/bin/wall-pi" "$@" ;; esac\n', 1))
    (HOME / 'tools/bin').symlink_to(HOME / 'bin', target_is_directory=True)
    run('herdr', 'plugin', 'link', HOME / 'tools')
    run('ha', 'new', 'wall', '--repo', repo, '--goal', 'Break the isolated harness')
    (HOME / 'release').mkdir()
    (HOME / 'control.json').write_text(json.dumps({'hold':['before-seal']}))


def open_project(strict=False):
    opened = subprocess.run(['ha', 'open', 'wall'], check=False)
    if strict:
        opened.check_returncode()
        coordinator = json.loads((PROJECT / '.state/coordinator.json').read_text())
        poll('unassisted coordinator registration', lambda: coordinator['agent_name'] in
             subprocess.check_output(['herdr', 'agent', 'list'], text=True))
    coordinator = json.loads((PROJECT / '.state/coordinator.json').read_text())
    # A successful modern ha open may return before the asynchronous hook
    # publishes the agent name. Never double-start that occupied pane.
    if opened.returncode:
        agents = subprocess.check_output(['herdr','agent','list'], text=True)
        if coordinator['agent_name'] not in agents:
            print('FINDING D27: explicit sandbox bootstrap after ha open timeout; not a harness fix', flush=True)
            run('herdr','agent','start',coordinator['agent_name'],'--kind','pi',
                '--pane',coordinator['pane_id'],'--timeout','30000','--','--wall-coordinator')
            run('ha','open','wall')
        else:
            opened.check_returncode()
    binding = json.loads((PROJECT / '.state/coordinator-hook.json').read_text())
    env = dict(os.environ, HERDR_PANE_ID=binding['pane'])
    reply = subprocess.check_output(['ha','hook','--kind','pi','--project','wall',
        '--binding',binding['pane'],'--phase','prompt'], env=env, text=True,
        input=json.dumps({'session_id':'wall-test-request',
                          'prompt':'Run scripted fault lanes in this disposable project'}))
    request = reply.strip().removeprefix('request ')
    (HOME / 'request').write_text(request)
    print('Wall request:',request)


def connect():
    if HOME != INSTANCE.home:
        raise ValueError('connect runs on the local account only')
    machine = INSTANCE.machine
    box = HOME / 'box'
    (HOME / '.ssh/config').write_text(f'''Host {machine}
  HostName 127.0.0.1
  Port {INSTANCE.box_port}
  User {INSTANCE.box_user}
  IdentityFile {HOME}/.ssh/box_key
  IdentitiesOnly yes
  StrictHostKeyChecking yes
  UserKnownHostsFile {HOME}/.ssh/known_hosts
''')
    run('herdr','machine','add',machine,'--label',machine)
    config = HOME / '.config/herdr-ade/config.toml'
    with config.open('a') as out:
        out.write(f'''
[machines.{machine}]
label = "{machine}"
target = "{machine}"
session = "default"
home = "{box}"
root = "{box}/.herdr-ade"
worktrees = "{box}/worktrees"
build = "{box}/build"
path = "{box}/.local/bin:{box}/bin:/usr/local/bin:/usr/bin:/bin"
ade_bin = "{box}/bin/herdr-ade"
pi_bin = "{box}/bin/herdr-pi"
kinds = ["pi"]
[[machines.{machine}.repos]]
path = "{HOME}/repo"
box_path = "{box}/repo"
publish_url = "{HOME}/remote.git"
''')
    run('chmod','-R','a+rwX',HOME / 'remote.git')
    run('ssh',machine,f'git -C "$HOME/repo" remote set-url origin {HOME}/remote.git; '
        'git -C "$HOME/repo" fetch origin; git -C "$HOME/repo" reset --hard origin/main')


def lane(remote=False, wait_ready=True):
    if not (HOME / 'request').exists():
        open_project()
    task = HOME / 'task.md'
    task.write_text('Read the frozen brief, commit a scratch file, write your report and seal.\n')
    command = ['ha','thread','start','wall','--task-file',task,'--title','Scripted fault lane',
        '--repo',HOME / 'repo','--recipe','wall_lane','--request',(HOME / 'request').read_text(),
        '--acceptance','Scripted fault plumbing observed']
    if remote:
        command += ['--machine', INSTANCE.machine]
    run(*command)
    record = records()[-1]
    if wait_ready:
        poll('lane brief submission', lambda: target(record['id']).get('brief_submitted'), seconds=60)
        record = target(record['id'])
    print(json.dumps(record, indent=2), flush=True)
    return record


def wait(thread, phase):
    record = target(thread)
    for _ in range(1200):
        for file in (HOME / 'runs').glob('*.jsonl'):
            for line in file.read_text().splitlines():
                row = json.loads(line)
                if row['pane'] == record['pane_id'] and row['phase'] == phase:
                    print(json.dumps(row), flush=True)
                    return
        time.sleep(0.05)
    raise RuntimeError(f'timed out waiting for {thread} phase {phase}')


def fault(kind, args):
    if kind in ['kill-pane','kill-process']:
        record = target(args[0])
        pane = record['pane_id']
        if kind == 'kill-pane':
            run('herdr','pane','close',pane)
        else:
            # No arbitrary host PID: select the script process from its run
            # evidence and check current uid/command before killing.
            rows = []
            for file in (HOME / 'runs').glob('*.jsonl'):
                rows += [json.loads(line) for line in file.read_text().splitlines()]
            helper = len(args) > 1 and args[1] == 'seal'
            phase = 'seal-helper' if helper else 'ready'
            row = next(r for r in reversed(rows) if r['pane'] == pane and r['phase'] == phase)
            pid = row['helper_pid'] if helper else row['pid']
            proc = Path('/proc') / str(pid)
            expected = HOME / ('bin/herdr-ade' if helper else 'bin/node')
            if (proc.stat().st_uid != os.getuid() or (proc / 'exe').resolve() != expected
                    or (proc / 'cwd').resolve() != Path(record['worktree_path'])):
                raise ValueError('target process identity changed')
            os.kill(pid, 9)
            print(f'Killed sandbox {"seal helper" if helper else "scripted agent"} pid={pid}, pane={pane}')
    elif kind == 'ticker':
        run('ha','ticker','status')
        run('ha','ticker','stop')
        run('ha','ticker','start')
        run('ha','ticker','status')
    elif kind == 'fill':
        # The whole HOME is mounted tmpfs, so records/worktrees also get ENOSPC.
        fs = subprocess.check_output(['findmnt','-n','-o','FSTYPE','--target',HOME],text=True).strip()
        if fs != 'tmpfs':
            raise ValueError('disk fill requires confined tmpfs')
        count = 0
        try:
            with (HOME / 'disk-full').open('wb', buffering=0) as out:
                block = bytes(1024 * 1024)
                while True:
                    count += out.write(block)
        except OSError as error:
            if error.errno != 28:
                raise
            print(f'Confined ENOSPC after {count} bytes; remove $HOME/disk-full to recover')
    elif kind == 'corrupt':
        file = confined_file(args[0])
        mode = args[1] if len(args) > 1 else 'truncate'
        if mode not in ['truncate','garbage']:
            raise ValueError('mode must be truncate or garbage')
        before = file.stat().st_size
        file.write_bytes(b'' if mode == 'truncate' else b'{invalid wall record\n')
        print(f'{file}: {before} -> {file.stat().st_size} bytes ({mode})')
    elif kind == 'clock':
        print('FINDING: no injectable harness clock. project::now and direct jiff::Timestamp::now '
              'calls bypass a shared clock; monotonic Runner deadlines are separate. '
              'No host time changed. Clock-skew behavior is NOT ESTABLISHED in v1.')
    else:
        raise ValueError(kind)


def poll(name, check, seconds=120):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.5)
    raise RuntimeError(f'timed out at {name}')


def regression_d27():
    print('EXPECTED: ha open starts and registers its coordinator unassisted', flush=True)
    try:
        open_project(strict=True)
    except Exception as error:
        print(f'ACTUAL: unassisted open failed: {error}', flush=True)
        raise
    print('ACTUAL: ha open succeeded; its recorded agent registered; no bootstrap', flush=True)


def regression_d30():
    lane()
    row = records()[-1]
    file = PROJECT / '.state/threads' / (row['id'] + '.toml')
    # Use a real started lane, not a fixture the harness never produced.
    for invalid in ['{invalid wall record\n', 'id = "cut', '']:
        file.write_text(invalid)
        print(f'EXPECTED: Unreadable lane and {file} in overview (damage={invalid!r})', flush=True)
        result = subprocess.check_output(['ha', 'overview', 'wall'], text=True)
        print('ACTUAL:', result, flush=True)
        if 'Unreadable lane' not in result or str(file) not in result:
            raise RuntimeError('corrupt lane omitted or not identified as unreadable')


def gate_review(thread):
    """Judge only our tiny fixture contract, not arbitrary software semantics."""
    record = target(thread)
    review = tomllib.loads((PROJECT / '.state/reviews' / (record['review_id'] + '.toml')).read_text())
    rows = []
    for member in review['members']:
        run('git', 'merge', '--no-edit', member['sha'])
        filename = f'scripted-{member["thread"]}.txt'
        expected = f'Scripted wall lane {member["thread"]}\n'
        actual = subprocess.check_output(['git', 'show', f'HEAD:{filename}'], text=True)
        if actual != expected:
            raise RuntimeError(f'fixture mismatch: {filename}: {actual!r}')
        tasks = [tomllib.loads(p.read_text()) for p in (PROJECT / '.state/tasks').glob('*.toml')]
        task = next(t for t in tasks if member['thread'] in t['attempts'])
        if task['acceptance'] != ['Scripted fault plumbing observed']:
            raise RuntimeError('scripted reviewer cannot judge a non-fixture criterion')
        rows.append(f'[[acceptance]]\nthread = {json.dumps(member["thread"])}\n'
                    f'event = {json.dumps(member["event"])}\ncriterion = 1\n'
                    f'condition = {json.dumps(task["acceptance"][0])}\nestablished = true\n'
                    f'evidence = {json.dumps("git show HEAD:" + filename + "; exact fixture contents verified")}\n')
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    report = (f'+++\nreview = {json.dumps(review["id"])}\nverdict = "MERGE"\n'
              f'candidate = "{head}"\n' + ''.join(rows) + '+++\n'
              'Scripted fixture review only; no real-model or arbitrary semantic acceptance claimed.\n')
    Path(record['thread_dir'], 'report.md').write_text(report)


def gate_loop():
    # Reset's init creates a fresh project and local bare remote using real ha
    # new. As in journey.rs: verify open, exact seals, publication, then delete.
    if records():
        raise RuntimeError('loop must start from clean reset')
    metadata = PROJECT.stat()
    identity = (metadata.st_dev, metadata.st_ino)
    config = HOME / '.config/herdr-ade/config.toml'
    text = config.read_text().replace('[adapters.pi]',
        '[[routing.rules]]\nworkflow = "reviewer"\nrecipe = "wall_lane"\n\n[adapters.pi]')
    config.write_text(text)
    (HOME / 'control.json').write_text(json.dumps({'hold': ['before-seal'], 'gate_review': True}))
    print('LOOP new: fresh reset project and local bare remote', flush=True)
    open_project(strict=True)
    print('LOOP open: unassisted scripted coordinator', flush=True)
    local = lane(wait_ready=False)['id']
    remote = lane(remote=True, wait_ready=False)['id']
    poll('both lane briefs', lambda: all(target(t).get('brief_submitted') for t in [local, remote]))
    if target(remote).get('machine') != INSTANCE.machine:
        raise RuntimeError('box lane fell back to local')
    # Both lanes exist before enabling the review; neither is allowed to seal
    # until then, so the proof cannot accidentally omit the remote member.
    run('ssh', INSTANCE.machine, 'touch "$HOME/release/before-seal"')
    (HOME / 'release/before-seal').touch()
    def sealed():
        events = [tomllib.loads(p.read_text()) for p in (PROJECT / '.state/events').glob('*.toml')]
        return all(any(e.get('thread') == t and e.get('payload', {}).get('done', {}).get('sha')
                       for e in events) for t in [local, remote])
    poll('both lane seals and courier', sealed)
    print(f'LOOP sealed: {local} local; {remote} {INSTANCE.machine}', flush=True)
    run('ha', 'review', 'wall')
    def landed():
        reviews = [tomllib.loads(p.read_text()) for p in (PROJECT / '.state/reviews').glob('*.toml')]
        for review in reviews:
            if review.get('attention'):
                raise RuntimeError(f'review attention: {review["attention"]}')
        return all(any(r['phase'] == 'complete' and r['fast_forward'] and r['push']
                       and any(m['thread'] == t for m in r['members']) for r in reviews)
                   for t in [local, remote])
    poll('scripted review and landing', landed, seconds=240)
    head = subprocess.check_output(['git', '-C', HOME / 'repo', 'rev-parse', 'main'], text=True).strip()
    pushed = subprocess.check_output(['git', '--git-dir', HOME / 'remote.git',
                                     'rev-parse', 'main'], text=True).strip()
    if head != pushed:
        raise RuntimeError(f'local main {head} != bare main {pushed}')
    for thread in [local, remote]:
        run('git', '--git-dir', HOME / 'remote.git', 'cat-file', '-e', f'main:scripted-{thread}.txt')
    print(f'LOOP landed: both fixture files reviewed and pushed to local bare main {pushed}', flush=True)
    # Preserve project evidence before real delete removes its records.
    temporary = HOME / '.gate-loop-capture'
    with temporary.open('wb') as output:
        run('python3', HOME / 'tools/guest.py', 'evidence', stdout=output)
    temporary.rename(HOME / 'gate-loop-before-delete.tar')
    current = PROJECT.stat()
    if (current.st_dev, current.st_ino) != identity or PROJECT.is_symlink():
        raise RuntimeError('project identity changed before delete')
    run('ha', 'delete', 'wall')
    if PROJECT.exists():
        raise RuntimeError('project still present after delete')
    print('LOOP delete: project absent; SKIP Claude trust probe (Mac only)', flush=True)


def evidence():
    with tarfile.open(fileobj=sys.stdout.buffer, mode='w|') as out:
        for directory in [PROJECT, HOME / 'runs', HOME / '.config/herdr']:
            if directory.exists():
                for file in directory.rglob('*'):
                    if file.is_file() and not file.is_symlink() and file.name != 'config.toml':
                        out.add(file, arcname=str(file.relative_to(HOME)), recursive=False)
        for file in list(ROOT.glob('.ticker.*')) + [HOME / 'server-start.log', HOME / 'control.json',
                                                HOME / 'gate-loop-before-delete.tar']:
            if file.is_file() and not file.is_symlink():
                out.add(file, arcname=str(file.relative_to(HOME)), recursive=False)


if __name__ == '__main__':
    expected_user = INSTANCE.user if HOME == INSTANCE.home else INSTANCE.box_user
    if (HOME not in [INSTANCE.home, INSTANCE.home / 'box'] or os.geteuid() == 0
            or pwd.getpwuid(os.getuid()).pw_name != expected_user):
        sys.exit('guest commands require the selected unprivileged sandbox account')
    command = sys.argv[1]
    if command == 'fault':
        fault(sys.argv[2],sys.argv[3:])
    elif command == 'evidence':
        evidence()
    elif command == 'records':
        print(json.dumps(records()))
    elif command == 'wait':
        wait(sys.argv[2],sys.argv[3])
    elif command == 'remote-lane':
        lane(remote=True)
    elif command == 'gate-review':
        gate_review(sys.argv[2])
    elif command == 'gate-loop':
        gate_loop()
    elif command == 'regression-D27':
        regression_d27()
    elif command == 'regression-D30':
        regression_d30()
    else:
        {'init':init,'boot':boot,'open':open_project,'lane':lane,'connect':connect}[command]()
