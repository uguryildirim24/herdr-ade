+++
verdict = "MERGE"
round = "r99"
candidate = "0a440989e554019f619f013737c7f4ff86e5a48a"
manifest_hash = "c9c7725ea179599cad1819955feb29a58f3980e15ab2ad3ba566a09b92ccaba9"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

# W25 repair review

MERGE. The earlier reviewed candidate merged cleanly onto the revised base. The intervening W18 change is confined to `src/pro/turn.rs`; it does not overlap or alter W25.

Both generated box repository scripts still rebuild the box-local index from `HEAD` and refresh its stat information before reading `source_head`, checking `source_dirty`, or running `cargo build`. This repairs an index omitted by sync without changing the working tree or refs, and the ordering test still covers both the plugin and fork build scripts.

Requested checks passed: `cargo fmt --check`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `git diff --check` (all exit 0).
