+++
verdict = "MERGE"
round = "r108"
candidate = "9b8f1aa0fa66af71ecc5841efbc65e7a2194e140"
manifest_hash = "e8ef796f195728ca119f18a26bc4d3b18e020b172e4e9a8bd01f2e07f1efdbcf"
policy_hash = "b0854bd6e2b014fabc7e40bfde1e4ee7fa636e894571a68daae3999272c7ef0e"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check"]
+++

# Review

## t-0281

MERGE. Whole cross-session messages and raw or paste-wrapped ticker prompts are excluded at the hook. Native text around a cross-session wrapper remains Rolf's request. Historical automated rows remain in the journal but cannot appear as recent requests or context, render in the talk view, authorize a recipe quote, or satisfy a `request:<id>` decision basis.

The hook's historical fallback also rejects a paste-wrapped `DONE t-NNNN <artifact> <sha>` without relying on a pending marker. I changed its test to exercise that unmarked path and assert that no journal row is written. I also covered raw ticker prompts and native text around a cross-session wrapper.

All requested gates passed.
