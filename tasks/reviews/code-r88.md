+++
verdict = "MERGE"
round = "r88"
candidate = "ccb1a6b3d32d66e31e416e6c36b88288b786c0ca"
manifest_hash = "57868c9e1a3ebf2f0543a72c800a06b6ffd23198f2414293842345e4352513f1"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

# Code review: r88

## Verdict

MERGE.

## t-0211

The lane correctly preserves the page's exact `stream_error` message, classifies the two observed failures as `provider`, keeps rate limits on the cooldown route, and writes the same typed class and message to the failed turn and coordinator notice. A normal non-empty completion remains delivered. Historical turn TOML still loads because `failure_class` remains a defaulted optional field. The Pro code does not set a Codex status-line tag; its only turn-specific pane input is the `TURN <tag>` prompt.

I fixed two review findings in `review(pro): settle trailing errors and make collection idempotent`:

- A trailing error was retained only when it happened to be present in the same filesystem read as `task_complete`. The reader now waits one normal poll and drains complete trailing events, with a test that appends the error after the completion was already read.
- Retrying a collector could prompt and deliver a terminal turn again. Per-turn collection is now serialized and a retried collector returns after observing the terminal record; the test proves there is no second prompt or answer file.

The fixture tests cover both real page messages. The failed-record test now also checks that the coordinator notice contains the exact provider line.

## Checks

- `cargo fmt --check` — exit 0.
- `cargo test` — exit 0: 607 main, 57 pi, 82 Pro, then integration suites of 8, 6, 2, and 2 tests; all passed.
- `cargo clippy --all-targets -- -D warnings` — exit 0; finished successfully.
- `git diff --check` — exit 0.

No live Pro turn was run on the cloud box; Pro runs on the Mac and needs verification after installation.
