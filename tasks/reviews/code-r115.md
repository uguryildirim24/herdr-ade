+++
verdict = "MERGE"
round = "r115"
candidate = "042d0e09b20a29400cc2810714c613bc40ac0c8d"
manifest_hash = "8319e62641038dd54ddfc52a9f4044e2f7ad14565b3315bc5907c109eff6a2d2"
policy_hash = "7d013b92516df0434273f0f2ae57e9ec661dd6ba01b7b9a588a327b3cc724425"
gates = [{ command = "cargo fmt --check", exit = 0 }, { command = "cargo test", exit = 0 }, { command = "cargo clippy --all-targets -- -D warnings", exit = 0 }, { command = "git diff --check", exit = 0 }]
+++

# Verdict

MERGE.

## t-0314

Machine-owned records move under `.state` while historical records remain readable. Review fixes keep remote box project creation at the real project root and make the courier read hidden events, receipts, and artifacts with historical fallback.

## t-0313

A follow-up holds each affected round, stops an active reviewer, and permits automatic re-review only after the lane is ready again. Review fixes bind the hold to completion evidence captured before prompt delivery and carry the barrier safely across a lane retry.

All pinned gates pass on candidate `042d0e09b20a29400cc2810714c613bc40ac0c8d`.
