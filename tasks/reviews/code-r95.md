+++
verdict = "MERGE"
round = "r95"
candidate = "feb103b6b33a447db7f7a7dd23427d363036e72c"
manifest_hash = "713684cbfb97b73fce47aa9589c7ee15f4c3ea5099d462e6086bbc3a3e30e3a5"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check"]
+++

# Repair review

MERGE. The earlier reviewed D7 candidate merged cleanly over the r93 base. The three paths changed on both sides (`README.md`, `docs/operations.md`, and `src/threads.rs`) retain both changes.

D7 still records provenanced notes, applies replacements across notes, decisions, task notes and tasks, removes replaced rows from current views and briefs, and links the stable task before composing the first brief. Its replacement and brief tests pass.

W15 still limits the shipped `oci` machine to the `pi` adapter kind. Claude and agy therefore remain on the Mac without box readiness or sign-in probes, while pi lanes and reviewers can still use the box. Its machine-kind, placement, and install tests pass.

All requested gates pass.
