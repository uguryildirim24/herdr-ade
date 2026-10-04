#!/usr/bin/env python3
# Only touches the named sandbox instance's own .herdr-ade state.
import argparse
import os
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('home')
parser.add_argument('record')
parser.add_argument('condition', choices=['empty', 'truncated', 'unreadable'])
args = parser.parse_args()
home = Path(os.environ['HOME'])
assert home == Path(args.home) and os.geteuid() != 0
state = (home / '.herdr-ade/wall/.state').resolve(strict=True)
relative = Path(args.record)
assert not relative.is_absolute() and '..' not in relative.parts
path = (state / relative).resolve(strict=True)
assert path.is_relative_to(state) and path.is_file()
before = path.read_bytes()
after = (b'' if args.condition == 'empty' else before[:len(before)//2]
         if args.condition == 'truncated' else b'! interrupted write !\n')
path.write_bytes(after)
print('APPLIED', args.condition, path, 'bytes', len(before), '->', len(after))
