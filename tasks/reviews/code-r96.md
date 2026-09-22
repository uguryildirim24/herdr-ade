+++
verdict = "MERGE"
round = "r96"
candidate = "c25def4e25814b6c4adf19382b17d250f9f9a6c2"
manifest_hash = "a73732ad79270252c5e10d90da53881483937bd898750b3905c7cf47aa660b7d"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

## t-0235

MERGE. The earlier reviewed candidate merged onto the revised W14 base without conflicts. Pro now preserves `task_complete.payload.error.message`, treats page and stopped-thinking errors as provider failures, keeps rate limits on the cooldown path, and uses rollout timestamps to skip the short packet-load completion even when the real answer turn starts after the first settle poll.

W14 still keeps a repeated same-commit install on the existing inode, reports exact same-commit ticker builds as current, preserves earlier verification when a later process check is unknown, and marks dirty build identities. The focused regression tests for both W18 and W14 passed within the full suite.

Checks passed: `cargo fmt --check`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `git diff --check`.
