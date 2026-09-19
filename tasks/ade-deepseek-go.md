# Lane b1 — `ade-deepseek-go`: the DeepSeek row through OpenCode Go (SPEC-ADE §6 item 80)

Plain: after this, a lane that asks for DeepSeek runs inside pi on Rolf's OpenCode Go key,
the same login the Muse row uses, with no OpenCode CLI in the picture.

You are lane b1 of plugin round r2 in `herdr-ade`, Rolf's coordination plugin for his herdr
fork. Round r1 merged tonight (plugin `main` 2a383e29). Its reviewer deleted the direct
DeepSeek provider row because Rolf has no DeepSeek API key, and found that no
DeepSeek-through-OpenCode-Go row existed (verdict `tasks/reviews/code-plugin-r1.md`, item 80,
and its "Deleted" list, commit 98966c5, which shows the shape the direct row had). Rolf's
words: "I don't want the opencode cli, you should be able to put in my opencode go api key".

## Setup

- Repo `/Users/rolfie/projects/herdr-ade`. Worktree
  `/Users/rolfie/projects/herdr-ade/.worktrees/b1`, branch `lane/ade-deepseek-go` from
  `main` 2a383e29.
- Build: `export CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/b1`,
  `export DEVELOPER_DIR=/Library/Developer/CommandLineTools`; Rust 1.89 (`cargo +1.89.0`),
  edition 2024. GNU sed is first in PATH: edit with python or your editor, never `sed -i ''`.
- Throwaway everything under `/var/tmp/ade-b1/`: `HERDR_ADE_ROOT=/var/tmp/ade-b1/root`,
  `XDG_CONFIG_HOME=/var/tmp/ade-b1/xdg`, `XDG_STATE_HOME=/var/tmp/ade-b1/state`; pi only inside
  a throwaway prefix there (`herdr-pi setup` with that env, as `tasks/ade-pi.md` and the A5
  report describe). The herdr under test, if a row needs one, is
  `/Users/rolfie/projects/herdr/.target/review/release/herdr` (0.9.1) by absolute path
  (`HERDR_BIN_PATH`), never first on your PATH.
- Never push. Never merge. Never `git stash`. Never touch the root checkout or another
  worktree. Never `herdr plugin link`, `herdr plugin install`, `cargo install`. Never write
  under `~/.config`, `~/.herdr-ade`, `~/.pi`, `~/.local/bin`, `~/.claude`, `~/.codex`,
  `~/.cursor`. Never type a `/login`. Rolf's live server: never `herdr server stop`,
  `herdr server restart`, `herdr update`.
- Rolf's two standing rules: (1) no backward compatibility of any kind, no shims, fallbacks or
  legacy flags; (2) no new test unless it fails on a real defect you found; the existing table
  validation and doctor tests cover a new row.
- Start line (for a restart after GONE):
  `herdr agent start b1 --kind codex --pane <the b1 tab's pane> --parent w1F:p1 -- -c model=gpt-5.6-sol -c model_reasoning_effort=high --dangerously-bypass-approvals-and-sandbox`

## Spec (read only)

`/Users/rolfie/projects/herdr/tasks/pi/SPEC-pi.md` v2 §3.4 (providers and recipe rows) and
§3.5 (model ids); `/Users/rolfie/projects/herdr/tasks/SPEC-ADE.md` D2 (roles table), D17
(plain phrases R1 to R7) and §6 items 63, 65, 80, 93. In this repo: `src/pi/roles.rs`
(`pi_recipes()`, the enabled providers list, the Muse row on `opencode-go` as the model to
copy), `src/pi/doctor.rs`, `src/pi/launch.rs` (`validate_provider_column`), `skill/PI.md`,
`plain/vocabulary.txt`, `plain/words.txt`.

## The package

1. Add the recipe `pi_opencode_deepseek`: provider `opencode-go`, model id
   `deepseek-v4.1-flash` (verified on this Mac: `opencode models` lists
   `opencode-go/deepseek-v4.1-flash`, the id Rolf's DeepSeek lanes ran on tonight), the same
   args, env, ready timeout and thinking shape as the Muse row, enabled, with a plain phrase
   that passes the checker (R1 to R7) and fits A4's reason templates.
2. Put the row where SPEC-pi §3.4 meant DeepSeek to sit in the D2 roles table (the ordinary
   coding lane rows); if the spec's table names the old direct provider, the row now names
   `opencode-go`, and you say so in the report so the coordinator changes the spec text.
3. `herdr-pi check opencode-go` and `doctor` need no new row: the `opencode-go` login is
   already required. Verify; change nothing there unless the new recipe fails a check.
4. `skill/PI.md`: one line for the row under the OpenCode Go login (what a coordinator gets
   when it asks for DeepSeek). No other doc changes.
5. Nothing else. No OpenCode CLI path, no Zen provider, no DeepSeek direct provider, no Grok.

## Gates (all green before DONE; paste each command and its last lines in the report)

```
cargo +1.89.0 fmt --check
cargo +1.89.0 clippy --all-targets --locked -- -D warnings
cargo +1.89.0 test --locked
cargo +1.89.0 build --release --locked
HERDR_ADE_ROOT=/var/tmp/ade-b1/root XDG_CONFIG_HOME=/var/tmp/ade-b1/xdg XDG_STATE_HOME=/var/tmp/ade-b1/state $CARGO_TARGET_DIR/release/herdr-pi setup
HERDR_ADE_ROOT=/var/tmp/ade-b1/root XDG_CONFIG_HOME=/var/tmp/ade-b1/xdg XDG_STATE_HOME=/var/tmp/ade-b1/state $CARGO_TARGET_DIR/release/herdr-pi doctor
HERDR_ADE_ROOT=/var/tmp/ade-b1/root XDG_CONFIG_HOME=/var/tmp/ade-b1/xdg XDG_STATE_HOME=/var/tmp/ade-b1/state $CARGO_TARGET_DIR/release/herdr-pi check opencode-go
```

Doctor and check may fail only on the missing logins (fail-closed, item 63); the new row must
appear in doctor's recipe listing, if it has one, and must pass the table validation.

## Commits

Small, on `lane/ade-deepseek-go`: `feat(pi): DeepSeek row through OpenCode Go (item 80)`,
`docs(pi): the DeepSeek row in PI.md`. The final sha goes in the DONE line.

## Report and closing steps (verbatim, every turn ends with one of these, also on failure)

Write `.reports/ade-deepseek-go-report.md` (git-ignored) in your worktree: what you read, the
exact row you added and where, which roles rows now name it, each gate with its output tail,
what you could not do and why, open questions numbered from 1 for SPEC-ADE §6, the final
commit sha.

```
herdr pane report-metadata $HERDR_PANE_ID --source lane --token lane=ade-deepseek-go --token done=1
herdr notification show "ade-deepseek-go done" --body "lane b1" --sound done
herdr agent prompt hcoord "DONE ade-deepseek-go .reports/ade-deepseek-go-report.md <final commit sha>" || herdr agent prompt hcoord "DONE ade-deepseek-go .reports/ade-deepseek-go-report.md <final commit sha>"
```

If you must stop: `herdr agent prompt hcoord "WAITING ade-deepseek-go <what>"`.
A turn that ends without one of these pushes is the one failure nothing catches.
