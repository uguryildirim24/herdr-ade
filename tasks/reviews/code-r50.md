+++
verdict = "MERGE"
round = "r50"
candidate = "41af07671e40ff5e88d96d81abc8a14a6441d308"
manifest_hash = "8eb29a587e0fb9c5df16aff55d460e8b6835e3899023e5c1e7fcd43410331324"
policy_hash = "518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d"
gates = []
+++

# r50 review

MERGE after one review fix. Reviewed and tested on `oci`.

## t-0108

Merged pinned SHA `47002d8f9234d828db85fcb8b1ed30bb8cd04ad6`.
Optional branch/file queries now return absence through successful Git output;
subprocess errors still record. The typed answer contract preserves failures for
missing executables, signals and timeouts. Closed-round and adjacent abandon
refusals carry `DesignedRefusal`. The subprocess audit explicitly retains mixed
probes, including ancestry, rather than hiding genuine failures. Existing ledger
history is untouched.

Review fix `41af076`: the new helper reused an arbitrary-ref normalizer, so
`refs/tags/release` could satisfy a local integration-branch query. It now always
queries `refs/heads/<branch>`, preserving the previous callers' namespace.
A regression test failed before the fix and passes after it, also checking that
fully qualified branch aliases cannot silently change the short-name contract.
No outstanding blocking findings.

## Checks

The brief lists no required gates (`gates = []`). Independently reran the lane's
checks on the final candidate, with
`PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools` and
`CARGO_TARGET_DIR=/home/ubuntu/build/lanes/adeherdr-t-0109`:

- `cargo fmt --check` — exit 0, no output.
- `cargo test` — exit 0; 661 passed, zero failed/ignored across seven suites.
  Last lines:
  ```text
  test coordinator_cannot_select_a_role_recipe_or_model ... ok
  test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
  ```
- `cargo clippy --all-targets -- -D warnings` — exit 0. Last line:
  ```text
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.35s
  ```
- `git diff --check` — exit 0, no output.

Before the fix, `cargo test absent_branch_is_an_answer_but_a_broken_repository_is_not`
exited 101 with `0 passed; 1 failed`, confirming the namespace regression.
The final full suite includes that passing regression plus negative-answer,
missing-tool, timeout/signal, invalid repository/revision and closed-round tests.

The cloud mirror has no canonical `.state/rounds/r50.toml`: `ha round show adeherdr r50`
returned `round_manifest_unavailable`. The hashes above are from the committed
brief, not an independently verified live manifest; the coordinator's merge
validator must check them against its canonical record.
