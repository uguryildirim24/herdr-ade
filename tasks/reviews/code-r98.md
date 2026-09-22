+++
verdict = "MERGE"
round = "r98"
candidate = "66ed9939ebb0650b489f266ea684c21f43bd91ba"
manifest_hash = "112e7a3b6a6a173dbc29e05d81aa0a6a08f0c504dec4976ce764b57228ce11c7"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check"]
+++

## t-0238

The pinned change makes artifact-backed `.reports/` disposable without weakening protection for other ignored data or nested checkouts. The same inspection applies to local and box lanes, while closed-round review cleanup checks the reviewer's artifact. Remote resolve and cancel remove build output independently, including a repeated resolve of an already-resolved lane. No review fix was needed, and all requested gates passed.
