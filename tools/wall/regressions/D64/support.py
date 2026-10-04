"""Reset-first host driver for D64–D66, default sandbox unless --instance N."""
import argparse
from pathlib import Path
import shlex
import subprocess
import tempfile

WALL = Path(__file__).resolve().parents[2] / 'wall'


class Sandbox:
    def __init__(self):
        parser = argparse.ArgumentParser()
        parser.add_argument('--instance', type=int, choices=range(1, 9))
        args = parser.parse_args()
        self.command = ['sudo', '-n', str(WALL)]
        if args.instance:
            self.command += ['--instance', str(args.instance)]

    def run(self, *args, check=True, input=None):
        command = self.command + list(args)
        print('+', shlex.join(command), flush=True)
        result = subprocess.run(command, text=True, input=input, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, timeout=360)
        print(result.stdout, end='', flush=True)
        print('EXIT', result.returncode, flush=True)
        if check:
            result.check_returncode()
        return result

    def guest(self, code, check=True):
        prefix = ('import os\nfrom pathlib import Path\nhome = Path(os.environ["HOME"])\n'
                  'assert os.geteuid() != 0 and home.name.startswith("wall")\n'
                  'state = home / ".herdr-ade/wall/.state"\n')
        # Send a plain file, never encoded shell text.
        with tempfile.TemporaryDirectory(prefix='wall-records-') as directory:
            script = Path(directory) / 'records-check.py'
            script.write_text(prefix + code)
            self.run('enter', 'tee "$HOME/tools/records-check.py" >/dev/null',
                     input=script.read_text())
        return self.run('enter', 'python3 "$HOME/tools/records-check.py"', check=check)

    def ha(self, *args, check=True):
        return self.run('enter', shlex.join(['ha', *args]), check=check)

    def reset(self):
        self.run('reset')
        self.ha('--version')
        self.ha('ticker', 'stop')

    def apply_condition(self, record, condition):
        self.run('fault', 'interrupted-write', record, condition)
