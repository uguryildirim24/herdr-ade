# Package ade-outbox — lane a2

Plain: how a lane finishes and how the coordinator hears about it without reading screens. A
lane says "done" or "waiting", the plugin records it durably in a way that survives a crash
halfway, delivers it to the coordinator's inbox, and the correction hook checks every
message the coordinator writes for Rolf against the plain-language checker.

Start line (for a restart after GONE): `herdr agent start a2 --kind <kind> --pane <pane> --parent w1F:p1 -- <flags per HANDOFF>`

## Owned files (SPEC-ADE §4.2, lane A2)

`src/cli.rs` (subcommand registration; other lanes add only their marked blocks), new
`src/ops.rs` and `src/events.rs` (D5: staged operation with the complete payload, helper
pid, revision and fixed event id; seal create-if-absent with byte equality; the delivery
journal; crash points including X2b; item 32 fixtures as tests), new `src/lane.rs`
(`ha done`, `ha waiting`, `ha skill`), `src/steps.rs` (delivery, `config-changed`,
`report-available`), `src/inbox.rs` (event-linked items, acknowledgement by binding),
`src/coordinator.rs` (receipt, hook install in `open`, `ha close`, `RULES.md` printing, context
sections), `skill/THREAD.md` becomes `skill/LANE.md`, `src/adapters.rs` (per-kind adapter
table, positive-evidence rules, capability labels, D15), new `src/hook.rs` (`ha plain hook`:
per-kind install and removal, the correction budget of item 28 (three requests, one optional
translator call, ten minutes, 64 KiB, 60 s subprocess cap, persistence across continuations,
fixed failure notice), translator off this round, and the typed-publisher contract of item 35:
the hook forwards only `ade-say`/`ade-ask` envelope blocks), the `--plain` refusal on
`dialogue start`, and the verification of the Cursor and Codex hooks on the installed CLIs
before their adapter rows count (read-only probes of the installed binaries; never edit
`~/.cursor` or `~/.codex`; the Claude hook is verified on a throwaway Claude pane in your
throwaway session, kind `claude`, `--model claude-haiku-4-5-20251001`).

Scenarios from §1.3 that are yours: `ha done` sealed and delivered, `ha done` with a dirty tree,
wrong sha, dead helper after staging, helper/ticker race, event written before the sealed
marker, retry with changed HEAD; the hook budget exhaustion; the raw-hook question bypass
(item 35). Your `tick` entry point: `ops::tick`.
## Setup and rules (same for every lane of this round)

- Repo `/home/agent/projects/herdr-ade` (fork of eliasstravik/herdr-projects; `origin` =
  uguryildirim24/herdr-ade). Your worktree `/home/agent/projects/herdr-ade/.worktrees/a2`,
  branch `lane/ade-outbox`, cut from A0's pin `20fd9e9c` (`lane/ade-contracts`: the rename to
  `herdr-ade`, `src/contracts.rs`, `src/plain.rs` with `plain/words.txt` and
  `plain/vocabulary.txt`, FakeRunner wiring in `src/runner.rs`/`src/scenarios.rs`, the
  `scripts/acceptance` skeleton). Read A0's report first:
  `/home/agent/projects/herdr-ade/.worktrees/a0/.reports/ade-contracts-report.md`.
- Build: `CARGO_TARGET_DIR=/home/agent/projects/herdr-ade/.target/a2`,
  `DEVELOPER_DIR=/Library/Developer/CommandLineTools`, Rust 1.89, edition 2024.
- Never: `herdr plugin link` or `install`, `cargo install`, anything under `~/.config`,
  `~/.herdr-ade`, `~/.herdr-projects`, `~/.claude`, `~/.codex`, `~/.cursor`, the live herdr
  server (session `default`, socket in `~/.config/herdr`), another worktree, `git stash`,
  `git push`. Throwaway runs: `HERDR_ADE_ROOT=/var/tmp/ade-a2/root`,
  `XDG_CONFIG_HOME=/var/tmp/ade-a2/xdg`, `XDG_STATE_HOME=/var/tmp/ade-a2/state`, a throwaway
  herdr session `env -u CLAUDE_CODE_CHILD_SESSION /home/agent/projects/herdr/.target/install/release/herdr --session ade-a2 server`
  (the fork's release build, 0.9.1-based, has `--parent` on `agent start`; stop it with
  `herdr --session ade-a2 session stop ade-a2` when done; never `herdr server stop`).
- Spec (read only): `/home/agent/projects/herdr/tasks/SPEC-ADE.md` v3.1. §4.2 names your
  lane's owned files; §1.2 D1 to D18 the behaviour; §1.3 the FakeRunner scenarios; §4.3 the
  reviewer's acceptance rows (you do not run those; you make them runnable); §6 items 32
  to 35 the fixtures that are tests. Decisions: `/home/agent/projects/herdr/tasks/ade/decisions.md`.
  Base plugin line references use `hp:` = `/home/agent/projects/herdr-projects` at a4cdb0a.
- Seams, so four lanes merge without conflicts: you edit only your owned files. The two
  shared files take one marked block per lane and nothing else: in `src/main.rs` your `mod`
  lines between `// ade-outbox begin` and `// ade-outbox end`; in `src/cli.rs` your subcommand enum
  variants and match arms inside the same markers. A field you need in `src/contracts.rs`
  goes in its own commit `contracts(ade-outbox): <field>` that only adds (new fields carry
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

Write `.reports/ade-outbox-report.md` (git-ignored) in your worktree: what you read, what you
built per owned file with spec lines, your `tick` entry point if any, contract fields you
added, each gate with its output tail, "Left for the reviewer" with exact lines, what you
could not do and why, open questions numbered from 1 for SPEC-ADE §6, the final commit sha.

```
herdr pane report-metadata $HERDR_PANE_ID --source lane --token lane=ade-outbox --token done=1
herdr agent prompt hcoord "DONE ade-outbox .reports/ade-outbox-report.md <final commit sha>" || herdr agent prompt hcoord "DONE ade-outbox .reports/ade-outbox-report.md <final commit sha>"
```

If you must stop: `herdr agent prompt hcoord "WAITING ade-outbox <what>"`.
A turn that ends without one of these pushes is the one failure nothing catches.
