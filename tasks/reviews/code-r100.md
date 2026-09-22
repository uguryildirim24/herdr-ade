+++
verdict = "MERGE"
round = "r100"
candidate = "62525575bd12bc368ac65db66f3ad99eb837c475"
manifest_hash = "afb2a1a272b97ceb93f0ce47687a01e9fd0d25a9b46c0bbd1bc7bca263e803c9"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

# Round r100 review

MERGE. Both pinned lane commits and the revision-2 brief are ancestors of the candidate. I found no code defect requiring a review fix.

## W22: first brief after readiness

Adopted helpers remain `starting` while `thread adopt` owns delivery, wait for Herdr to report `idle` or `done`, and retain an open record with `prompt_pending = true` when readiness or prompt confirmation fails. The ticker therefore cannot race adoption, and its retry clears the pending bit only after Herdr observes `working` or `blocked` following submission.

The changed ticker call is also the ordinary `thread start` priming path. It still runs only for a ready agent with a pending prompt. A successful call returns on the initial `working` or `blocked` transition rather than waiting for the turn to finish; only a refused or unobserved submission stays pending. The ordinary-start regression `threads::tests::ade_start_uses_git_worktree_and_tab_env_then_parent_launch` passed and still observes exactly one prime over two ticker passes.

## W23: bridge restart evidence

The bridge record now binds the observed port to PID plus the process start stamp. A missing PID, missing process evidence, unavailable recorded port, or a live old-format PID that disagrees with a fallback answer is reported as unknown without cooldown or drain. Equal PID/start identity is not a restart even if the observed port differs. A confirmed PID or start-stamp change trips the breaker with recorded and running values. The optional fields keep historical state readable.

W18's Pro result handling is intact: `task_complete.error.message`, the trailing `stream_error` settle pass, and their fixture regressions remain in the candidate and passed.

## Current main

The lanes do not merge collectively without a repair review onto current `main` (`9663d449ff7a81469601496d97917fb0450fc895`). W23 merges cleanly. W22 has one textual conflict in `src/herdr.rs`: current main added `pane_submit_text` at the same insertion point where W22 adds the readiness and waited-prompt helpers. The methods are independent and the other W22 files auto-merge, but the harness correctly needs its moved-base repair review before landing.

## Requested checks

The round manifest lists no gates, so front matter remains `gates = []`. Rolf requested these additional checks; all ran on the candidate:

- `cargo fmt --check` — exit 0; no output.
- `cargo test` — exit 0; final suites: 633 main, 57 `herdr-pi`, 86 `herdr-pro`, then 8, 6, 2, and 3 integration tests; all passed with 0 failures. Final line: `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s`.
- `cargo clippy --all-targets -- -D warnings` — exit 0; final line: `Finished dev profile [unoptimized + debuginfo] target(s) in 25.19s`.
- `git diff --check` — exit 0; no output.

The live bridge and its state were not touched.
