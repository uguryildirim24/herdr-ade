+++
verdict = "MERGE"
round = "r106"
candidate = "524d313d40de0871e58ce0479bc3fc0e2ee2891c"
manifest_hash = "be9aa7a3d138ad04aebb7e5704582b969a1808febfda2f55c6fa23e4370b8a94"
policy_hash = "b0854bd6e2b014fabc7e40bfde1e4ee7fa636e894571a68daae3999272c7ef0e"
gates = []
+++

# Review r106

## t-0275

MERGE. `stage_done` checks the requested commit tree after confirming HEAD and refuses every tracked path below `.herdr-project/`, while ignored untracked runtime files do not appear in the tree and remain allowed. The refusal names all paths and the recovery steps. The five reports are removed from the shared tree.

## t-0276

MERGE. A resolved, uncancelled current attempt without `done` now derives `unknown` with the required retry-or-attest next step. Attestation accepts only a matching stored final copy, stores those exact bytes as the artifact, records the coordinator and reason, and uses a real remaining lane-folder HEAD or no SHA. Open, cancelled, complete, missing-copy, and hash-mismatch cases refuse. Old events load with no attestation. I fixed the eligibility read to fail closed on unreadable sealed evidence rather than silently skipping a possibly existing completion.

## t-0277

MERGE. Helper briefs retain PROJECT.md instructions, applicable dated instructions and memory notes, and task notes, but no longer inline `MEMORY.md` or `memory/*.md`. Task scope and replacement filtering remain in the shared note projection. Coordinator context still loads and renders legacy rows as undated, and the cap now measures only notes a helper brief can carry. The integrated callers contain no remaining `memory_index` path; `legacy_rows` remains only as the coordinator-facing loader.

## Requested checks

The round manifest lists no gates, so the front matter remains `gates = []`. I ran all four checks requested for this review on the candidate.

- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check` — exit 0; no output.
- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test` — exit 0. Final suite lines:
  ```text
  test result: ok. 651 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 36.43s
  test result: ok. 58 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.82s
  test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.84s
  test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.88s
  test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.17s
  test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
  test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
  ```
- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets -- -D warnings` — exit 0. Final line:
  ```text
      Finished `dev` profile [unoptimized + debuginfo] target(s) in 29.44s
  ```
- `git diff --check` — exit 0; no output.
