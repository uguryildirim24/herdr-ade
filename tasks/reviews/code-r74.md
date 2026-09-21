+++
verdict = "MERGE"
round = "r74"
candidate = "796737029ea2f1e4051070988da9217539f35eb0"
manifest_hash = "bdf1ec354e6c413deb17488d53ac4a477e3c4e7c61ed9eadcaea96c158485ea4"
policy_hash = "bff8071031d843087f976592f50ab33dc6b68d153f49f9b3e9a328610080ef29"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check"]
+++

The repaired candidate keeps the bounded retry, cancel, rebind, and adopt recovery family intact on the newer integration base. Thread cancellation inspects both tracked changes and non-disposable ignored data before removal. Round cancellation uses that same thread path for lanes and reviewers, while closed-round review checkout cleanup performs the same ignored-data inspection directly. Superseded-reviewer cleanup also delegates to thread cancellation, so it cannot bypass the check.

The earlier review fixes remain present: interrupted reviewer placement recovers the exact reviewer rather than creating a duplicate, reviewer adoption checks repository and exact review head as well as sealed verdict evidence, and an adopted verdict remains stable through the next advance pass. No regression was found in the rest of r74.

All requested gates pass on the cloud box.
