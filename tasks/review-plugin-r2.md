# Code review + fix — plugin round r2 (one lane: `ade-deepseek-go`) on branch `review/plugin-r2`

Plain: one lane added the table row that makes "DeepSeek" mean DeepSeek 4.1 flash on Rolf's
OpenCode Go key inside pi. You check that row is right, fix what is wrong, and say MERGE or not.

You are the one reviewer of plugin round r2 in `herdr-ade`, Rolf's coordination plugin for
his herdr fork. Round r1 merged tonight (plugin `main` 2a383e29, verdict
`tasks/reviews/code-plugin-r1.md`). Lane b1 built package `ade-deepseek-go` on
`lane/ade-deepseek-go` (eda1352b, two commits) from SPEC-ADE §6 item 80. You merge it into
the candidate, inspect and **fix** it yourself, run every gate, and write one verdict.
Nothing reaches `main` before your verdict.

## Setup

- Repo `/home/agent/projects/herdr-ade`. Worktree
  `/home/agent/projects/herdr-ade/.worktrees/review`, branch `review/plugin-r2` from `main`.
- Build: `export CARGO_TARGET_DIR=/home/agent/projects/herdr-ade/.target/review`,
  `export DEVELOPER_DIR=/Library/Developer/CommandLineTools`; Rust 1.89 (`cargo +1.89.0`),
  edition 2024. GNU sed is first in PATH: edit with python or your editor, never `sed -i ''`.
- Throwaway everything under `/var/tmp/ade-review2/`: `HERDR_ADE_ROOT=/var/tmp/ade-review2/root`,
  `XDG_CONFIG_HOME=/var/tmp/ade-review2/xdg`, `XDG_STATE_HOME=/var/tmp/ade-review2/state`, a
  throwaway `HOME=/var/tmp/ade-review2/home` for the pi prefix the way b1's report shows. The
  herdr under test is `/home/agent/projects/herdr/.target/review/release/herdr` (0.9.1) by
  absolute path (`HERDR_BIN_PATH`), never first on your PATH: your closing pushes must reach
  Rolf's live server through `~/.local/bin/herdr` (0.9.0, session `default`). Any throwaway
  `claude` pane runs Haiku: `-- --model claude-haiku-4-5-20251001 --dangerously-skip-permissions`.
- Never push. Never merge into `main`. Never `git stash`. Never touch the root checkout or
  another worktree. Never `herdr plugin link`, `herdr plugin install`, `cargo install`. Never
  write under `~/.config`, `~/.herdr-ade`, `~/.pi`, `~/.local/bin`, `~/.claude`, `~/.codex`,
  `~/.cursor`. Never type a `/login`. Rolf's live server: never `herdr server stop`,
  `herdr server restart`, `herdr update`.
- Rolf's two standing rules: (1) no backward compatibility of any kind, no shims, fallbacks
  or legacy flags, unless the brief names the consumer; (2) no new test unless it fails on a
  real defect you found and fixed; every new test in your commits points at a row of your
  defects table.
- Start line (for a restart after GONE):
  `herdr agent start reviewer --kind claude --pane <the review tab's pane> --parent w1F:p1 -- --model claude-opus-5 --effort high --dangerously-skip-permissions`

## Steps

