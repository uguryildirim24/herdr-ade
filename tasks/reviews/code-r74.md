+++
verdict = "MERGE"
round = "r74"
candidate = "c5d6fe06e3f4f5ffe94e5b6a1916d886baf5d3a5"
manifest_hash = "bdf1ec354e6c413deb17488d53ac4a477e3c4e7c61ed9eadcaea96c158485ea4"
policy_hash = "bff8071031d843087f976592f50ab33dc6b68d153f49f9b3e9a328610080ef29"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check"]
+++

The recovery commands are one bounded family and no longer duplicate reviewers after a placement crash. Verdict adoption validates the reviewer repository and exact review base as well as its candidate, manifest and policy evidence. Cancellation reports unreachable cleanup honestly and uses the ignored-data-aware removal check from round r73.

All requested gates pass on the cloud box.
