+++
verdict = "MERGE"
round = "r120"
candidate = "73a3191bd5775b48b66bef69d06e1db6a38f1f0c"
manifest_hash = "be8f737512311dbc4ca3d7407a41d71571512102c761a20d8320487043629e2a"
policy_hash = "7d013b92516df0434273f0f2ae57e9ec661dd6ba01b7b9a588a327b3cc724425"
gates = [{ command = "cargo fmt --check", exit = 0 }, { command = "cargo test", exit = 0 }, { command = "cargo clippy --all-targets -- -D warnings", exit = 0 }, { command = "git diff --check", exit = 0 }]
+++

## t-0325

The lane makes coordinator support depend on a real prompt-submit hook, rebuilds every open coordinator hook after an install, surfaces unreadable bindings in the failure ledger, and stores request-backed project recipe choices for relaunch. I fixed one review defect: moving a coordinator from a dead session with `open --rebind` discarded that stored recipe. Rebinding now carries the exact launch, arguments, and request authority into the replacement coordinator.

All pinned gates pass on candidate `73a3191bd5775b48b66bef69d06e1db6a38f1f0c`.
