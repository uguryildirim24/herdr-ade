#!/usr/bin/python3
"""Real open/close/open and Herdr transport; scripted Pi, no provider claim."""
import json
import os
from pathlib import Path
import subprocess

home = Path(os.environ['HOME'])
assert home.name in [f'wall-{n}' for n in range(1, 9)]
project = home / '.herdr-ade/wall'
run = lambda *args: subprocess.run(args, check=True, capture_output=True, text=True)
run('ha', 'open', 'wall')
first = json.loads((project / '.state/coordinator.json').read_text())
run('herdr', 'tab', 'close', first['tab_id'])
run('ha', 'close', 'wall')
real = run('which', 'herdr').stdout.strip()
wrapper = home / 'second-open-herdr'
wrapper.write_text('''#!/usr/bin/python3
import json, subprocess, sys
from pathlib import Path
real = REAL
args = sys.argv[1:]
if args[:2] == ['pane', 'report-metadata']:
    Path(METADATA).write_text(json.dumps(args))
if args[:2] == ['agent', 'start'] and '--pane' in args:
    pane = args[args.index('--pane') + 1]
    rows = json.loads(subprocess.check_output([real, 'pane', 'list'], text=True))['result']['panes']
    row = next(row for row in rows if row['pane_id'] == pane)
    Path(PROOF).write_text(json.dumps(row))
    # Spawn the real scripted coordinator, then force the readiness-error exit
    # that previously skipped ADE's post-start ownership publication.
    subprocess.run([real, *args], capture_output=True)
    print(json.dumps({'error': {'code': 'agent_name_not_found', 'message': 'D46 forced post-spawn readiness failure'}}), file=sys.stderr)
    sys.exit(1)
subprocess.run([real, *args], check=True)
'''.replace('REAL', repr(real)).replace('PROOF', repr(str(home / 'D46-before-start.json'))).replace('METADATA', repr(str(home / 'D46-metadata-args.json'))))
wrapper.chmod(0o755)
second = subprocess.run(['ha', 'open', 'wall'], env=dict(os.environ, HERDR_BIN_PATH=str(wrapper)), capture_output=True, text=True)
print('EXPECTED: failed second open has project=wall/thread=coordinator tokens BEFORE agent start; no TTL', flush=True)
row = json.loads((home / 'D46-before-start.json').read_text())
print('ACTUAL: second-open exit', second.returncode, 'before-start pane', json.dumps(row), flush=True)
assert second.returncode != 0
assert row.get('tokens', {}).get('project') == 'wall'
assert row.get('tokens', {}).get('thread') == 'coordinator'
metadata_args = json.loads((home / 'D46-metadata-args.json').read_text())
assert '--ttl-ms' not in metadata_args
print('ACTUAL: persistent ownership publication', json.dumps(metadata_args), flush=True)
# This Linux sandbox proves startup ownership. The actual journey shutdown
# driver is exercised by the real-process regression and the live Mac run.
