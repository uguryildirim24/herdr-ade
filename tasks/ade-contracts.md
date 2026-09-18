# Package ade-contracts — lane a0 (Cursor Grok 4.6 extra-high)

Plain: this is the first piece of the coordination plugin. It renames the base plugin
to its new name, writes down the shared data shapes every later piece will use, and
builds the plain-language checker that every message Rolf reads will pass through.
Nothing here talks to a running herdr; it is types, a checker, fixtures and a test
skeleton.

## Setup

- Repo `/Users/rolfie/projects/herdr-ade` (a GitHub fork of eliasstravik/herdr-projects;
  `origin` = uguryildirim24/herdr-ade, `upstream` = the base). Worktree
  `/Users/rolfie/projects/herdr-ade/.worktrees/a0`, branch `lane/ade-contracts` from
  `main` at a4cdb0a.
- Build: `CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/a0`,
  `DEVELOPER_DIR=/Library/Developer/CommandLineTools`. Rust 1.89, edition 2024, no
  toolchain file. Cargo only; no `herdr plugin link`, no `herdr plugin install`, no
  `cargo install`, nothing written under `~/.config`, `~/.herdr-ade`, `~/.herdr-projects`
  or the live herdr server. Throwaway runs use `HERDR_ADE_ROOT=/var/tmp/ade-a0/root`
  and an isolated `XDG_CONFIG_HOME=/var/tmp/ade-a0/xdg`. Never `git stash`, never push.
- Start line (for a restart after GONE):
  `herdr agent start a0 --kind cursor --pane <pane> --parent w1F:p1 -- --model cursor-grok-4.6-xhigh --force`

## Spec (read only)

`/Users/rolfie/projects/herdr/tasks/SPEC-ADE.md` v3.1. Your package is §4.2
"Prerequisite A0: `ade-contracts`" (lines 1254 to 1267). The sections it draws on:
D1 (name and home: plugin id, crate and binary `herdr-ade`, root `~/.herdr-ade`,
`HERDR_ADE_ROOT`, config `~/.config/herdr-ade/`), D2 (roles table type), D5 (`Op` with
the complete requested payload, helper pid, revision, fixed event id; `Event` with tagged
payload; the delivery journal line), D6 (round record with the authoritative admission
manifest and its revision; merge and checkpoint intents), D17 (the plain checker rules
R1 to R7, `plain/words.txt`, `plain/vocabulary.txt`, the glossary as an input value),
D18 (`Ask`, `HumanMessage`, the talk journal record with request states), §6 items 32 to
35 (the types they require and the fixtures they name), §1.3 (the FakeRunner scenarios
for the new verbs), §4.3 (the required-row list for `scripts/acceptance`). Base plugin
line references in the spec use the `hp:` prefix = this repository at a4cdb0a.

## Owned files (nobody else edits them; you edit nothing else)

1. The rename tuple: `Cargo.toml` (package `herdr-ade`), `Cargo.lock`, `herdr-plugin.toml`
   (`id = "herdr-ade"`, `name`, every `target/release/herdr-ade` command line, keep
   `min_herdr_version = "0.9.1"`), `build.rs`, `src/actions.rs` (the id and binary
   references at `hp:src/actions.rs:18,108-109`), and every other literal
   `herdr-projects`/`herdr_projects` in `src/`, `skill/`, `scripts/`, `README.md` that names
   the plugin id, crate, binary, root dir or config dir (list each one you changed in the
   report; the root becomes `~/.herdr-ade`, the config dir `~/.config/herdr-ade`, the env
   override `HERDR_ADE_ROOT`).
2. New `src/contracts.rs`: the shared data types as the spec defines them, `serde`
   round-trip tested: thread record (`role`, `launch`, `attempt`, `partial`, `bootstrap`,
   `plain`, identity binding), roles table, `Op`, `Event`, delivery journal line, `Ask`,
   `HumanMessage`, round record with admission manifest and revision, merge intent,
   checkpoint intent, talk journal record with request states (`queued`, `submitted`,
   `uncertain`, `accepted`). Every field the spec names, none it does not; a doc comment
   per type naming the spec section. Registered in `src/main.rs` (one `mod` line: this is
   the one exception to "edit nothing else"; the same for `src/plain.rs`).
3. New `src/plain.rs` with `plain/words.txt` and `plain/vocabulary.txt`: the complete pure
   checker (a function from text plus a glossary value to a pass/fail with the failing
   rule and span), rules R1 to R7 exactly as D17 states them, normalization as D17 states,
   no hooks, no file writes, no glossary persistence. `plain/words.txt` is a checked-in,
   versioned, redistributable everyday-English list (item 29): record its source URL,
   version and licence in `plain/README.md`; a list whose licence forbids redistribution is
   refused. Fixtures: one passing and one failing text per rule plus the adversarial cases
   D17 names, as unit tests. A permissive stub is not a passing gate: this checker is the
   real one A2's hook and A3's board will call.
4. FakeRunner scenario wiring for the new verbs (`src/runner.rs`, `src/scenarios.rs`): the
   scenario names and canned herdr replies §1.3 lists for `thread start --parent`, `ha
   done`, `ha waiting`, `round open`, `round review`, `checkpoint`, `ask`, `say`, `talk`
   (only what §1.3 names; no behaviour behind them yet).
5. `scripts/acceptance/` skeleton: a runner script and the required-row list from §4.3,
   every row present with its id and title and marked NOT-RUN, printing the PASS/FAIL/
   NOT-RUN report format §4.3 asks for.

## Gates (all green before DONE; paste each command and its last lines in the report)

```
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
HERDR_ADE_ROOT=/var/tmp/ade-a0/root XDG_CONFIG_HOME=/var/tmp/ade-a0/xdg $CARGO_TARGET_DIR/release/herdr-ade doctor
```

The last one may report a missing herdr socket or config; it must run under the new
name and must not create anything outside `/var/tmp/ade-a0/`.

## Commits

Small, on `lane/ade-contracts`: `chore(ade): rename herdr-projects to herdr-ade`,
`feat(contracts): shared types (SPEC-ADE D2, D5, D6, D18, items 32-35)`,
`feat(plain): pure checker R1-R7 with word list and fixtures (D17)`,
`test(scenarios): FakeRunner wiring for the new verbs (§1.3)`,
`chore(acceptance): required-row skeleton (§4.3)`. The final sha is the pin A1 to A3
branch from.

## Report and closing steps (verbatim, every turn ends with one of these, also on failure)

Write `.reports/ade-contracts-report.md` (git-ignored) in your worktree: what you read,
every renamed literal, every type and which spec lines it comes from, the word list
source and licence, each gate with its output tail, what you could not do and why,
open questions numbered from 1 for the spec's §6 (the coordinator appends them), the
final commit sha.

```
herdr pane report-metadata $HERDR_PANE_ID --source lane --token lane=ade-contracts --token done=1
herdr agent prompt hcoord "DONE ade-contracts .reports/ade-contracts-report.md <final commit sha>" || herdr agent prompt hcoord "DONE ade-contracts .reports/ade-contracts-report.md <final commit sha>"
```

If you must stop: `herdr agent prompt hcoord "WAITING ade-contracts <what>"`.
A turn that ends without one of these pushes is the one failure nothing catches.
