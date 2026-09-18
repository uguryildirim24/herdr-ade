# Package ade-core — lane a1

Plain: the plugin's backbone. It reads the roles table (which helper and settings each job
runs on), starts a lane under its coordinator in its own git worktree, keeps the records
that say what each lane is and where it is, and ships the tested binary-swap script the
migration needs.

Start line (for a restart after GONE): `herdr agent start a1 --kind cursor --pane <pane> --parent w1F:p1 -- --model cursor-grok-4.6-xhigh --force`

## Owned files (SPEC-ADE §4.2, lane A1)

`src/main.rs` (module registration; other lanes add only their marked blocks),
`src/paths.rs` (root and config resolution incl. `HERDR_ADE_ROOT` and `XDG_CONFIG_HOME`, A0
open question 4), `src/project.rs` (roles table resolution per D2, launch recipe, front matter
including `talk`; the `[roles]`/`[recipes]` parsing of SPEC-jev-picker v2 is lane A4's: leave
a documented hook point), `src/herdr.rs` (`Agent.tokens`, `--parent` D3, `tab create --env`,
`ready_timeout_ms`, git worktree helpers, `pane get`, `pane process-info`), `src/threads.rs`,
`src/thread.rs`, `src/adopt.rs` (`--role`, `--passive`), `src/ticker.rs` whole file (the
reviewer wires A2's, A3's and A4's `tick` calls), `src/doctor.rs` (the full D1 tuple line, A0
open question 3; capability probes; role warnings; accept the fork's version string, A0
open question 7), `src/git.rs` (repository lock, `worktree_add`, `worktree_remove`,
`commit_file_on_branch`, `update_ref`, ancestry checks, per D6 and item 34), the `--plain`
refusal on `thread start` and `thread adopt` through A0's checker,
`scripts/migration/swap-binary.sh` with `scripts/migration/swap-binary.test.sh` (§3.3 step
6: the state-dependent swap with the exclusive-create backup branch, tested for the retry
and failure cases Pro named; M1 stays a no-go condition until this test passes on this Mac
and the report says so), `README.md`, `docs/operations.md`.

Scenarios from §1.3 that are yours: `thread start --parent`, adopt, restart with the
persisted launch, the plain refusal, the worktree lifecycle. Also A0's open questions 1
and 2 as far as your files go: format what you own, fix the `too_many_arguments` in
`ticker.rs` and drop that allow from `Cargo.toml` if no other file needs it.
## Setup and rules (same for every lane of this round)

- Repo `/Users/rolfie/projects/herdr-ade` (fork of eliasstravik/herdr-projects; `origin` =
  uguryildirim24/herdr-ade). Your worktree `/Users/rolfie/projects/herdr-ade/.worktrees/a1`,
  branch `lane/ade-core`, cut from A0's pin `20fd9e9c` (`lane/ade-contracts`: the rename to
  `herdr-ade`, `src/contracts.rs`, `src/plain.rs` with `plain/words.txt` and
  `plain/vocabulary.txt`, FakeRunner wiring in `src/runner.rs`/`src/scenarios.rs`, the
  `scripts/acceptance` skeleton). Read A0's report first:
  `/Users/rolfie/projects/herdr-ade/.worktrees/a0/.reports/ade-contracts-report.md`.
- Build: `CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/a1`,
  `DEVELOPER_DIR=/Library/Developer/CommandLineTools`, Rust 1.89, edition 2024.
- Never: `herdr plugin link` or `install`, `cargo install`, anything under `~/.config`,
  `~/.herdr-ade`, `~/.herdr-projects`, `~/.claude`, `~/.codex`, `~/.cursor`, the live herdr
  server (session `default`, socket in `~/.config/herdr`), another worktree, `git stash`,
  `git push`. Throwaway runs: `HERDR_ADE_ROOT=/var/tmp/ade-a1/root`,
  `XDG_CONFIG_HOME=/var/tmp/ade-a1/xdg`, `XDG_STATE_HOME=/var/tmp/ade-a1/state`, a throwaway
  herdr session `env -u CLAUDE_CODE_CHILD_SESSION /Users/rolfie/projects/herdr/.target/install/release/herdr --session ade-a1 server`
  (the fork's release build, 0.9.1-based, has `--parent` on `agent start`; stop it with
  `herdr --session ade-a1 session stop ade-a1` when done; never `herdr server stop`).
- Spec (read only): `/Users/rolfie/projects/herdr/tasks/SPEC-ADE.md` v3.1. §4.2 names your
  lane's owned files; §1.2 D1 to D18 the behaviour; §1.3 the FakeRunner scenarios; §4.3 the
  reviewer's acceptance rows (you do not run those; you make them runnable); §6 items 32
  to 35 the fixtures that are tests. Decisions: `/Users/rolfie/projects/herdr/tasks/ade/decisions.md`.
  Base plugin line references use `hp:` = `/Users/rolfie/projects/herdr-projects` at a4cdb0a.
- Seams, so four lanes merge without conflicts: you edit only your owned files. The two
  shared files take one marked block per lane and nothing else: in `src/main.rs` your `mod`
  lines between `// ade-core begin` and `// ade-core end`; in `src/cli.rs` your subcommand enum
  variants and match arms inside the same markers. A field you need in `src/contracts.rs`
  goes in its own commit `contracts(ade-core): <field>` that only adds (new fields carry
  `#[serde(default)]`; never rename or remove). Ticker passes: expose
  `pub fn tick(t: &mut crate::ticker::Ticker) -> anyhow::Result<()>` in your module and name
  it in your report; the round's reviewer wires the call into A1's `ticker.rs`. Nothing
  else crosses lanes; what you cannot do without another lane's code goes in the report
  under "Left for the reviewer", with the exact line you would have written.
- Gates (all before DONE; paste each command and its last lines in the report):
  `cargo fmt --check` (pre-existing wrapping on unowned files fails on `main`; format only
  what you own and say so), `cargo clippy --all-targets --locked -- -D warnings`,
  `cargo test --locked` (your §1.3 scenarios included), `cargo build --release --locked`,
  and the throwaway `doctor`. Commits small and named per package. Never claim a check you
  did not run.

## Report and closing steps (verbatim, every turn ends with one of these, also on failure)

Write `.reports/ade-core-report.md` (git-ignored) in your worktree: what you read, what you
built per owned file with spec lines, your `tick` entry point if any, contract fields you
added, each gate with its output tail, "Left for the reviewer" with exact lines, what you
could not do and why, open questions numbered from 1 for SPEC-ADE §6, the final commit sha.

```
herdr pane report-metadata $HERDR_PANE_ID --source lane --token lane=ade-core --token done=1
herdr agent prompt hcoord "DONE ade-core .reports/ade-core-report.md <final commit sha>" || herdr agent prompt hcoord "DONE ade-core .reports/ade-core-report.md <final commit sha>"
```

If you must stop: `herdr agent prompt hcoord "WAITING ade-core <what>"`.
A turn that ends without one of these pushes is the one failure nothing catches.
