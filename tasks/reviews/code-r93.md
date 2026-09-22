+++
verdict = "MERGE"
round = "r93"
candidate = "f9b4efe00ab008a3f448f2bd9b9ef1ac776a4102"
manifest_hash = "3c35ab5b5295c2e22181094f338a2af19b938636ab137f1f62125d213db773ee"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

## t-0223

MERGE. The earlier reviewed candidate applies cleanly over the new r91 install ordering. Installation still builds and offers the new local binary for re-exec before saved-machine lookup. The later lookup resolves the saved `oci` profile to the shipped declaration, including `kinds = ["pi"]`; a regression assertion now checks that pi is allowed while Claude and agy are excluded.

The dispatch behavior remains intact: Claude and agy stay local without an SSH readiness probe, pi lanes and reviewers can use `oci`, doctor omits unsupported sign-in probes, omitted `kinds` preserves unrestricted existing declarations, and an explicit empty list allows no agent jobs.

## Requested checks

- `cargo fmt --check` — exit 0; no output.
- `cargo test` — exit 0. Final suite lines:
  - `622 passed; 0 failed`
  - `57 passed; 0 failed`
  - `82 passed; 0 failed`
  - integration suites: `8`, `6`, `2`, and `3` passed; 0 failed.
- `cargo clippy --all-targets -- -D warnings` — exit 0; `Finished dev profile`.
- `git diff --check` — exit 0; no output.
