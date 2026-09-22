+++
verdict = "MERGE"
round = "r97"
candidate = "f54303e3dd7d5f7f14261d5b87e24702499575ea"
manifest_hash = "0529d1ff5cff1bcaf3a7aa46e84e7dfeb806e6a5ce13135b728c0567a09a1745"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

## t-0239

MERGE. The earlier drop-task candidate is integrated with the new W14 base. Dropped tasks remain terminal, retain and display their reason, stay out of plan completion, installation proof, and ticker work, and cannot replace a task that already has verification evidence. W14 still preserves verified state after a later install whose process proof is unavailable and labels that uncertainty without erasing the earlier verification.

The merge needed a direct resolution where both changes render task status. The result keeps W14's status wording in both generated `TASKS.md` and task output. I also added the missing serde default for drop evidence so task records written before this feature still load.

Requested checks passed: `cargo fmt --check`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `git diff --check`.
