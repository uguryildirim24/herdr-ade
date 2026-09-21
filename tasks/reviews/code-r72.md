+++
verdict = "MERGE"
round = "r72"
candidate = "34a3208a4cd65a796a4b368c23ec5627797d8640"
manifest_hash = "aa68f5e79be6feb79f184905d5fa03f9686280ca2443ffaa503302334d65dfd7"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++

## t-0165

MERGE. The repaired candidate cleanly combines the editable routing table and bounded recovery with r71's typed command results and doctor checks. Missing routing is a failed typed doctor check and the command returns a structured refusal; launch returns the same exact `routing_default_missing` remedy through the typed command result. No old picker, key, or routing-file references remain in the requested source, documentation, and skill folders.

Checks on the candidate:

- `cargo fmt --check` — passed with no output.
- `cargo test` — passed: 558 main tests, 56 `herdr-pi` tests, 79 `herdr-pro` tests, and all integration targets passed.
- `cargo clippy --all-targets -- -D warnings` — passed; finished the dev profile with no warnings.
- `git diff --check` — passed with no output.
- `rg -ni 'jev|typesafe_api_key|routing\.json' src docs skill` — no matches (exit 1, as expected for a clean scan).
- Missing-table doctor probe — exit 1 with `outcome=refused`, `reason=some checks failed`, and failed typed checks carrying `routing_default_missing`.
- Missing-table launch probe through `open` — exit 1 with `outcome=refused` and the exact `routing_default_missing` reason.
