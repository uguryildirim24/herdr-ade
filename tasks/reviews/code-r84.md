+++
verdict = "MERGE"
round = "r84"
candidate = "80a5f2d68dd5416a778e6f05001a6dd1a3416e9e"
manifest_hash = "bd64714da47fdf99035e8ab06288460b0911f69608c450c795b17dc3dbc897e5"
policy_hash = "d523603c07eaba1d3f8e4dbae733f1c5746755d35294ffa5fa08cfe24ff202b3"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check"]
+++

# Round r84 review

## Verdict

MERGE. The coordinator skill states the no-workaround rule once and plainly. It preserves `thread adopt` only for a verified process already belonging to the lane. COORDINATOR, LANE, PICKUP, REVIEWER, PI, CRITIC and DRAFTER contain no manual recovery path around a broken start or blocked pane.

## t-0202

The lane correctly routes broken harness behavior through a lane, round and `ha harness install`, tells peer coordinators to wait for that install, and requires a plain account to Rolf of what is blocked and what fixes it.

I corrected PI's provider-recovery description in candidate commit `80a5f2d68dd5416a778e6f05001a6dd1a3416e9e`. The guard seals a typed provider-failure event and bounded recovery runs automatically. An explicit `thread retry` also keeps the same recipe, consumes the next bounded retry, and refuses after exhaustion; it does not select a fallback.

## Gates

All commands used `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools` where requested.

### `cargo fmt --check`

Exit code: 0

```text
EXIT_CODE=0
```

### `cargo test`

Exit code: 0

```text
running 2 tests
test workflow_help_describes_routing_match ... ok
test coordinator_cannot_select_a_role_recipe_or_model ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
EXIT_CODE=0
```

### `cargo clippy --all-targets -- -D warnings`

Exit code: 0

```text
    Checking ratatui v0.30.2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.96s
EXIT_CODE=0
```

### `git diff --check`

Exit code: 0

```text
EXIT_CODE=0
```
