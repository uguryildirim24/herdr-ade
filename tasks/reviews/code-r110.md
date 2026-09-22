+++
verdict = "MERGE"
round = "r110"
candidate = "45d7fbd92c7de894d70acc247c72258ad93c3249"
manifest_hash = "883a924e9c47691b0476ec052ede28ca1c8047bf2d0c1dc193a263d693e9010b"
policy_hash = "b6e35aab70fe96c6aa8639d6ef111bbb6baaa60cbb2a2de9b5e8eef6e4ab3b1b"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check"]
+++

# Review r110

MERGE.

- **t-0286:** Task milestones are derived conservatively from sealed attempt commits. An unfinished attempt or any sealed SHA different from its base keeps every configured code milestone; no-attempt and unchanged report-only tasks can proceed to verification. The CLI and coordinator guide agree on the simplified evidence command.
- **t-0287:** ADE configuration reads go through the shared document boundary, while each caller decodes only its own view. Recipe defaults and overrides drive launch and both doctor surfaces. A bad section becomes a failed doctor row without suppressing independent checks.
- **t-0289:** Round merge and cancellation resolve their lanes and reviewer through the guarded final-copy path. Dirty worktrees and kept ignored data are not removed, and cleanup failure cannot roll back a completed merge. I fixed a crash gap: a durable round-level marker now lets the ticker reconcile members if the process dies after closing the round but before writing a member's cleanup marker.
- **t-0290:** Live automated prompt markers retain pane and text, remove all matching automated fragments, and preserve only Rolf's remainder. The same projection hides historical ticker suffixes from conversation, request lookup, citations, and decision authority without rewriting the journal.

## Gates

`cargo fmt --check`

```text
(no output; exit 0)
```

`cargo test`

```text
test result: ok. 656 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 36.28s
test result: ok. 54 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.82s
test result: ok. 87 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.83s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.15s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.17s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

`cargo clippy --all-targets -- -D warnings`

```text
    Checking ratatui v0.30.2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 26.51s
```

`git diff --check`

```text
(no output; exit 0)
```
