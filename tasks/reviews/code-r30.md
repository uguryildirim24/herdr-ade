+++
verdict = "MERGE"
round = "r30"
candidate = "536f1959c887f4c7e3433296ee497e17348299c6"
manifest_hash = "864956d7c018dafabc69af4553e337020483b0c0ae72ba1e5da1cfe9dc4cad2c"
policy_hash = "ef334053e0961740bc2a51d4698483517101f9dc9bb35a46bff91e23b9d43d21"
gates = []
+++

# Round r30 review

## t-0065

The repair path now keeps the original brief, starts the replacement review from the moved integration head, and tells that reviewer to merge the earlier candidate with its review fixes. The existing retry behavior from t-0058 remains in charge of stale manifests and failed reviewer starts.

I fixed three integration defects:

- A repeated `round review` can no longer clear a reviewer that has not produced a structurally valid sealed verdict. Re-reviews also retain the earlier verdict kind, candidate and exact verdict commit, including the REJECT path.
- Resolution fails closed when any round record is unreadable, and a forced early resolution must publish its warning successfully.
- A blocked pi lane accepts a prompt only when its own recorded error caused the block; a person-facing blocked question remains protected.

The merge-conflict hint, manual replacement of a gone reviewer, open-slot count, stale-event suppression, and coordinator documentation are consistent with the current main-line behavior.

## Checks

No project gates were listed for this round. Additional format, lint, test and release checks were green:

```text
$ cargo fmt --all -- --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked && cargo build --release --locked
Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.21s
...
test result: ok. 430 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 56 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 78 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
Finished `release` profile [optimized] target(s) in 13.79s
```
