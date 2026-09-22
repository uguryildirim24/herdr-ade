+++
verdict = "MERGE"
round = "r100"
candidate = "8261bd5d2eddee1299fd2b7189f7f9c265d537d3"
manifest_hash = "afb2a1a272b97ceb93f0ce47687a01e9fd0d25a9b46c0bbd1bc7bca263e803c9"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

# Round r100 repair review

MERGE. The earlier reviewed candidate was merged onto current `main`; both pinned lane commits and the revision-2 brief are ancestors of the candidate. The only conflict was composed in `src/herdr.rs` by retaining both independent methods.

## W22: first message after readiness

The adoption path still owns first delivery while the helper is starting, waits for `idle` or `done`, and requires Herdr to observe `working` or `blocked` after prompting. Failures leave the brief pending. The ticker's pending-brief retry uses the same waited-prompt method and clears `prompt_pending` only after observed submission. Its three focused adoption regressions passed.

## W23: bridge restart evidence

Bridge identity still records port, PID, and process start. Missing evidence and a live old-format PID remain unknown without cooldown or drain; a confirmed identity change trips the breaker with the differing values. The four focused bridge regressions passed.

W18's Pro failure handling remains intact: `task_complete.error.message` and a trailing `stream_error` are retained, and both focused regressions passed.

## W26 and W27 composition

W26's `pane_submit_text` remains beside W22's readiness and waited-prompt methods. The blocked pi error-screen path still submits through the pane, clears only its durable adapter error, and refuses a blocked question or gone pane; its regression passed.

W27's durable ticker handoff remains intact. W22 changes only the pending-brief prompt call inside that ticker. The slow-pass handoff regression passed.

## Requested checks

The manifest has no gates, so front matter remains `gates = []`. These requested checks all passed on the candidate:

- `cargo fmt --check` — exit 0; no output.
- `cargo test` — exit 0. Unit suites: 642 `herdr-ade`, 58 `herdr-pi`, and 86 `herdr-pro`; integration suites: 8, 6, 2, and 3 tests. Final line: `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s`.
- `cargo clippy --all-targets -- -D warnings` — exit 0. Final line: `Finished dev profile [unoptimized + debuginfo] target(s) in 24.63s`.
- `git diff --check` — exit 0; no output.

The live web bridge and its state were not touched.
