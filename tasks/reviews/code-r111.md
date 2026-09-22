+++
verdict = "MERGE"
round = "r111"
candidate = "0ca6277cc92d7cb32cacab1ebbfb4ea3e8bec745"
manifest_hash = "2ce570425452c54ec3446c830e5aa2d963e39fd3d5583163152c276fb1268d67"
policy_hash = "b6e35aab70fe96c6aa8639d6ef111bbb6baaa60cbb2a2de9b5e8eef6e4ab3b1b"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check"]
+++

# Review

## t-0291 — MERGE

The combined context keeps command syntax in one coordinator-skill location, uses `ha` only for the default root, distinguishes current, unknown and resolved failures without erasing evidence, and suppresses unchanged task nudges. Stable check identities retain the literal failed command as evidence.

## t-0292 — MERGE WITH REVIEW FIX

Fresh box observations and reviewer attention are derived from current courier evidence. A leftover box tab now closes only when its terminal thread record matches the exact machine, workspace, tab, pane and cwd; no agent owns the tab; the tab has exactly that one pane; and process-info returns that pane with no foreground process. Unowned shells remain advisory.

Review fix: `782fb99 review(ticker): close only exclusively owned empty tabs`.

## t-0293 — MERGE WITH REVIEW FIX

`ha delete` has one durable, retryable path and keeps GitHub deletion explicit. The review found that the lane could remove colliding session-log names, shared Pro files, nested/shared repositories, a GitHub repository used from another clone, and the whole historical `.trash` folder even when it held another project's retained copy.

The fix preflights every readable project record, preserves and names resources claimed elsewhere, treats nested repositories as shared, protects normalized session-name collisions, and removes only an exact `<slug>-<timestamp>` historical copy. The old holding folder is removed only when no other copy remains. The obsolete `--force` documentation was removed.

Review fix: `6b5e5cf review(lifecycle): preserve resources claimed by other projects`.

## t-0294 — MERGE

Rolf-facing prose still receives the full vocabulary, name, sentence and board checks. Technical Work, Decided for you and Tasks rows retain exact internal records and are visibly marked with their generated ids. The old 25-word internal prose cap was removed because it discarded technical facts while the screen already wraps or collapses technical rows. The 80-character board cap remains because the Herdr metadata transport truncates board tokens there; the decision journal's 64 KiB serialized-entry envelope remains a storage-integrity bound, not a prose rule.

## t-0295 — MERGE WITH REVIEW FIX

Queued messages are attempt-bound and ordered. The ticker marks a queued delivery uncertain before transport, stops at an uncertain item on every later pass, and creates one reconciliation item rather than resending it.

The review found a gap after brief transport but before the matching bootstrap receipt: `thread prompt` could send directly because `prompt_pending` was already false. It now queues every launched lane's follow-up until that exact attempt acknowledges bootstrap; adopted panes remain exempt because they have no launch receipt.

Review fix: `0ca6277 review(threads): queue follow-ups until bootstrap receipt`.

## Gates

Run with `PATH=/bin:$PATH`, `DEVELOPER_DIR=/Library/Developer/CommandLineTools`, and `CARGO_TARGET_DIR=/home/ubuntu/.cache/herdr-ade-target-t0297`.

```text
$ cargo fmt --check
[exit 0]
```

```text
$ cargo test
test result: ok. 668 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 36.55s
test result: ok. 58 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.81s
test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.83s
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.83s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.16s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

```text
$ cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.15s
```

```text
$ git diff --check
[exit 0]
```
