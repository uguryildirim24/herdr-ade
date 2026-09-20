+++
verdict = "MERGE"
round = "r35"
candidate = "7d32a0bbef3efac55156cb4be489c87b79743e80"
manifest_hash = "eac0968bab41bcf3b2675d48144b9c61664ddcd4cf24b88227f38559ee3b23c4"
policy_hash = "3401228a1791aa7e7a698c29f1126da65d46d4a840c3c529b226b4d737856174"
gates = []
+++

# Round r35 review

## t-0077 — cost, out-of-date parts and tasks on the screen

MERGE after two review fixes.

The lane's task projection reads the same `TASKS.md` as the coordinator, keeps each overview row clipped, groups open tasks by heading, and shows the delegated thread's workflow word.

The original cost projection had three correctness gaps: a box lane with no local session showed zero minutes instead of measured elapsed time; the shared Pro usage file was charged to every project; and all-time lane totals omitted minutes. The review now measures elapsed time when usage is absent, stops at a lane's recorded done or gone time, uses real local-day boundaries, and attributes old Pro sends only when their full agent name identifies this project. Unknown money remains explicitly unknown.

The original client check compared the installed CLI with itself at version `0.9.1`, so it could not detect an older open window from another fork build. The review compares the open client's process start with the installed binary instead. It also adds the omitted lane-brief comparison, refreshes the recorded skill hash on an explicit lane restart, and bounds the box probe so an unreachable machine cannot hold the screen for the full general command timeout. Unreadable and pre-recording skill sources still produce no guessed stale claim.

The task's SPEC-talk amendment belongs in the fork and is not present in this repository. The lane report contains the exact amendment for the coordinator to apply there.

## Gates

The round listed no project gates, so the verdict's gate list is empty.

Additional validation on candidate `7d32a0bbef3efac55156cb4be489c87b79743e80`:

```text
$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check
(no output; exit 0)

$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets --locked -- -D warnings
   Compiling herdr-ade v0.1.0 (/home/agent/projects/herdr-ade/.worktrees/t-0084)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.29s

$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test --locked
     Running tests/cli.rs (target/debug/deps/cli-27bc6b59d6c059e2)
running 4 tests
....
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.38s
```

The full test command also passed 455 main tests, 56 `herdr-pi` tests and 78 `herdr-pro` tests.
