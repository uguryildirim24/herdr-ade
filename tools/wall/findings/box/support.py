"""Host-side helpers for numbered, disposable box finding replays."""
import json
import shlex
import subprocess
import time
from pathlib import Path

WALL = Path(__file__).resolve().parents[2] / 'wall'


class Sandbox:
    def __init__(self, instance):
        if instance not in range(1, 9):
            raise ValueError('a numbered instance is required')
        self.instance = instance
        self.home = f'/home/wall-{instance}'
        self.machine = f'wall-box-{instance}'

    def wall(self, *args, check=True, timeout=180):
        command = ['sudo', str(WALL), '--instance', str(self.instance), *map(str, args)]
        print('+', shlex.join(command), flush=True)
        result = subprocess.run(command, text=True, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, timeout=timeout)
        print(result.stdout, end='', flush=True)
        if check:
            result.check_returncode()
        return result

    def enter(self, command, box=False, **kwargs):
        return self.wall('enter', *(['--box'] if box else []), command, **kwargs)

    def py(self, code, box=False):
        return self.enter('python3 -c ' + shlex.quote(code), box=box).stdout

    def control(self, value, box=True):
        self.py('import os,json; from pathlib import Path; '
                f'(Path.home()/"control.json").write_text({json.dumps(json.dumps(value))})', box)

    def records(self, box=False):
        return json.loads(self.enter('python3 "$HOME/tools/guest.py" records', box).stdout)

    def record(self, thread='t-0001', box=False):
        return next(r for r in self.records(box) if r['id'] == thread)

    def wait(self, predicate, seconds=90):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            result = predicate()
            if result:
                return result
            time.sleep(2)
        raise TimeoutError(f'condition not met after {seconds}s')

    def open(self):
        self.enter('python3 "$HOME/tools/guest.py" open')

    def start(self, wait=True):
        self.py('import sys; from pathlib import Path; '
                'sys.path.insert(0,str(Path.home()/"tools")); import guest; '
                f'guest.lane(remote=True,wait_ready={wait})')

    def phase(self, phase, thread='t-0001'):
        self.enter(f'python3 "$HOME/tools/guest.py" wait {shlex.quote(thread)} {shlex.quote(phase)}', True)

    def evidence(self, destination):
        self.wall('evidence', Path(destination).resolve())
