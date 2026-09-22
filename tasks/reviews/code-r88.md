+++
verdict = "MERGE"
round = "r88"
candidate = "97ef1382a51fed6b0a651c8aad19f4ed7b340db0"
manifest_hash = "57868c9e1a3ebf2f0543a72c800a06b6ffd23198f2414293842345e4352513f1"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

# Code review: r88 repair

## Verdict

MERGE.

## t-0211

The reviewed candidate applies cleanly to the W10 base. The Pro collector keeps the exact provider line from `error` and `stream_error` events, waits for a trailing event after `task_complete`, classifies the observed “Stopped thinking” and page-error cases as provider failures, and keeps rate limits on the cooldown route. The failed turn and coordinator notice retain the same typed provider class and exact reason.

I fixed one locking race in `review(pro): separate collector and inflight locks`. The earlier idempotence fix used the in-flight marker itself as the collector mutex, but that marker is unlinked when the first collector exits; another retry could then lock a new inode while a waiting retry still held the old one. Collectors now serialize on a stable per-turn lock while the in-flight marker keeps only its liveness role.

W10's recipe visibility and explicit-selection implementation is unchanged by the repair merge. In particular, Pro recipes remain command-only and continue to refuse routing or `thread start --recipe`, naming `herdr-pro start` and `herdr-pro turn` as the supported path.

## Checks

- `cargo fmt --check` — exit 0.
- `cargo test` — exit 0: 610 main, 57 pi, 82 Pro, and integration suites of 8, 6, 2, and 3 tests; all passed.
- `cargo clippy --all-targets -- -D warnings` — exit 0; finished successfully.
- `git diff --check` — exit 0.

No live Pro turn was run on the cloud box; Pro remains Mac-only and should be verified after installation.
