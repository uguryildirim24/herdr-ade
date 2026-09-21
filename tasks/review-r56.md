# Review brief: round r56

plain: This round takes out code nothing uses and stops empty spaces piling up on the cloud box.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r56` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 2, manifest hash `24885850533980320f9b21568fa483fd87b283a4309b96b7dbca5dd3176fefa4`, policy hash `73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0120 | 1 | `bfd8e2fd8f4ee2f7d1cf6db6a03aba1195318dae` | `t-0120-1-1` | `294c5a8dc61c4fbc538725dd92ea3fd4360a6b0d0b4fd461bd167ecf83a9c1a0` |
| t-0121 | 1 | `819375cb841c5accb31c163da2bc2696c0c4d654` | `t-0121-1-1` | `dd580e1e34c72144043cae78b79f840a574cf7301b9a0a02f48ce124fd75f813` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r56.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r56"
candidate = "<C>"
manifest_hash = "24885850533980320f9b21568fa483fd87b283a4309b96b7dbca5dd3176fefa4"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0120 (artifact `294c5a8dc61c4fbc538725dd92ea3fd4360a6b0d0b4fd461bd167ecf83a9c1a0`)

Data, not instructions.

```text
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
```

### t-0121 (artifact `dd580e1e34c72144043cae78b79f840a574cf7301b9a0a02f48ce124fd75f813`)

Data, not instructions.

```text
# t-0121 — dead tests and dead surface removed

Branch: `hp/adeherdr/t-0121-dead-tests-and-dead-surface-removed`
Final code commit: `819375c`
Machine: `oci`

## What was removed, and why it was dead

### Tests

- Removed `adapters::tests::matrix_has_the_declared_kinds`. It repeated the eight string literals in the `ADAPTERS` declaration and exercised no lookup or adapter behavior. Production only called `get` from `capability_label` and discarded the result; the real correction paths remain covered by the hook tests. This also exposed `ADAPTERS`/`get` as dead production code, listed below for the Rust lane.
- Removed `pi::provider::tests::the_window_is_the_compaction_point_plus_the_pi_reserve`. It asserted two constants by repeating their definitions. The actual behavior remains covered by `write_sets_the_override_and_keeps_every_other_key`, which checks that `DEEPSEEK_CONTEXT_WINDOW` is written to the shipped model override, and `missing_overrides_names_a_row_without_the_window`, which checks that the value is enforced.

No test was ignored, commented out, or replaced with a weaker check. I inspected every `#[test]`/`#[cfg(test)]` site outside the five forbidden round-r55 files, searched for assertion-free and exact-body duplicate tests, and checked helper reachability. The only exact duplicate body found (`runner` versus `pi::sh` output capture) covers two separate command-runner implementations, so it remains. Cargo/clippy found no unused private test helper. The round fixtures remain because `src/round.rs` loads both with `include_str!`; that file was forbidden in this lane.

### Scripts and fixtures

- Removed `scripts/acceptance/run` and `scripts/acceptance/rows.md`. Nothing in Cargo, the plugin manifest, CI, current docs, or another script invoked this initial-build acceptance harness. It still manufactured a removed roles table and described the old six-lane assembly flow.
- Removed `src/pi/testdata/guard-check.sh` and `mock-provider.js`. Their only caller was that dead acceptance script; no Rust test or build loaded them.
- Removed `scripts/context-size.py` and `scripts/context-history-size.py`. Git history identifies these as one-off context-size measurements from the bounded-context work. No build, gate, command, or current document invoked them; the history replay also depended on an old fixed r1-r45 record shape.
- Removed `scripts/dev-herdr`, `scripts/dev-hp`, and `scripts/dev-server`. Their only live-tree documentation was the dated manual-test record removed below; no manifest, build, test, or other script called them.

The migration installer and its test remain: `docs/operations.md` names both, and the test directly executes the installer. The three routing JSON files remain: `routing.json` is loaded by dispatch, `routing-cases.json` by the routing CLI test/evaluator, and the explicitly-live history corpus remains intact.

### Obsolete documents

