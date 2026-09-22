+++
verdict = "MERGE"
round = "r97"
candidate = "ce57527c25da8f332dd4f71795a0af5b74f51dbf"
manifest_hash = "0529d1ff5cff1bcaf3a7aa46e84e7dfeb806e6a5ce13135b728c0567a09a1745"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

## t-0239

MERGE. The previously reviewed task-drop candidate merged without conflicts onto the W18 base. A dropped task remains terminal without completing its plan step, retains and displays its reason, receives no installation proof or ticker action, and cannot replace a task with verification evidence. Historical task records still load because drop evidence defaults when absent.

W18 remains intact: the merge did not change `src/pro/turn.rs`, including its nested `task_complete.error.message` handling and timestamp-paired packet-load handling. The full test suite covers both bodies of work and passes.

Checks:

- `cargo fmt --check` — exit 0.
- `cargo test` — the first run exited 101 on `installing_the_same_clean_commit_keeps_every_installed_inode`; that unrelated test immediately passed alone, and the exact full rerun exited 0: 634 main, 57 pi, 83 Pro, 8 CLI, 6 actionable-context, 2 context-record, and 3 routing tests passed with no failures.
- `cargo clippy --all-targets -- -D warnings` — exit 0.
- `git diff --check` — exit 0.
