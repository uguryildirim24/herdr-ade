# Package ade-rounds — lane a3

Plain: the round machinery and everything Rolf looks at. Rounds (open, review, merge, checkpoint,
with recovery after a crash at any point), the spec dialogue between two lanes, the board on
the Spaces tab, questions to Rolf as picturable choices, the glossary and "explain", and the
talk tab: the plugin's own conversation window where only checked replies appear.

Start line (for a restart after GONE): `herdr agent start a3 --kind claude --pane w1F:p11 --parent w1F:p1 -- --model claude-opus-5 --effort high --dangerously-skip-permissions`

## Owned files (SPEC-ADE §4.2, lane A3)

New `src/round.rs` (D6 including the authoritative admission manifest with revision, merge
intent and checkpoint intent, re-validation under the repository lock at the effect
boundary, V to H recovery, "only checkpointed is a no-op"; items 33 and 34 fixtures as
tests), `src/dialogue.rs` (D7: the two-lane spec dialogue; the pair filter hook point for
lane A4), `src/checkpoint.rs` (`ha checkpoint`, `ha pickup`: the port of
`/Users/rolfie/.claude/skills/save-state/state.py` snapshot/check/restore, read only there),
`src/board.rs` (item 14 board templates on workspace tokens `ade_stage`, `ade_lanes`,
`ade_needs_you`, `ade_last`; gate B: every publish path refuses text that fails A0's
checker), `src/glossary.rs` (`GLOSSARY.md`, `ha explain`, `ha term`; the glossary is passed to
the checker as a value), `src/ask.rs` (`ha ask` with two to four picturable choices, `ha say`,
notifications through the check; the typed publisher `Say`/`Ask{id,revision}`/`Notice` of
item 35), new `src/talk.rs` (D18: the plugin-owned conversation surface; request id and
`queued`/`submitted`/`uncertain`/`accepted` states, frozen ask binding while a number is typed,
`!native`/`!back`, fixed notices, one journal append owner; decision on item 24 = ships this
round, on by default for a Claude coordinator, off for other kinds until their hooks are
verified), `src/overview.rs`, `skill/COORDINATOR.md`, new role skills
`skill/{REVIEWER,CRITIC,DRAFTER,PICKUP}.md`, `docs/herdr-notes.md`, the `--plain` refusal on
`round open` and `term add`, and the assembled `scripts/acceptance` (every row of §4.3 wired
to a real command or an explicit NOT-RUN with the reason, the required-row list, the
PASS/FAIL/NOT-RUN report).

Scenarios from §1.3 that are yours: round open/review/merge with the crash-recovery table of
`tasks/ade/turns/06-pro.md` (V to H, missing manifest fails closed, stale review on a later
admission), ask/answer binding, board publish refused on failing text, talk input states.
Your `tick` entry point: `round::tick`.
## Setup and rules (same for every lane of this round)

- Repo `/Users/rolfie/projects/herdr-ade` (fork of eliasstravik/herdr-projects; `origin` =
  uguryildirim24/herdr-ade). Your worktree `/Users/rolfie/projects/herdr-ade/.worktrees/a3`,
  branch `lane/ade-rounds`, cut from A0's pin `20fd9e9c` (`lane/ade-contracts`: the rename to
  `herdr-ade`, `src/contracts.rs`, `src/plain.rs` with `plain/words.txt` and
  `plain/vocabulary.txt`, FakeRunner wiring in `src/runner.rs`/`src/scenarios.rs`, the
  `scripts/acceptance` skeleton). Read A0's report first:
  `/Users/rolfie/projects/herdr-ade/.worktrees/a0/.reports/ade-contracts-report.md`.
- Build: `CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/a3`,
  `DEVELOPER_DIR=/Library/Developer/CommandLineTools`, Rust 1.89, edition 2024.
- Never: `herdr plugin link` or `install`, `cargo install`, anything under `~/.config`,
  `~/.herdr-ade`, `~/.herdr-projects`, `~/.claude`, `~/.codex`, `~/.cursor`, the live herdr
  server (session `default`, socket in `~/.config/herdr`), another worktree, `git stash`,
  `git push`. Throwaway runs: `HERDR_ADE_ROOT=/var/tmp/ade-a3/root`,
  `XDG_CONFIG_HOME=/var/tmp/ade-a3/xdg`, `XDG_STATE_HOME=/var/tmp/ade-a3/state`, a throwaway
  herdr session `env -u CLAUDE_CODE_CHILD_SESSION /Users/rolfie/projects/herdr/.target/install/release/herdr --session ade-a3 server`
  (the fork's release build, 0.9.1-based, has `--parent` on `agent start`; stop it with
  `herdr --session ade-a3 session stop ade-a3` when done; never `herdr server stop`).
- Spec (read only): `/Users/rolfie/projects/herdr/tasks/SPEC-ADE.md` v3.1. §4.2 names your
  lane's owned files; §1.2 D1 to D18 the behaviour; §1.3 the FakeRunner scenarios; §4.3 the
  reviewer's acceptance rows (you do not run those; you make them runnable); §6 items 32
  to 35 the fixtures that are tests. Decisions: `/Users/rolfie/projects/herdr/tasks/ade/decisions.md`.
  Base plugin line references use `hp:` = `/Users/rolfie/projects/herdr-projects` at a4cdb0a.
- Seams, so four lanes merge without conflicts: you edit only your owned files. The two
  shared files take one marked block per lane and nothing else: in `src/main.rs` your `mod`
  lines between `// ade-rounds begin` and `// ade-rounds end`; in `src/cli.rs` your subcommand enum
  variants and match arms inside the same markers. A field you need in `src/contracts.rs`
  goes in its own commit `contracts(ade-rounds): <field>` that only adds (new fields carry
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

Write `.reports/ade-rounds-report.md` (git-ignored) in your worktree: what you read, what you
built per owned file with spec lines, your `tick` entry point if any, contract fields you
added, each gate with its output tail, "Left for the reviewer" with exact lines, what you
could not do and why, open questions numbered from 1 for SPEC-ADE §6, the final commit sha.

```
herdr pane report-metadata $HERDR_PANE_ID --source lane --token lane=ade-rounds --token done=1
herdr agent prompt hcoord "DONE ade-rounds .reports/ade-rounds-report.md <final commit sha>" || herdr agent prompt hcoord "DONE ade-rounds .reports/ade-rounds-report.md <final commit sha>"
```

If you must stop: `herdr agent prompt hcoord "WAITING ade-rounds <what>"`.
A turn that ends without one of these pushes is the one failure nothing catches.
