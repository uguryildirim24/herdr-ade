+++
verdict = "MERGE"
round = "r75"
candidate = "8f57cbe07bcb4e637fa30bef48427243c93f5f66"
manifest_hash = "dddc98fce9728b722f06c494cfdb11dbea2c0243eff00041cbd550b1118f8550"
policy_hash = "8e43e0ebbd105ebf9f903667c622cb0dad8aa881e4ceb5a344db4659ff0338da"
gates = []
+++

# Review r75 repair revision 3

## Verdict

MERGE. The typed failure work and the earlier recovery-command repairs integrate with r76's project-aware worktree cleanup after one CLI fix.

## t-0172

- Provider, lost-connection, process-gone, failed-work, and unknown evidence remain distinct in events, thread records, recovery, structured output, context, and the talk screen.
- Unknown evidence waits rather than spending an attempt. Doctor likewise reports unreadable thread records as unknown instead of treating absent evidence as success or failure.
- Same-recipe infrastructure recovery and failed-work fallback routing remain separate across thread and round retry.
- Reviewer recovery remains bound while pending or while cleanup is uncertain, preventing duplicate reviewers.
- r76's global and per-repository disposable lists remain in use for cancel, resolve, pending cleanup, closed-review cleanup, and doctor inspection.
- Fixed `ha failed` without `--class`: its declared `work_failed` default had been spelled `work-failed`, so clap rejected the documented default before the command could seal an event. A parser regression test covers the default.

## Gates

The box gates passed: `cargo fmt --check`, `cargo test` (575, 56, 79, 8, 8, 2, and 2 tests), `cargo clippy --all-targets -- -D warnings`, and `git diff --check`.