1. Read `tasks/ade-deepseek-go.md` (the lane's brief), `/home/agent/projects/herdr/tasks/pi/SPEC-pi.md`
   v2 §3.4 and §3.5, `/home/agent/projects/herdr/tasks/SPEC-ADE.md` D2, D17 and §6 items 63,
   65, 80, 93, and the lane's report pasted below.
2. `git merge lane/ade-deepseek-go` into `review/plugin-r2` (one merge commit; it should be
   clean).
3. Adversarial review, then fix in separate `review(ade-deepseek-go):` commits. Attack: the
   row's id, provider, model id (`deepseek-v4.1-flash` on `opencode-go`, the id Rolf's lanes
   ran on tonight, `opencode models` lists `opencode-go/deepseek-v4.1-flash`), args shape
   against the Muse row and `validate_provider_column`, thinking level and ready timeout,
   `enabled`, `start_time_allowed`, the plain phrase against R1 to R7 and A4's reason
   templates, its place in the ordinary coding role rows (does a lane that asks for DeepSeek
   get this row, and does the picker's fixed table see it), doctor's provider deduplication
   (one `opencode-go` row, not two), the PI.md line, and that nothing else changed (no
   OpenCode CLI, Zen, direct DeepSeek or Grok trace came back). Contract conformance:
   `#[serde(default)]` where the row's shape needs it, no `unwrap()`, no dependency added.
4. Gates, from the worktree with the env above, all green before the verdict:
   ```
   cargo +1.89.0 fmt --check
   cargo +1.89.0 clippy --all-targets --locked -- -D warnings
   cargo +1.89.0 test --locked
   cargo +1.89.0 build --release --locked
   HERDR_ADE_ROOT=/var/tmp/ade-review2/root XDG_CONFIG_HOME=/var/tmp/ade-review2/xdg XDG_STATE_HOME=/var/tmp/ade-review2/state HOME=/var/tmp/ade-review2/home $CARGO_TARGET_DIR/release/herdr-pi setup
   HERDR_ADE_ROOT=/var/tmp/ade-review2/root XDG_CONFIG_HOME=/var/tmp/ade-review2/xdg XDG_STATE_HOME=/var/tmp/ade-review2/state HOME=/var/tmp/ade-review2/home $CARGO_TARGET_DIR/release/herdr-pi doctor
   HERDR_ADE_ROOT=/var/tmp/ade-review2/root XDG_CONFIG_HOME=/var/tmp/ade-review2/xdg XDG_STATE_HOME=/var/tmp/ade-review2/state HOME=/var/tmp/ade-review2/home $CARGO_TARGET_DIR/release/herdr-pi check opencode-go
   ACC_DIR=/var/tmp/ade-review2/acc sh scripts/acceptance/run
   ```
   Doctor and check fail only on the three missing logins (fail-closed, item 63). The
   deterministic acceptance parts all pass as they did on r1's candidate. No live acceptance
   run is needed for this row unless a deterministic part fails.
5. Write `tasks/reviews/code-plugin-r2.md`: a 3-line verdict (MERGE / MERGE-AFTER-DECISION /
   REJECT), the gate table, the defects table (severity, file:line, what was wrong, what you
   changed, commit), and "Needs a decision" numbered from 101 (SPEC-ADE §6 continues there;
   item 100 is the SPEC-pi text that still names the old direct row, already recorded). Commit
   it on `review/plugin-r2` as `docs(review): plugin round r2 verdict`.

## Report and closing steps (verbatim, every turn ends with one of these, also on failure)

Write `/home/agent/projects/herdr-ade/.worktrees/review/.reports/review-plugin-r2-report.md`
(git-ignored, not committed): what you ran, what you did not run and why, the final commit
sha. Then:

```
herdr pane report-metadata $HERDR_PANE_ID --source lane --token lane=review-plugin-r2 --token done=1
herdr notification show "plugin review r2 done" --body "reviewer" --sound done
herdr agent prompt hcoord "DONE review-plugin-r2 .reports/review-plugin-r2-report.md <final commit sha>" || herdr agent prompt hcoord "DONE review-plugin-r2 .reports/review-plugin-r2-report.md <final commit sha>"
```

If you must stop for something outside the lane:

```
herdr pane report-metadata $HERDR_PANE_ID --source lane --token lane=review-plugin-r2 --token waiting="<what>"
herdr agent prompt hcoord "WAITING review-plugin-r2 <what>"
```

A turn that ends without one of these pushes is the one failure nothing catches.

## The lane's report, verbatim

### ade-deepseek-go (lane b1, `lane/ade-deepseek-go` eda1352b)

````markdown
# ade-deepseek-go report

## Read

- `tasks/ade-deepseek-go.md`, whole.
- `/home/agent/projects/herdr/tasks/pi/SPEC-pi.md` v2, especially sections 2, 3.4, 3.5, 5, and 6.2.
- `/home/agent/projects/herdr/tasks/SPEC-ADE.md` D2, D17 rules R1 to R7, and section 6 items 63, 65, 80, and 93.
- `/home/agent/projects/herdr/tasks/jev/SPEC-jev-picker.md`, the role lists and start-time rules needed to interpret the ordinary coding rows.
- `tasks/ade-pi.md`, `tasks/reviews/code-plugin-r1.md`, `tasks/reviews/code-plugin-r1-b.md`, and deletion commit `98966c5`.
- `src/pi/roles.rs`, the relevant provider/login paths in `src/pi/doctor.rs` and `src/pi/launch.rs`, `skill/PI.md`, and the plain word lists for the chosen phrase.

The old A5 report was not present at `/home/agent/projects/herdr-ade/.worktrees/a5/.reports/ade-pi-report.md`; the checked-in A5 reviewer reports and current setup code supplied the setup details.

## Package

The first ready-made row in `src/pi/roles.rs::pi_recipes()` is now:

```text
id: pi_opencode_deepseek
kind: pi
provider: opencode-go
model_family/model: deepseek-v4.1-flash
args: --provider opencode-go --model deepseek-v4.1-flash --thinking high --no-skills
env: []
ready_timeout_ms: 30000
enabled: true
start_time_allowed: true
plain: the cheap coding helper
```

It occupies the old DeepSeek row's first position among the ready-made D2/Jev recipes. It uses the Muse row's OpenCode Go provider, high thinking, enabled state, start-time shape, empty environment, and timeout. The plain phrase passes R1 to R7 and A4's rendered reason checks in the full test suite.

The row is available to ordinary start-time role `allowed` lists because `start_time_allowed = true`. No user-owned `[roles.<name>]` list is shipped by this package, so no specific role array was changed. `skill/PI.md` adds exactly one row telling a coordinator that DeepSeek means provider `opencode-go`, model `deepseek-v4.1-flash`, thinking `high`.

`opencode-go` was already an allowed provider and an enabled login because Muse uses it. Therefore `src/pi/doctor.rs` and `src/pi/launch.rs` needed no change. Doctor deduplicates the shared provider and checks it once.

## Commits

- `2b599a6 feat(pi): DeepSeek row through OpenCode Go (item 80)`
- `eda1352 docs(pi): the DeepSeek row in PI.md`

## Gates

Build environment:

```text
CARGO_TARGET_DIR=/home/agent/projects/herdr-ade/.target/b1
DEVELOPER_DIR=/Library/Developer/CommandLineTools
```

The throwaway pi gates also used `HOME=/var/tmp/ade-b1/home`, that home's `.local/bin` first on `PATH`, and `HERDR_BIN_PATH=/home/agent/projects/herdr/.target/review/release/herdr`. The setup-produced symlink was made only at `/var/tmp/ade-b1/home/.local/bin/pi`.

### 1

```text
cargo +1.89.0 fmt --check
```

Tail: no output; exit 0.

### 2

```text
cargo +1.89.0 clippy --all-targets --locked -- -D warnings
```

Tail:

```text
    Checking toml v0.9.12+spec-1.1.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.87s
```

Exit 0.

### 3

```text
cargo +1.89.0 test --locked
```

Tail:

```text
test every_recipe_phrase_passes_as_a_birth_sentence ... ok
test every_rendered_reason_passes_as_a_birth_sentence ... ok

test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.15s
```

The preceding suites were also green: 357 main tests, 41 `herdr-pi` tests, and 4 CLI tests. Exit 0.

### 4

```text
cargo +1.89.0 build --release --locked
```

Tail:

```text
   Compiling clap v4.6.7
    Finished `release` profile [optimized] target(s) in 11.55s
```

Exit 0.

### 5

```text
HERDR_ADE_ROOT=/var/tmp/ade-b1/root XDG_CONFIG_HOME=/var/tmp/ade-b1/xdg XDG_STATE_HOME=/var/tmp/ade-b1/state $CARGO_TARGET_DIR/release/herdr-pi setup
```

Tail:

```text
The pi folder is /var/tmp/ade-b1/root/pi/agent
Type this one line to put the wrapper on your login PATH:

ln -s /var/tmp/ade-b1/root/pi/bin/pi /var/tmp/ade-b1/home/.local/bin/pi

Then run `herdr-pi doctor` and `herdr-pi login`.
```

Exit 0. The exact pi pin installed under the throwaway ADE root.

### 6

```text
HERDR_ADE_ROOT=/var/tmp/ade-b1/root XDG_CONFIG_HOME=/var/tmp/ade-b1/xdg XDG_STATE_HOME=/var/tmp/ade-b1/state $CARGO_TARGET_DIR/release/herdr-pi doctor
```

Tail:

```text
[ok  ] Cursor: outside pi; native cursor lanes only, then retired
[FAIL] provider opencode-go: missing login: credentials_not_configured (run `herdr-pi login opencode-go`)
[FAIL] provider openai-codex: missing login: credentials_not_configured (run `herdr-pi login openai-codex`)
[FAIL] provider kimi-coding: missing login: credentials_not_configured (run `herdr-pi login kimi-coding`)
[ok  ] ~/.codex: no config.toml
[ok  ] ~/.pi: not present
herdr-pi: some checks failed
```

Exit 1, allowed by item 63. Every setup row passed; the only failures were the three deliberately absent throwaway logins. The DeepSeek row shares the single `opencode-go` provider check with Muse.

### 7

```text
HERDR_ADE_ROOT=/var/tmp/ade-b1/root XDG_CONFIG_HOME=/var/tmp/ade-b1/xdg XDG_STATE_HOME=/var/tmp/ade-b1/state $CARGO_TARGET_DIR/release/herdr-pi check opencode-go
```

Tail:

```text
    {
      "check": "login",
      "detail": "missing login: credentials_not_configured (run `herdr-pi login opencode-go`)",
      "level": "fail",
      "ok": false
    }
  ],
  "ok": false,
  "provider": "opencode-go"
}
```

Exit 1, allowed by item 63. All seven preceding readiness rows were `ok`; only the missing login failed.

## Could not do

- I did not type `/login` or copy a live credential. The brief forbids it, so the throwaway doctor and provider check remain fail-closed only on `credentials_not_configured`.
- The old A5 ignored report was absent; no required implementation or gate depended on it.

## Open questions for SPEC-ADE section 6

1. Coordinator spec follow-up: SPEC-pi still names the old direct `deepseek` provider and `pi_deepseek_flash` row in its provider, start-line, recipe, migration, and composition text. Those entries should name `pi_opencode_deepseek`, provider `opencode-go`, model `deepseek-v4.1-flash`, and high thinking. This lane did not edit the read-only spec.

## Final commit

`eda1352bb216560c8802e7732b2f88203dbde799`
````
