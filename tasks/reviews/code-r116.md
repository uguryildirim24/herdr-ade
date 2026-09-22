+++
verdict = "MERGE"
round = "r116"
candidate = "4f587b1e049876cc638293f17caeccc45242781e"
manifest_hash = "7e7fd74fa3f579f3d8d1da20673cf36b2ed2470f1b549d9e0580365d749e7016"
policy_hash = "7d013b92516df0434273f0f2ae57e9ec661dd6ba01b7b9a588a327b3cc724425"
gates = [{ command = "cargo fmt --check", exit = 0 }, { command = "cargo test", exit = 0 }, { command = "cargo clippy --all-targets -- -D warnings", exit = 0 }, { command = "git diff --check", exit = 0 }]
+++

# Review

## t-0317

MERGE. Admission now distinguishes a real manifest change from an idempotent admission, durably unbinds any reviewer only for a real change, and then cancels that reviewer outside the project lock. Removal follows the same ordering. The new tests exercise both active-review paths and confirm the superseded records become resolved. The existing cancellation path closes the associated process and clears its display tokens while retaining recoverable cleanup state when external cleanup cannot finish.

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
test coordinator_cannot_select_a_role_or_model ... ok
test workflow_help_describes_routing_match ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

The main suite also reported `688 passed; 0 failed`; the remaining binary and integration suites passed.

### `cargo clippy --all-targets -- -D warnings`

Exit 0. Last output:

```text
    Checking ratatui-widgets v0.3.2
    Checking ratatui v0.30.2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 35.14s
```

### `git diff --check`

Exit 0. Last output:

```text
(no output)
```
