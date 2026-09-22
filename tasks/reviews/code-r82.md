+++
verdict = "MERGE"
round = "r82"
candidate = "7ffe42bec252c9e72f7bb8023dae39adf0541bc4"
manifest_hash = "8d8a42ec86ea849132a9512b747a0c0b44c349db29205fb66215e77535b2360c"
policy_hash = "02816b428036d87760383c4f2dd1dc60ad38d9c9d1c0824896ac6bbcd2b3b342"
gates = []
+++

# Verdict

MERGE.

## t-0194

The project thread cap is gone from settings, thread start, coordinator context, the coordinator skill, and current operations documentation. The old count helper and its slot-semantics test are deleted. `doctor` now names `PROJECT.md`, the removed `max_parallel_threads` setting, and the required deletion.

Machine placement, readiness, hold, fallback, and disk-capacity behavior are unchanged. The full requested box checks passed, including the existing coverage for those paths. No review fix was needed.

## Checks requested on the box

- `cargo fmt --check` — exit 0; no output.
- `cargo test` — exit 0; 596 main, 57 `herdr-pi`, 79 `herdr-pro`, and all integration tests passed.
- `cargo clippy --all-targets -- -D warnings` — exit 0; finished successfully.
- `git diff --check` — exit 0; no output.
