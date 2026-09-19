+++
verdict = "MERGE"
round = "r28"
candidate = "3a782afe0771bc63f1fcaa67e0342df4084c746e"
manifest_hash = "54f7ca82c1a864d7d71f4da5abc8a6a854c118ec406ae718b7d15be3d1e704ce"
policy_hash = "f4bdef93095754d81dd5c733ba71b0b7c4062430d6aadef443eb1fd8c3f8ba2e"
gates = []
+++

# Round r28 review

## t-0058

MERGE. A rejected round now starts and binds its next reviewer after `round review` creates the next revision. A failed reviewer start is retried on a later pass regardless of the earlier failure announcement. Re-review tasks name the previous verdict, review file, and branch. A bound reviewer that is gone is still reported rather than replaced.

I fixed one manifest-safety defect before accepting the lane. A retry previously reused any existing review branch even when a lane had restarted and its pin was missing or had changed. It could therefore start a reviewer against a stale brief. The retry now waits until every lane is pinned and re-runs `round review` when the frozen revision or manifest hash is stale. The added regression test covers the missing-pin wait and the fresh review revision after the replacement completion arrives.

## Gates

The round brief lists no project gates. I also ran the three checks required by the reviewer task.

```text
PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check
(exit 0; no output)
```

```text
PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test --locked
running 4 tests
test ticker_start_without_projects_creates_nothing ... ok
test path_like_names_and_slugs_are_refused ... ok
test context_prints_a_usable_prefix_in_a_scrubbed_environment ... ok
test peek_records_nothing_and_context_records_seen_items ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.26s
```

The same command's unit-test groups also passed: 407 main, 56 `herdr-pi`, and 78 `herdr-pro` tests.

```text
PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets --locked -- -D warnings
    Checking serde v1.0.229
    Checking serde_json v1.0.151
    Checking toml v0.9.12+spec-1.1.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.72s
```
