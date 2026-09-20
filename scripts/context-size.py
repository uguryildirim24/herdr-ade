#!/usr/bin/env python3
"""Compare context on the same synthetic 176-message backlog (no live state).

Usage: python3 scripts/context-size.py BEFORE_BINARY AFTER_BINARY OUTPUT_DIR
Only the executable-dependent Commands line is normalized to `Commands: ha`.
"""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

before, after, output = map(Path, sys.argv[1:])
output.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(prefix="ade-context-size-") as home:
    root = Path(home) / "root"
    env = {"HOME": home}

    def run(binary, *args):
        return subprocess.check_output(
            [str(binary.resolve()), "--root", str(root), *args], env=env
        ).decode()

    run(before, "new", "demo")
    project = root / "demo"

    def write(path, text):
        path = project / path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def message(n, kind, subject, summary):
        ident = f"20260920T000000Z-{kind}-{subject}-{n:03}"
        front = {"id": ident, "kind": kind, "subject": subject,
                 "created": "2026-09-20T00:00:00Z", "summary": summary}
        write(f"inbox/{ident}.md", "+++\n" + "".join(
            f"{k} = {json.dumps(v)}\n" for k, v in front.items()) + "+++\n")

    for n in range(1, 66):
        ident = f"t-0001-1-{n}"
        write(f"ops/{ident}.toml", f'''op = "{ident}"
revision = 1
thread = "t-0001"
attempt = 1
kind = "done"
helper_pid = 1
event = "{ident}"
state = "abandoned"
created = "2026-09-20T00:00:00Z"
[recipient]
pane = "w1:p1"
coordinator_attempt = 1
[requested]
sha = "abc"
report_path = "report.md"
''')
        message(n, "preparation-abandoned", "t-0001", "completion preparation was abandoned")
    for n in range(1, 41):
        thread = f"t-{n:04}"
        write(f"threads/{thread}.toml", f'''id = "{thread}"
title = "Check the change {n}"
status = "open"
attempt = 1
last_group = "ready-for-review"
last_state = "done"
report_hash = "report-{n}"
''')
        message(65 + n, "thread-state", thread, f'{thread} is now Ready for review (done)')
        message(105 + n, "report-available", thread,
                f'{thread} has a new report: threads/{thread}.md; report bytes are not a completion')
        if n <= 20:
            ident = f"{thread}-1-1"
            write(f"events/{ident}.toml", f'''id = "{ident}"
op = "{ident}"
thread = "{thread}"
attempt = 1
created = "2026-09-20T00:00:00Z"
[recipient]
pane = "w1:p1"
coordinator_attempt = 1
[payload.done]
sha = "sha-{n}"
report_path = "threads/{thread}.md"
artifact = "report-{n}"
''')
            message(145 + n, "done", thread,
                    f'{thread} completed with report threads/{thread}.md at sha-{n}')
    for n in range(1, 11):
        write(f".state/rounds/r{n}.toml", f'''phase = "under_review"
round = "r{n}"
branch = "main"
plain = "This round checks the change."
gates = []
policy_hash = "fixture"
announced = "verdict:MERGE"
attention = "Round r{n} has a merge verdict; run `round merge r{n}`"
[manifest]
revision = 1
members = []
''')
        message(165 + n, "round-advance", f"r{n}",
                f'Round r{n} has a merge verdict; run `round merge r{n}`')
    message(176, "routine", "nightly", "routine `nightly` is due")
    for label, binary in [("before", before), ("after", after)]:
        text = run(binary, "context", "demo", "--peek")
        text = "Commands: ha\n" + text.split("\n", 1)[1]
        (output / f"context-{label}.txt").write_text(text)
        print(f"{label}: {len(text.encode())} bytes, {len(text.splitlines())} lines, "
              f"{len(text.split())} whitespace words")
