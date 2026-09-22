+++
verdict = "MERGE"
round = "r99"
candidate = "be550d679122d655fdb91307a2b990a64fd60067"
manifest_hash = "c9c7725ea179599cad1819955feb29a58f3980e15ab2ad3ba566a09b92ccaba9"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check"]
+++

# W25 repair review

MERGE. The earlier reviewed candidate merged cleanly onto the r97 base without conflicts. Both W21 and W25 are ancestors of the candidate.

W25 still rebuilds the box-local Git index from `HEAD` and refreshes it before reading `source_head`, checking `source_dirty`, or running `cargo build`. Its ordering test still checks both generated repository build scripts. W21's task-drop changes do not overlap this behavior.

## Gates

- `cargo fmt --check` — exit 0; no output.
- `cargo test` — exit 0. Final suite: `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s`. Main suites also passed: 634 library, 57 herdr-pi, and 83 herdr-pro tests.
- `cargo clippy --all-targets -- -D warnings` — exit 0. Final line: `Finished dev profile [unoptimized + debuginfo] target(s) in 23.76s`.
- `git diff --check` — exit 0; no output.
