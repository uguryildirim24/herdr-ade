+++
verdict = "MERGE"
round = "r93"
candidate = "b2e16ec4c3e5bc8448f1d7a550137035c2042597"
manifest_hash = "3c35ab5b5295c2e22181094f338a2af19b938636ab137f1f62125d213db773ee"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

## t-0223

MERGE. The earlier reviewed candidate merged cleanly onto the r94 base without conflicts. The r94 cleanup change remains intact, and the machine-kind change is unchanged: Claude and agy jobs stay local without SSH readiness probes, pi lanes and reviewers can use `oci`, and doctor omits unsupported sign-in probes.

Missing `kinds` continues to preserve unrestricted declarations, while an explicit empty list permits no agent jobs. The install path still resolves the shipped declaration after re-exec and sees `kinds = ["pi"]`.

## Requested checks

- `cargo fmt --check` — exit 0; no output.
- `cargo test` — exit 0. Final suite lines: `625`, `57`, `82`, `8`, `6`, `2`, and `3` passed; 0 failed.
- `cargo clippy --all-targets -- -D warnings` — exit 0; `Finished dev profile [unoptimized + debuginfo] target(s) in 23.67s`.
- `git diff --check` — exit 0; no output.
