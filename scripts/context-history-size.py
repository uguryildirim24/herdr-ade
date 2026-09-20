#!/usr/bin/env python3
"""Measure real context stdout, first on a cloud snapshot, then a history replay.

Usage: context-history-size.py BEFORE AFTER CLOUD_PROJECT FORK PLUGIN OUTPUT
No live writes. The binaries occupy the same executable path in turn, so the
Commands line is identical; outputs are raw bytes, not normalized estimates.
The replay uses committed review prose/pins, with explicitly modeled lifecycle
state: 44 merged rounds, one open round, 79 abandoned preparation revisions.
It is not represented as a copy of the unavailable Mac state.
"""
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tomllib

before, after, cloud, fork, plugin, output = [Path(p).resolve() for p in sys.argv[1:]]
output.mkdir(parents=True, exist_ok=True)
if (output / "cloud").exists() or (output / "history").exists():
    raise SystemExit("Use a fresh output directory to keep measurements immutable")
q = json.dumps
slug = cloud.name
provenance = {"cloud_source": str(cloud), "sources": [], "modeled": [
    "r44 is kept under_review to exercise an open round; other r1..r45 rounds are merged",
    "79 abandoned revisions distributed over real pinned lane attempts; not the Mac's exact ops",
    "one sealed successor per pinned attempt; completed lanes resolved except r44 members",
]}


def write(project, path, text):
    path = project / path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def source(path):
    data = path.read_bytes()
    provenance["sources"].append({"path": str(path), "sha256": hashlib.sha256(data).hexdigest()})
    return data.decode()


# This is an actual project snapshot, not a generated backlog.
shutil.copytree(cloud, output / "cloud" / "root" / slug)
# Build the history-shaped project from the project's committed records, not
# generic repeated sentences. Keep the real cloud project settings too.
project = output / "history" / "root" / slug
project.mkdir(parents=True)
shutil.copy2(cloud / "PROJECT.md", project / "PROJECT.md")
pins = {}
open_threads = set()
for n in range(1, 46):
    paths = [repo / f"tasks/review-r{n}.md" for repo in (fork, plugin)]
    path = next((p for p in reversed(paths) if p.exists()), None)
    if path is None:
        raise SystemExit(f"Missing real review brief r{n}")
    text = source(path)
    plain = re.search(r"^plain: (.+)$", text, re.M).group(1)
    branch = re.search(r"on integration branch `([^`]+)`", text).group(1)
    policy = re.search(r"policy hash `([^`]+)`", text).group(1)
    members = re.findall(r"^\| (t-\d+) \| (\d+) \| `([0-9a-f]+)` \| `([^`]+)` \| `([^`]+)` \|", text, re.M)
    phase = "under_review" if n == 44 else "merged"
    record = f'round = "r{n}"\nphase = "{phase}"\nbranch = {q(branch)}\nplain = {q(plain)}\npolicy_hash = {q(policy)}\n'
    if n == 44:
        record += 'attention = "Review is still open; check the reviewer before proceeding."\n'
    record += '[manifest]\nrevision = 1\n'
    if not members:
        raise SystemExit(f"No real manifest pins in {path}")
    for thread, attempt, sha, event, artifact in members:
        pins[(thread, int(attempt))] = (sha, event, artifact, path.parent.parent)
        if n == 44:
            open_threads.add(thread)
        record += f'[[manifest.members]]\nthread = {q(thread)}\n[manifest.members.pin]\nattempt = {attempt}\nsha = {q(sha)}\nevent = {q(event)}\nartifact = {q(artifact)}\n'
    if phase == "merged":
        verdict_path = path.parent / "reviews" / f"code-r{n}.md"
        verdict_text = source(verdict_path)
        front = verdict_text.split("+++", 2)[1]
        candidate = tomllib.loads(front)["candidate"]
        record += f'[merge]\nop = "merge-r{n}"\nexpected_old = {q(candidate)}\ncandidate = {q(candidate)}\nverdict = {q(candidate)}\nphase = "checkpointed"\n'
    write(project, f".state/rounds/r{n}.toml", record)

# Preserve the real lane names/task prose. Modeled ops carry the actual admitted
# sha and artifact; revisions are explicit simulation, not fabricated evidence.
ordered = sorted(pins.items())
for index, ((thread, attempt), (sha, event, artifact, repo)) in enumerate(ordered):
    brief = repo / f"tasks/{thread}.md"
    title = re.search(r"^plain: (.+)$", source(brief), re.M).group(1)
    status = "open" if thread in open_threads else "resolved"
    write(project, f"threads/{thread}.toml", f'id = {q(thread)}\ntitle = {q(title)}\nstatus = {q(status)}\nattempt = {attempt}\nresolved_reason = "merged"\nreport_hash = {q(artifact)}\n')
    count = 79 // len(ordered) + (index < 79 % len(ordered))
    for revision in range(1, count + 2):
        op = f"{thread}-{attempt}-{revision}"
        state = "abandoned" if revision <= count else "sealed"
        write(project, f"ops/{op}.toml", f'op = {q(op)}\nrevision = 1\nthread = {q(thread)}\nattempt = {attempt}\nkind = "done"\nhelper_pid = 1\nevent = {q(op)}\nstate = {q(state)}\ncreated = "2026-09-20T00:00:00Z"\n[recipient]\npane = "w1:p1"\ncoordinator_attempt = 1\n[requested]\nsha = {q(sha)}\nreport_path = {q("artifacts/" + artifact)}\n')
        if state == "sealed":
            write(project, f"events/{op}.toml", f'id = {q(op)}\nop = {q(op)}\nthread = {q(thread)}\nattempt = {attempt}\ncreated = "2026-09-20T00:00:00Z"\n[recipient]\npane = "w1:p1"\ncoordinator_attempt = 1\n[payload.done]\nsha = {q(sha)}\nreport_path = {q("artifacts/" + artifact)}\nartifact = {q(artifact)}\n')

# Three real sealed remote deliveries, represented as inbox transport messages
# (the cloud does not keep the Mac courier's inbox). Text is derived from events.
for event_path in sorted((cloud / "events").glob("*.toml"))[-3:]:
    event = tomllib.loads(source(event_path))
    ident = event["id"]
    write(project, f"inbox/{ident}.md", f'+++\nid = {q(ident)}\nkind = "courier-delivery"\nsubject = {q(event["thread"])}\nsummary = {q("Received sealed event " + ident + " from oci")}\ncreated = {q(event["created"])}\n+++\n')

provenance["replay_counts"] = {"rounds": 45, "merged": 44, "open": 1, "abandoned_ops": 79, "pinned_attempts": len(pins)}
(output / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n")
results = {}
for fixture in ("cloud", "history"):
    folder = output / fixture
    root = folder / "root"
    home = folder / "home"
    home.mkdir()
    executable = folder / "ha"
    results[fixture] = {}
    for label, binary in (("before", before), ("after", after)):
        shutil.copy2(binary, executable)
        result = subprocess.run([str(executable), "--root", str(root), "context", slug, "--peek"],
                                env={"HOME": str(home)}, capture_output=True, check=True)
        (folder / f"context-{label}.txt").write_bytes(result.stdout)
        (folder / f"stderr-{label}.txt").write_bytes(result.stderr)
        results[fixture][label] = {"bytes": len(result.stdout), "lines": len(result.stdout.splitlines())}
    executable.unlink()
(output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
print(json.dumps(results, indent=2))
