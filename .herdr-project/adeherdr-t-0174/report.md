# W1 report

Implemented safe worktree cleanup for ignored data.

- Added shared local and box inspection using `git status --porcelain --ignored --untracked-files=all`.
- Added editable `[worktrees].disposable`; an absent table treats every ignored path as data.
- Finished lanes now resolve while retaining ignored data with an `ignored_data` reason, folder names, and sizes. Dirty tracked or untracked work still refuses resolution.
- Nested Git checkouts are always retained, including inside disposable folders.
- Closed-round review cleanup uses the same inspection.
- `doctor` separates retained data from mistakenly leftover finished worktrees.
- Updated the coordinator skill, README, and operations/getting-started docs.

Tests cover configured `target/` removal, unconfigured `camber-runs/` retention, box retention, absent config, nested worktrees, review worktrees, and doctor reporting.

Gates passed:

- `cargo fmt --check`
- `cargo test` (566 main tests, 56 herdr-pi tests, 79 herdr-pro tests, and integration suites)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
