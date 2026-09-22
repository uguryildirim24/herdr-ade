+++
verdict = "MERGE"
round = "r109"
candidate = "3199b43c78432f2b9c61647f9e97fdea8915f2a2"
manifest_hash = "11c7f373c576eb0dab7b5b30f86ebcd92565f8d71d1fb35bcc73654d272b82bf"
policy_hash = "b0854bd6e2b014fabc7e40bfde1e4ee7fa636e894571a68daae3999272c7ef0e"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check"]
+++

# Review

## t-0284 — MERGE

The prompt hook now excludes a prompt only when the complete prompt is one or more Claude idle notices. Historical notice rows remain readable but are omitted from requests and cannot authorize a decision. The same classification also keeps them out of the talk view and coordinator context.

I tightened the existing exact-notice test to prove that Rolf's words before a notice, after a notice, or between two notices are each recorded as his request. The implementation passed all review gates.

## Gates

Run with `PATH=/bin:$PATH`, `DEVELOPER_DIR=/Library/Developer/CommandLineTools`, and `CARGO_TARGET_DIR=/home/ubuntu/build/lanes/adeherdr-t-0285`.

```text
$ cargo fmt --check
[exit 0]
```

```text
$ cargo test
running 3 tests
test an_exact_recipe_requires_rolfs_quoted_words ... ok
test workflow_help_describes_routing_match ... ok
test coordinator_cannot_select_a_role_or_model ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

The suite summaries were 656, 58, 86, 10, 6, 2, and 3 passing tests, with no failures.

```text
$ cargo clippy --all-targets -- -D warnings
    Checking ratatui-crossterm v0.1.2
    Checking ratatui-widgets v0.3.2
    Checking ratatui v0.30.2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 24.55s
```

```text
$ git diff --check
[exit 0]
```
