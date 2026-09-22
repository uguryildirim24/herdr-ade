+++
verdict = "MERGE"
round = "r107"
candidate = "564d3d8c7048eb37e02f9cb1eb51d7f97f891586"
manifest_hash = "829836b2b58f40d92099ba20c53afbe042301dcfe902689d8e80374f3d0536ac"
policy_hash = "b0854bd6e2b014fabc7e40bfde1e4ee7fa636e894571a68daae3999272c7ef0e"
gates = []
+++

# Review r107

## t-0279

MERGE. Individual acceptance conditions can be withdrawn with dated reasons. Withdrawn conditions are excluded from state derivation and remaining-verification counts, and the task becomes terminal when every live condition is verified, so the ticker stops nudging it. Show, generated `TASKS.md`, and the talk overview include the withdrawal. Old records without the field still load and derive normally. Verified, repeated, out-of-range, and all-condition withdrawals refuse, as does later verification of a withdrawn condition. Whole-task drop and the installed/running and harness-install paths remain intact.

I fixed a race in which verification could pass its withdrawal check before waiting for the task lock and then be written after a concurrent withdrawal. The check now runs against the locked current record, and record validation rejects contradictory persisted withdrawal and verification evidence. I also added direct coverage for the terminal ticker behavior and the evidence refusal.

## Requested checks

The round manifest lists no gates, so the front matter remains `gates = []`. I ran all four checks requested for this review on the candidate.

- `cargo fmt --check` — exit 0; no output.
- `cargo test` — exit 0. Final suite lines:
  ```text
  test result: ok. 654 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 37.61s
  test result: ok. 58 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.82s
  test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.84s
  test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.89s
  test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.19s
  test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s
  test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
  ```
- `cargo clippy --all-targets -- -D warnings` — exit 0. Final line:
  ```text
      Finished `dev` profile [unoptimized + debuginfo] target(s) in 28.08s
  ```
- `git diff --check` — exit 0; no output.
