# t-0120 — dead Rust removed

## Result

Removed nine unreachable items (twelve declarations when paired constants/methods are counted), removed two lying `dead_code` allowances, and narrowed 1,781 declarations/fields from public visibility. No forbidden file, test block, test file, or non-Rust surface was changed.

## Deletions and reachability evidence

For every name below I ran an exact identifier search across `src/`, `tests/`, `skill/`, `plain/`, `docs/`, `scripts/`, and `config/`. After deletion every search returned no occurrence.

- `src/events.rs`: deleted `list_imports`, `source_path`, and `artifact_dir_path`. Each had only its declaration before deletion. This also removed their three `#[allow(dead_code)]` attributes.
- `src/pi/mod.rs`: deleted `HERDR_EXTENSION_FILE` and `Layout::herdr_extension`; the constant was reached only by the dead method.
- `src/pi/recipes.rs`: deleted `is_pi_provider`; it had no caller.
- `src/pi/launch.rs`: deleted `agent_start_line`; it had no caller. `agent_start_args` remains live from `src/launch.rs`.
- `src/pro/bridge.rs`: deleted `health_json`; no doctor or JSON path called it.
- `src/pro/state.rs`: deleted `LANE_STATES`; runtime state validation does not use the list.
- `src/remote.rs`: removed obsolete `#[allow(dead_code)]` from `COPY_TIMEOUT` and `fetch_batch`; both are reached by the production courier (`steps::courier`).

I also compiled every target after temporarily removing all dead-code allowances and temporarily narrowing all public fields. That exposed no further globally dead item beyond the deletions above and the test-owned findings below. Repeating the scan after each deletion checked the dead closure rather than only the first layer.

## Visibility

This package has binary targets and no library target, so nothing can consume its Rust API from another crate. All remaining `pub` in the owned non-test files was reduced to `pub(crate)`. Items with no textual reference outside their own source module were narrowed further to private where Rust privacy permitted it. The compiler checked the inferred-return-type cases that a textual search cannot establish safely.

## Dependencies and features

Kept every dependency in `Cargo.toml`: each has direct Rust references (`anyhow`, `clap`, `jiff`, `ratatui`, `crossterm`, `unicode_width`, `serde`, `serde_json`, `sha2`, `toml`, and test-only `tempfile`). `clap/derive`, `serde/derive`, and Ratatui's Crossterm backend are used. Ratatui `layout-cache` changes the active layout engine rather than declaring an unused package feature. The package declares no Cargo features of its own.

## Existing dead-code allowances retained

- `src/main.rs`'s `pi` module: the shared module includes setup/login/doctor code called by the separate `herdr-pi` binary.
- `src/bin/herdr-pi.rs`'s `pi` module: the same shared module includes launch/resume validation called by the main ADE binary, not by the thin setup binary.
- `src/bin/herdr-pro.rs`'s `pro` module: its path-shared `pi/sh.rs` includes shell helpers called by the pi target but not the Pro target.
- `src/remote.rs::fetch_file`: production uses batched fetches, but tests `hostile_values_survive_the_double_shell_of_an_ssh_command` and `unsafe_remote_paths_never_reach_scp` directly call the single-file fallback. The brief forbids changing those tests, so I left this for the test/non-Rust lane to decide.

## Suspected but left

- `pro::lane::RolloutWait::Ready(PathBuf)` never reads its payload in production; `refresh_rollout` already stores the path in the lane. The Pro lane tests match `Ready(_)`, so deleting the payload requires a test edit forbidden to this lane.
- `remote::fetch_file` is production-dead as noted above, but test-reachable. These are findings, not proof that their tests should survive.

## Commits

- `5daf881` core event deletions and local visibility
- `35d0bba` pi deletions and local visibility
- `5ad88ff` Pro deletions and local visibility
- `7887193` core crate visibility
- `418c60b` pi crate visibility
- `84c8d75` Pro crate visibility
- `a818f88` obsolete remote allowances

## Gates

All pass on the final source commit with the box toolchain and `PATH=/bin:$PATH`:

- `cargo fmt --check`
- `cargo test` — 524 main, 56 pi, 78 Pro, and 18 integration tests
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