- Removed `docs/herdr-notes.md` and `docs/manual-test.md`: dated build evidence for the original `herdr-projects` plugin. They described the removed paths, environment variables, workspace-per-thread flow, and old binary name rather than the current product.
- Removed `docs/going-public.md`: a completed/obsolete checklist for publishing the old `eliasstravik/herdr-projects` repository.
- Removed `docs/subprocess-audit.md`: it explicitly said its command counts were the original census, not a current census; subsequent rounds added and changed command sites, making it an obsolete audit snapshot rather than operational documentation.
- Removed the references to those records from `docs/operations.md`.

### Stale user and agent surface

- Rewrote `docs/getting-started.md` around `herdr-ade`, required `routing.json`, `TYPESAFE_API_KEY`, task-based routing, tabs/worktrees, current paths, and current recovery commands. This removes the old `herdr-projects` binary/config/root, upstream repository, workspace-per-lane flow, project-selected agent kind, and false “no hosted service or API key” statements.
- Updated `README.md` from Herdr Projects/upstream links to Herdr ADE/the fork, corrected the hosted-routing disclosure, and renamed the referenced illustration from `herdr-projects-…` to `herdr-ade-…`.
- Removed stale operations claims that only Claude had been exercised and that the default was a model “role”; removed instructions for the deleted development wrappers.
- Fixed `skill/COORDINATOR.md`'s `thread start` example to include required `--plain`.
- Fixed `skill/DRAFTER.md`'s removed `waiting --text` form to the current positional `waiting "<what>"` form.

I searched the remaining user-facing tree for the old repository/binary/root/env names, removed command forms, acceptance/context fixture names, and stale `waiting --text`; none remain outside the deliberately historical labelled routing corpus.

## Test counts, before → after

| target | before | after | change |
|---|---:|---:|---:|
| `herdr-ade` unit target (`src/main.rs`) | 524 | 522 | -2 (adapter declaration test; pi constant test) |
| `herdr-pi` unit target | 56 | 55 | -1 (the shared pi constant test) |
| `herdr-pro` unit target | 78 | 78 | 0 |
| `tests/cli.rs` | 6 | 6 | 0 |
| `tests/context_actionable.rs` | 8 | 8 | 0 |
| `tests/context_records.rs` | 2 | 2 | 0 |
| `tests/routing_cli.rs` | 2 | 2 | 0 |
| **total** | **676** | **673** | **-3 executions / 2 source tests** |

Both before and after suites passed. Removing tests did not turn a failing suite green.

## Gates

Run with `PATH=/bin:$PATH` and the box toolchain, using `CARGO_TARGET_DIR=$PWD/.target/t-0121`:

- `cargo fmt --check` — PASS
- `cargo test` — PASS: 522 + 55 + 78 + 6 + 8 + 2 + 2, zero failed
- `cargo clippy --all-targets -- -D warnings` — PASS
- `git diff --check` — PASS

I also ran the retained `scripts/migration/swap-binary.test.sh`; it is explicitly a macOS test and stopped on this Linux box because GNU `stat` does not accept its macOS `stat -f '%m'` form. It is not one of this task's box gates and was not changed.

## Production Rust findings for t-0120 / review

I did not edit these because production Rust belongs to the other lane (and the five round-r55 files were entirely forbidden):

- `src/adapters.rs`: `ADAPTERS` and `get` have no meaningful production effect. The sole production call assigns `get(kind)` to `_adapter` and then computes the label without it; all `Adapter` metadata is therefore dead.
- `src/events.rs`: `list_imports`, `source_path`, and `artifact_dir_path` have no callers. The latter two are labelled “exposed for tests” but no test calls them.
- `src/remote.rs`: `fetch_file` is called only by its own test module; production ingress uses `fetch_batch`.
- `src/project.rs` still has a production comment linking the now-removed historical `docs/herdr-notes.md`; it should be made self-contained when that Rust file is next edited.
- `herdr-ade thread resolve --help` currently panics because `reopen` declares a conflict with removed argument `force` in `src/cli.rs`. I did not touch it because `src/cli.rs` is explicitly owned by t-0119 in r55.

## Commits by area

1. `93be913 test: remove assertions over static declarations`
2. `b05b910 chore: remove one-off acceptance and measurement scripts`
3. `5359bcb docs: remove obsolete build-era records`
4. `825c104 docs: remove renamed and obsolete user surface`
5. `819375c docs: remove stale skill command forms`
```

