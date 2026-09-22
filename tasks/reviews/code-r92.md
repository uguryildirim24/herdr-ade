+++
verdict = "MERGE"
round = "r92"
candidate = "3c19f639b9b75f885178db8779fe4f7e2edd8aaa"
manifest_hash = "f17bac1fbdb5a5af50147fe10d6e98811b15518661183a2180441fb05752aa05"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

# Round r92 review

Verdict: **MERGE**.

## t-0222

The box proof retries partial lock snapshots and cannot turn a missing build or PID into an empty stale row. Verified task evidence remains terminal after a later process check is unknown, while the latest uncertainty stays visible.

The review fixed two gaps in the lane. Install records now preserve the inode of every clean, identical commit for `herdr-ade`, `herdr-pi`, `herdr-pro`, and the herdr fork, locally and on the box. A dirty build is marked and always installed; a changed commit is installed normally. Historical task files without running-process evidence still deserialize.

## Gates

All requested gates passed.

- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check` — exit 0; no output.
- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test` — exit 0. Final result lines:
  - `test result: ok. 626 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`
  - `test result: ok. 57 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`
  - `test result: ok. 82 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`
  - integration groups: 8, 6, 2, and 3 passed; 0 failed.
- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets -- -D warnings` — exit 0; `Finished dev profile [unoptimized + debuginfo] target(s) in 23.95s`.
- `git diff --check` — exit 0; no output.
