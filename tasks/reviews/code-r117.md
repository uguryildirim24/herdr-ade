+++
verdict = "MERGE"
round = "r117"
candidate = "c2b0e752be6f71a5035fc71e3e8720ef706331ad"
manifest_hash = "d91d5480ad02f98c02d16b2120caf978a7e38ac1d30c3edbc3ba38a53eb48b03"
policy_hash = "7d013b92516df0434273f0f2ae57e9ec661dd6ba01b7b9a588a327b3cc724425"
gates = [{ command = "cargo fmt --check", exit = 0 }, { command = "cargo test", exit = 0 }, { command = "cargo clippy --all-targets -- -D warnings", exit = 0 }, { command = "git diff --check", exit = 0 }]
+++

# Review

## t-0314

The earlier reviewed candidate merges cleanly over the newer integration base. The storage move remains consistent with the intervening round and follow-up changes: records, artifacts, box courier paths, lane cards, report references, and tests all use the hidden state folder without undoing the newer reviewer-lifecycle behavior.

The earlier review fixes are present. Installation converts local records before later machine work can fail and converts box records immediately after installing the box plugin. Conversion also fails closed when the old thread-record location cannot be read.

All pinned gates pass on candidate `c2b0e752be6f71a5035fc71e3e8720ef706331ad`.
