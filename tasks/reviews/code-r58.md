+++
verdict = "MERGE"
round = "r58"
candidate = "973426fa020db314c7c067db784d4042cc99a8fe"
manifest_hash = "0bb401183a4f628a8f6dab66744cc91cebf19865c847d8ca7b200b853e77cd7b"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++

# Round r58 review

MERGE after review fixes.

## t-0128

The lane removes the integration-branch reservation, permits overlapping round reviews, gives every repair revision a new brief commit on its current base, and automatically starts the moved-base repair reviewer. The pinned lane commit and brief commit are ancestors of candidate C.

I fixed two recovery gaps in `review(rounds): preserve merge turns and repair recovery`:

- A merge now keeps the repository lock from the checked integration head through both ref effects, verdict V and checkpoint H. A durable unfinished merge also owns the merge turn, so a second round cannot start a merge transaction that strands the first round's crash recovery. The overlap regression now checks this refusal before resuming the first merge.
- An old pending repair `ReviewIntent` remains executable after upgrade. The old intent finishes its recorded branch-at-current-head output, while newly created repairs continue to write a new B. A regression covers that persisted legacy state.

Opening, admission, and review remain independent; only merge transactions on the same repository branch take turns. The automatic repair sequence and exact verdict validation remain covered.

## Gates and checks

The brief lists no project gates, so the verdict's `gates` array is empty. I additionally ran these checks on candidate C:

```text
$ PATH=/bin:$PATH cargo fmt --check
exit=0

$ PATH=/bin:$PATH cargo test
running 3 tests
test workflow_help_describes_policy_floor ... ok
test shipped_cases_run_offline_and_report_both_error_directions ... ok
test coordinator_cannot_select_a_role_recipe_or_model ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
exit=0

$ PATH=/bin:$PATH cargo clippy --all-targets -- -D warnings
   Compiling herdr-ade v0.1.0 (/home/ubuntu/projects/herdr-ade/.worktrees/t-0136)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.41s
exit=0

$ git diff --check
exit=0
```

The full `cargo test` run passed 534 main-binary tests, 55 pi tests, 78 Pro tests, 6 CLI tests, 8 actionable-context tests, 2 record-context tests, and 3 routing CLI tests.
