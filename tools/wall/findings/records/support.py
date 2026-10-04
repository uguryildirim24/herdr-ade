"""Host driver for durability checks against one disposable wall instance."""
import argparse
import json
from pathlib import Path
import shlex
import subprocess

WALL = Path(__file__).resolve().parents[2] / 'wall'


class Sandbox:
    def __init__(self):
        parser = argparse.ArgumentParser()
        parser.add_argument('--instance', type=int, choices=range(1, 9))
        parser.add_argument('--evidence', type=Path, required=True)
        self.args = parser.parse_args()
        self.command = ['sudo', '-n', str(WALL)]
        if self.args.instance:
            self.command += ['--instance', str(self.args.instance)]
        self.args.evidence.mkdir(parents=True, exist_ok=False)
        self.log = (self.args.evidence / 'commands.log').open('w')
        self.serial = 0

    def run(self, *args, check=True, input=None):
        command = self.command + list(args)
        print('+', shlex.join(command), flush=True, file=self.log)
        result = subprocess.run(command, text=True, input=input, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, timeout=360)
        print(result.stdout, end='', flush=True)
        print(result.stdout, end='', flush=True, file=self.log)
        print('EXIT', result.returncode, flush=True, file=self.log)
        if check:
            result.check_returncode()
        return result

    def guest(self, code, check=True):
        # Every state path in these snippets is relative to the selected SSH HOME.
        prefix = 'import os\nfrom pathlib import Path\nhome = Path(os.environ["HOME"])\nassert os.geteuid() != 0 and home.name.startswith("wall")\nstate = home / ".herdr-ade/wall/.state"\n'
        self.serial += 1
        script = self.args.evidence / f'guest-{self.serial:03}.py'
        script.write_text(prefix + code)
        self.run('enter', 'tee "$HOME/tools/records-check.py" >/dev/null', input=script.read_text())
        return self.run('enter', 'python3 "$HOME/tools/records-check.py"', check=check)

    def ha(self, *args, check=True):
        return self.run('enter', shlex.join(['ha', *args]), check=check)

    def reset(self):
        self.run('reset')
        self.ha('--version')
        self.ha('ticker', 'stop')

    def apply_condition(self, record, condition):
        self.run('fault', 'interrupted-write', record, condition)

    def evidence(self):
        self.run('evidence', str(self.args.evidence / 'capture'))


SETUP = r'''
import json, subprocess, sys, time
sys.path.insert(0, str(home / 'tools'))
import guest
config = home / '.config/herdr-ade/config.toml'
config.write_text(config.read_text().replace('[adapters.pi]',
    '[[routing.rules]]\nworkflow = "reviewer"\nrecipe = "wall_lane"\n\n[adapters.pi]'))
(home / 'control.json').write_text(json.dumps({'hold':['before-seal', 'mid-review']}))
guest.run('ha', 'ticker', 'start')
lane = guest.lane()
guest.wait(lane['id'], 'before-seal')
(home / 'release/before-seal').touch()
guest.poll('sealed event', lambda: list((state / 'events').glob('*.toml')))
guest.run('ha', 'review', 'wall')
guest.poll('reviewer', lambda: len(guest.records()) == 2)
reviewer = guest.records()[-1]
guest.poll('reviewer brief', lambda: guest.target(reviewer['id']).get('brief_submitted'), seconds=120)
guest.wait(reviewer['id'], 'mid-review')
guest.run('ha','note','add','wall','Keep measured results','--kind','memory',
          '--request',(home/'request').read_text())
guest.run('ha','plan','set','wall','--does','Durable progress')
guest.run('ha','plan','step','add','wall','Keep progress','--task','job-0001')
guest.poll('input hold', lambda: (state/'coordinator-input-hold.json').exists())
p = state/'coordinator-input-hold.json'
v = json.loads(p.read_text()); v['since'] -= 1900; v['notified'] = False
p.write_text(json.dumps(v))
guest.poll('draft notice', lambda: list((state/'inbox').glob('*.md')), seconds=60)
guest.run('ha','ticker','stop')
if not (state/'notice-batch.json').exists():
    # Explicit storage fixture, not a claim that a notice was delivered.
    (state/'notice-batch.json').write_text(json.dumps({'first_at':1,'entries':[
        {'source':{'Transition':'durability-fixture'},'line':'durability-fixture'}]}))
'''
