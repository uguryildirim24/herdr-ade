+++
verdict = "MERGE"
round = "r116"
candidate = "73fa92e2ad6c462fe9fa5ecc6d15b7eb2a20b26d"
manifest_hash = "91b8ef130aed8f6e9ad3bf4e78de8e932efec1d6851e9a4c4980759a007c430a"
policy_hash = "7d013b92516df0434273f0f2ae57e9ec661dd6ba01b7b9a588a327b3cc724425"
gates = [{ command = "cargo fmt --check", exit = 0 }, { command = "cargo test", exit = 0 }, { command = "cargo clippy --all-targets -- -D warnings", exit = 0 }, { command = "git diff --check", exit = 0 }]
+++

# Review

## t-0317

MERGE. Admission and removal first durably return the round to admission, detach the old reviewer, and then cancel that reviewer outside the project lock while the round operation remains serialized. A reviewer whose launch overlaps that transition is rejected by the final phase and frozen-revision check, then resolved through the normal cancellation path.

The follow-up changes preserve the completion visible before delivery as the barrier. They accept an immediate later completion, carry an older-attempt barrier across a retry, and no longer restore an earlier attempt's pin after the replacement attempt goes idle. The shared stale-reviewer cleanup covers both a newly started reviewer and an unbound reviewer recovered after an interrupted start.

No review fix was needed.

## Gates

### `cargo fmt --check`

Exit 0. Last output:

```text
(no output)
```

### `cargo test`

Exit 0. Last output:

```text
running 3 tests
test an_exact_recipe_requires_rolfs_quoted_words ... ok
test workflow_help_describes_routing_match ... ok
test coordinator_cannot_select_a_role_or_model ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

The main suite reported `688 passed; 0 failed`; all binary and integration suites also passed.

### `cargo clippy --all-targets -- -D warnings`

Exit 0. Last output:

```text
    Checking serde_json v1.0.151
    Checking toml v0.9.12+spec-1.1.0
   Compiling darling v0.24.1
   Compiling instability v0.3.13
    Checking ratatui-widgets v0.3.2
    Checking ratatui-crossterm v0.1.2
    Checking ratatui v0.30.2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 31.72s
```

### `git diff --check`

Exit 0. Last output:

```text
(no output)
```
