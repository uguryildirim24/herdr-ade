# Package ade-picker — lane a4

Plain: the lane picker. When a lane starts, a small outside ranking service (Jev) may answer
one or two yes-or-no questions ("is this web research?") over a fixed table of helpers and
switch the lane away from the usual helper only when it is sure. It ships switched off,
first only recording what it would have picked, so Rolf can compare it with his own choices
for a month.

Start line (for a restart after GONE): `herdr agent start a4 --kind <kind> --pane <pane> --parent w1F:p1 -- <flags per HANDOFF>`

## Spec

`/home/agent/projects/herdr/tasks/jev/SPEC-jev-picker.md` v2 (read only), especially §2 (the
typed decision), §3 (where it plugs in), "Design (normative)", "Plugin touch points by file",
and its open questions (Rolf's answers are pending; ship `resolver = "off"` as the default and
`shadow` as the only other mode this round; no real launches from Jev). Key file
`~/.config/typesafe/api_key` is read only and only by the throwaway `doctor`; every test
uses FakeRunner fake scripts, never the live service; the live Jev call is made only from
the throwaway `doctor` once, and reported (cost cap for this lane: $0.05).

## Owned files (SPEC-jev-picker v2 touch points; additive only)

New `src/jev.rs` (excerpt transform and scrub, gate questions, the curl `Cmd` through the
Runner, response parse and acceptance rules, pin `jev-1.13.0`, 3 s total with one retry,
one call per thread and per dialogue side, the 200-per-project-per-day guard), new
`src/launch.rs` (`resolve_launch(project, role, task, sibling)`: validation of the recipe
table against `herdr agent start --help` kinds, the pair filter for `dialogue start`, lock
discipline, daily cap, shadow log line with `ade_last` compact reason; config parsing of
`[roles] resolver`, `[recipes]`, `allowed`, `gates`, `escalate`, `policy_hash` covering
them, exposed as `pub fn parse_picker_config` for A1's `project.rs` to call), the
`contracts(ade-picker)` additive commit for `ResolverMode`, `Recipe { kind, args, env,
ready_timeout_ms, provider, cost, enabled, plain }`, `Gate`, and the `Launch` fields the spec
lists, runner fake scripts and tests (200 pick, 200 low confidence, 200 other model, 429 then
200, timeout, 401, malformed, daily cap), `pub fn doctor_rows()` for A1's doctor (key, key
mode, curl, `GET /v1/models`, recipe validation, `command -v` per kind, the Codex-quota
reminder, the Astra `enabled` note), reason templates as checked `plain` phrases with
fixtures in a new `tests/picker_plain.rs` and `picker` added to `plain/vocabulary.txt`, and
`skill/PICKER.md` (how a coordinator pins `--recipe`).

Integration into `project.rs`, `threads.rs` (`--role`, `--recipe`), `thread.rs` (persist
`launch`), `ticker.rs` (`agent_start` from `launch.kind` and `launch.args` only) and the
round/dialogue verbs is the reviewer's, from your "Left for the reviewer" lines. Your `tick`
entry point: none (the picker runs at launch time).
## Setup and rules (same for every lane of this round)

- Repo `/home/agent/projects/herdr-ade` (fork of eliasstravik/herdr-projects; `origin` =
  uguryildirim24/herdr-ade). Your worktree `/home/agent/projects/herdr-ade/.worktrees/a4`,
  branch `lane/ade-picker`, cut from A0's pin `20fd9e9c` (`lane/ade-contracts`: the rename to
  `herdr-ade`, `src/contracts.rs`, `src/plain.rs` with `plain/words.txt` and
  `plain/vocabulary.txt`, FakeRunner wiring in `src/runner.rs`/`src/scenarios.rs`, the
  `scripts/acceptance` skeleton). Read A0's report first:
  `/home/agent/projects/herdr-ade/.worktrees/a0/.reports/ade-contracts-report.md`.
- Build: `CARGO_TARGET_DIR=/home/agent/projects/herdr-ade/.target/a4`,
  `DEVELOPER_DIR=/Library/Developer/CommandLineTools`, Rust 1.89, edition 2024.
- Never: `herdr plugin link` or `install`, `cargo install`, anything under `~/.config`,
  `~/.herdr-ade`, `~/.herdr-projects`, `~/.claude`, `~/.codex`, `~/.cursor`, the live herdr
  server (session `default`, socket in `~/.config/herdr`), another worktree, `git stash`,
  `git push`. Throwaway runs: `HERDR_ADE_ROOT=/var/tmp/ade-a4/root`,
  `XDG_CONFIG_HOME=/var/tmp/ade-a4/xdg`, `XDG_STATE_HOME=/var/tmp/ade-a4/state`, a throwaway
  herdr session `env -u CLAUDE_CODE_CHILD_SESSION /home/agent/projects/herdr/.target/install/release/herdr --session ade-a4 server`
  (the fork's release build, 0.9.1-based, has `--parent` on `agent start`; stop it with
  `herdr --session ade-a4 session stop ade-a4` when done; never `herdr server stop`).
- Spec (read only): `/home/agent/projects/herdr/tasks/SPEC-ADE.md` v3.1. §4.2 names your
  lane's owned files; §1.2 D1 to D18 the behaviour; §1.3 the FakeRunner scenarios; §4.3 the
  reviewer's acceptance rows (you do not run those; you make them runnable); §6 items 32
  to 35 the fixtures that are tests. Decisions: `/home/agent/projects/herdr/tasks/ade/decisions.md`.
  Base plugin line references use `hp:` = `/home/agent/projects/herdr-projects` at a4cdb0a.
- Seams, so four lanes merge without conflicts: you edit only your owned files. The two
  shared files take one marked block per lane and nothing else: in `src/main.rs` your `mod`
  lines between `// ade-picker begin` and `// ade-picker end`; in `src/cli.rs` your subcommand enum
  variants and match arms inside the same markers. A field you need in `src/contracts.rs`
  goes in its own commit `contracts(ade-picker): <field>` that only adds (new fields carry
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

Write `.reports/ade-picker-report.md` (git-ignored) in your worktree: what you read, what you
built per owned file with spec lines, your `tick` entry point if any, contract fields you
added, each gate with its output tail, "Left for the reviewer" with exact lines, what you
could not do and why, open questions numbered from 1 for SPEC-ADE §6, the final commit sha.

```
herdr pane report-metadata $HERDR_PANE_ID --source lane --token lane=ade-picker --token done=1
herdr agent prompt hcoord "DONE ade-picker .reports/ade-picker-report.md <final commit sha>" || herdr agent prompt hcoord "DONE ade-picker .reports/ade-picker-report.md <final commit sha>"
```

If you must stop: `herdr agent prompt hcoord "WAITING ade-picker <what>"`.
A turn that ends without one of these pushes is the one failure nothing catches.
