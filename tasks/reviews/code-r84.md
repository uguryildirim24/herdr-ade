+++
verdict = "MERGE"
round = "r84"
candidate = "f87c27892ec9bbc653505c6189015ac8cc670f2c"
manifest_hash = "bd64714da47fdf99035e8ab06288460b0911f69608c450c795b17dc3dbc897e5"
policy_hash = "d523603c07eaba1d3f8e4dbae733f1c5746755d35294ffa5fa08cfe24ff202b3"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check"]
+++

# Round r84 repair review

## Verdict

MERGE. The earlier reviewed candidate merges cleanly onto the r83 base. The coordinator skill retains W6's no-repository flow: the harness creates a project-owned git folder, commits the brief, and the lane commits its work and calls `ha done` from that folder. The no-workarounds rule appears once, in plain language, and still preserves `thread adopt` only for a verified process already belonging to the lane.

## t-0202

The lane correctly requires broken harness behavior to be fixed through a lane, round and `ha harness install`. It forbids hand-start-and-adopt substitutions, pane input used to force state, and project-specific hand-made repositories or files. It tells peer coordinators to wait for the install and requires a plain account to Rolf of what is blocked and what will unblock it.

The earlier review's PI correction remains intact: typed provider failures use bounded recovery on the same recipe, and explicit retry consumes that same bounded policy rather than introducing a manual recovery path.

## Repair-base integration

The r83 no-repository work and r84 skill text agree. `skill/COORDINATOR.md` says the managed folder receives the brief commit and follows the normal commit-and-done flow. `skill/LANE.md` tells every lane to work in its recorded git folder, commit the finished work, and pass that commit to `ha done`.

## Gates

Commands ran with `PATH=/bin:$PATH` and `DEVELOPER_DIR=/Library/Developer/CommandLineTools`.

### `cargo fmt --check`

Exit code: 0

```text
(no output)
```

### `cargo test`

Exit code: 0

```text
running 2 tests
test workflow_help_describes_routing_match ... ok
test coordinator_cannot_select_a_role_recipe_or_model ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
```

### `cargo clippy --all-targets -- -D warnings`

Exit code: 0

```text
    Checking ratatui-widgets v0.3.2
    Checking ratatui v0.30.2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.24s
```

### `git diff --check`

Exit code: 0

```text
(no output)
```
