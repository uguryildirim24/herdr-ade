+++
verdict = "MERGE"
round = "r26"
candidate = "cf82556a95c7249a60606a114e401c796f3e2bf0"
manifest_hash = "3a59cb0ea225b0e3257c4eb41212d46c51e883439aa54e24c50987b1a638dd79"
policy_hash = "941e9ea13a16368ca1ee73b91e4a1dc137816aed5c9c2f6e7e6acea90d78dfbd"
gates = ["PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check", "PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test --locked", "PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets --locked -- -D warnings"]
+++

# Review

MERGE.

## t-0055

The pinned lane now reads each box once through the courier, re-links a live box agent with the machine-qualified parent, prints machine-routed recovery lines for a gone lane, and supports project-wide pickup plus safety-gated restart.

The review fixed five recovery defects before accepting it: `--all` now uses each project's own coordinator pane, paused projects stay stopped, a box shell whose agent is gone is treated as gone, a fresh box lane cannot silently lose its parent write, and the CLI refuses an ambiguous project-plus-`--all` invocation. The added regression cases cover the first three.

The machine-qualified token will become visible as cross-machine nesting when the fork change described by t-0053 lands; writing that token now is the intended interface.

## Integration update

Round r24 was merged into the candidate. Its default-placement paragraph precedes the pickup paragraph in `docs/operations.md`. The merged r24 reachability test was also corrected to match the fixed-PATH SSH probe it exercises.

## Gates

```text
$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check
(no output)

$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test --locked
running 4 tests
...
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets --locked -- -D warnings
Compiling herdr-ade v0.1.0 (...)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.86s
```

The four test binaries passed 403, 56, 78 and 4 tests: 541 total.
