+++
verdict = "MERGE"
round = "r26"
candidate = "1d4f51ef918cb70b0efe650be2229f1c0d3aef03"
manifest_hash = "3a59cb0ea225b0e3257c4eb41212d46c51e883439aa54e24c50987b1a638dd79"
policy_hash = "941e9ea13a16368ca1ee73b91e4a1dc137816aed5c9c2f6e7e6acea90d78dfbd"
gates = []
+++

# Review

MERGE.

## t-0055

The pinned lane now reads each box once through the courier, re-links a live box agent with the machine-qualified parent, prints machine-routed recovery lines for a gone lane, and supports project-wide pickup plus safety-gated restart.

The review fixed five recovery defects before accepting it: `--all` now uses each project's own coordinator pane, paused projects stay stopped, a box shell whose agent is gone is treated as gone, a fresh box lane cannot silently lose its parent write, and the CLI refuses an ambiguous project-plus-`--all` invocation. The added regression cases cover the first three.

The machine-qualified token will become visible as cross-machine nesting when the fork change described by t-0053 lands; writing that token now is the intended interface.

## Gates

The brief listed no project gates, so `gates` is empty.

Supplemental review checks:

```text
$ cargo test --locked checkpoint::tests::pickup_ -- --nocapture
running 6 tests
...
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 389 filtered out

$ cargo clippy --all-targets --locked -- -D warnings
Checking herdr-ade v0.1.0 (...)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.84s
```
