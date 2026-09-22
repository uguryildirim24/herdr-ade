+++
verdict = "MERGE"
round = "r111"
candidate = "a7334b3982608fe7090c450182729410de90981e"
manifest_hash = "2ce570425452c54ec3446c830e5aa2d963e39fd3d5583163152c276fb1268d67"
policy_hash = "b6e35aab70fe96c6aa8639d6ef111bbb6baaa60cbb2a2de9b5e8eef6e4ab3b1b"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check"]
+++

# Review

The earlier reviewed candidate was merged into the newer integration base carrying r110. I checked the combined `src/threads.rs`, `src/coordinator.rs`, `src/ticker.rs`, `src/doctor.rs`, and `skill/COORDINATOR.md` result. The r110 cleanup and task-state changes remain intact.

## t-0291 — MERGE

Context keeps command syntax in the coordinator skill, uses `ha` only at the default root, preserves failure evidence with current/unknown/resolved presentation, and suppresses unchanged work nudges. Stable recurring-check identities retain the literal failed command.

## t-0292 — MERGE

Box observations and reviewer attention use current courier evidence. A terminal box tab closes only when its recorded machine, workspace, tab, pane, and cwd match; the tab has exactly that pane; no agent owns the tab; and process-info verifies the same pane has no foreground process. Unowned shells remain advisory.

## t-0293 — MERGE WITH REVIEW FIXES

Deletion preflights every readable project and keeps repositories, normalized session-log names, and Pro lanes/files claimed by another project. It now also refuses before doing any deletion when the project-record folder overlaps another project's repository or recorded path, preventing the final project-folder trash from swallowing shared data.

Historical `.trash` cleanup removes only exact `<slug>-<timestamp>` copies, names retained entries, and removes the holding folder only when nothing else remains. I moved that cleanup before the project record is trashed, so any cleanup failure leaves the durable deletion intent available for retry.

Review fixes:

- `c88153c review(lifecycle): keep deletion retryable through old-trash cleanup`
- `a7334b3 review(lifecycle): refuse overlapping project records`

## t-0294 — MERGE

Rolf-facing board, notification, talk, say, and ask prose still receives the full vocabulary, identifier, name, sentence-length, and envelope checks. Internal thread/round sentences, task titles and conditions, and decision lines keep exact technical detail without a word cap and are wrapped or collapsed in technical views.

The old 25-word internal cap was removed because it discarded technical facts. The 80-character board cap remains because Herdr's metadata transport truncates board values there. The decision journal's 64 KiB serialized-entry limit remains a storage-integrity envelope, not a prose rule.

## t-0295 — MERGE

Starting-lane follow-ups stay attempt-bound and wait for the matching bootstrap receipt. Queued delivery is marked uncertain before transport; an ambiguous result remains uncertain, blocks later queue items, creates one reconciliation item, and is not sent again. Known pre-send refusals return the item to queued state.

## Gates

Run with `PATH=/bin:$PATH`, `DEVELOPER_DIR=/Library/Developer/CommandLineTools`, and `CARGO_TARGET_DIR=/home/ubuntu/.cache/herdr-ade-target-t0301`.

```text
$ cargo fmt --check
[exit 0; no output]
```

```text
$ cargo test
test result: ok. 669 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 49.64s
test result: ok. 54 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.91s
test result: ok. 87 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.84s
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.33s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.45s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.49s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

```text
$ cargo clippy --all-targets -- -D warnings
   Compiling herdr-ade v0.1.0 (/home/ubuntu/projects/herdr-ade/.worktrees/t-0301)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.06s
```

```text
$ git diff --check
[exit 0; no output]
```
