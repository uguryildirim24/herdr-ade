+++
verdict = "MERGE"
round = "r115"
candidate = "068cca42fbf8625bf117e3e48d3f1c98ff68dd45"
manifest_hash = "002c193ef9bad5f90d611df1ff3b2bf63bd50a5ee69f4ea2a58daf5f41e00277"
policy_hash = "7d013b92516df0434273f0f2ae57e9ec661dd6ba01b7b9a588a327b3cc724425"
gates = [{ command = "cargo fmt --check", exit = 0 }, { command = "cargo test", exit = 0 }, { command = "cargo clippy --all-targets -- -D warnings", exit = 0 }, { command = "git diff --check", exit = 0 }]
+++

# Review r115

## t-0313

MERGE. Follow-ups now remove a finished lane's pin before another review can start, stop a superseded reviewer, and automatically create the next review after a newer completion. The durable event barrier preserves historical records and prevents the pre-follow-up completion from being mistaken for the requested report. A question-only follow-up can restore the prior pin after the lane is observed idle, so it does not leave the round blocked indefinitely. The review found no code changes to make.

## Gates

### `cargo fmt --check`

Exit 0. No output.

### `cargo test`

Exit 0. Last lines:

```text
running 3 tests
test an_exact_recipe_requires_rolfs_quoted_words ... ok
test workflow_help_describes_routing_match ... ok
test coordinator_cannot_select_a_role_or_model ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
```

The preceding test binaries also passed, including 686 library tests, 54 `herdr-pi` tests, 87 `herdr-pro` tests, and all integration suites.

### `cargo clippy --all-targets -- -D warnings`

Exit 0. Last lines:

```text
    Checking toml v0.9.12+spec-1.1.0
    Checking ratatui-crossterm v0.1.2
    Checking ratatui-widgets v0.3.2
    Checking ratatui v0.30.2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 31.91s
```

### `git diff --check`

Exit 0. No output.
