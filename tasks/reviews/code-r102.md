+++
verdict = "MERGE"
round = "r102"
candidate = "0ec88a01d89f66e0be3105e55e15ab0da5a376d2"
manifest_hash = "379da6e7069d067d00bf69fd712f6057deaaedb3894d37e32433cb70e135a3a1"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

# Review

## t-0260

MERGE. I found no blocking defect.

The compiled guard is now compared byte-for-byte, and harness installation refreshes that guard locally and on the configured box after installing the plugin. The two reported provider errors enter escalation as typed provider failures and consume only the bounded same-recipe recovery budget.

A coordinator-requested retry of an unknown failure clones the prior launch, increments the same-recipe counter, records `coordinator-retry` with the supplied reason, and reuses the task and worktree without selecting a fallback. This also works for a bound reviewer thread: `thread retry` is role-independent and preserves the reviewer role and round binding. `round retry` deliberately continues through the automatic path and therefore still refuses unknown evidence.

The pane-input path is harness-owned rather than a coordinator workaround. In the shipped declarations only pi enables it, and it is reached only when Herdr reports the agent blocked and the thread has a durable recorded error. A blocked approval or question with no recorded error is refused before any pane input is sent.

Requested checks all exited 0: `cargo fmt --check`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `git diff --check`.
