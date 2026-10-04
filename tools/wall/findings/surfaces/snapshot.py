#!/usr/bin/env python3
"""Read-only surface capture, run inside the selected sandbox account."""
from pathlib import Path
import subprocess

home = Path.home()
root = home / '.herdr-ade'
project = root / 'wall'
commands = [
    ['ha', '--version'], ['ha', 'overview', 'wall'],
    ['ha', '--json', 'overview', 'wall'],
    ['ha', 'context', 'wall', '--peek', '--full'], ['ha', 'handoff', 'wall'],
    ['ha', 'plan', 'show', 'wall'], ['ha', 'task', 'list', 'wall'],
    ['ha', 'task', 'show', 'wall', 'job-0001'], ['ha', 'doctor'],
    ['herdr-rundown', '--print'],
]
for command in commands:
    print('\n=== COMMAND ' + ' '.join(command), flush=True)
    try:
        result = subprocess.run(command, text=True, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, timeout=70)
        print(result.stdout, end='', flush=True)
        print('EXIT', result.returncode, flush=True)
    except subprocess.TimeoutExpired as error:
        print('TIMEOUT', error.stdout, flush=True)
print('\n=== DURABLE RECORDS', flush=True)
for directory in ['threads', 'tasks', 'events', 'reviews', 'outbox', 'inbox']:
    for file in sorted((project / '.state' / directory).glob('*')):
        if file.is_file():
            print('\n=== RECORD', file, flush=True)
            print(file.read_text(errors='replace'), flush=True)
for file in sorted((project / '.state').glob('*')):
    if file.is_file() and ('notice' in file.name or 'outbox' in file.name or file.name == 'plan.toml'):
        print('\n=== RECORD', file, flush=True)
        print(file.read_text(errors='replace'), flush=True)
for file in [root / '.ticker.health', root / '.ticker.log']:
    if file.is_file():
        print('\n=== LOG', file, flush=True)
        print('\n'.join(file.read_text(errors='replace').splitlines()[-100:]), flush=True)
