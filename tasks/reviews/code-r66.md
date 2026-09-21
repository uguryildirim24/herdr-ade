+++
verdict = "MERGE"
round = "r66"
candidate = "04b1b3ce7a84663ea068b8f495bfb6e7d4e68b1d"
manifest_hash = "a94a34e7e0ce7867b9375a9a3d152bbe05d6fe7e7abd71a91c8b799603976af3"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++

## t-0149

MERGE. The shared renderer gives the named commands one JSON envelope while preserving their ordinary output, and refusals remain non-zero with their reason in the record. Round phase comes from the durable record, and round advance now reports the reviewer and round pairs it started, including event-driven advances.

Review fixes keep the command identity and available ids on argument-parse refusals, distinguish a reopened thread from a resolved one, expose restart and prompt results, and make the coordinator skill name `data.started` directly. The extra coordinator-requested checks passed: `cargo fmt --check`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `git diff --check`.

No project gates were listed for this review.
