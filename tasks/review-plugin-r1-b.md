# Code review + fix — plugin round r1, part b: A4 `ade-picker` and A5 `ade-pi` on their lane branches

You are the second reviewer of the first build round of `herdr-ade`, Rolf's coordination
plugin for his herdr fork (SPEC-ADE v3.1). The round's surface is about 26,000 added
lines, so it has two reviewers. The first (`reviewer`, brief `tasks/review-plugin-r1.md`)
owns the merged candidate `review/plugin-r1`, every seam, the wiring, the gates, the
acceptance and the verdict. **You inspect and fix the two largest self-contained
packages on their own lane branches, in parallel, before they are merged**: A4
`ade-picker` (`lane/ade-picker` 0d3e706e, worktree `.worktrees/a4`, about 3,600 lines: the
Jev client `src/jev.rs` and the launch resolver `src/launch.rs`) and A5 `ade-pi`
(`lane/ade-pi` d6f71088, worktree `.worktrees/a5`, about 4,100 lines: the pinned pi library
`src/pi/`, the thin `herdr-pi` binary, the guard extension). When you push DONE, the
coordinator hands your two branch heads to the first reviewer, who merges them last.

## Setup

- Repo `/Users/rolfie/projects/herdr-ade`. You work in two worktrees:
  `/Users/rolfie/projects/herdr-ade/.worktrees/a4` (branch `lane/ade-picker`) with
  `CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/a4`, and
  `/Users/rolfie/projects/herdr-ade/.worktrees/a5` (branch `lane/ade-pi`) with
  `CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/a5` (both caches are warm).
  `export DEVELOPER_DIR=/Library/Developer/CommandLineTools`; Rust 1.89 (`cargo +1.89.0`),
  edition 2024. GNU sed is first in PATH: edit with python or your editor, never `sed -i ''`.
- Throwaway everything, under `/var/tmp/ade-revb/`: `HERDR_ADE_ROOT=/var/tmp/ade-revb/root`,
  `XDG_CONFIG_HOME=/var/tmp/ade-revb/xdg`, `XDG_STATE_HOME=/var/tmp/ade-revb/state`; pi only
  inside a throwaway prefix there, the way A5's report gate 5 shows. The herdr binary under
  test, if a row needs one, is the r2 candidate
  `/Users/rolfie/projects/herdr/.target/review/release/herdr` (prints `herdr 0.9.1`), reached
  by absolute path (`HERDR_BIN_PATH`), never first on your PATH: your closing pushes must
  reach Rolf's live server through `~/.local/bin/herdr` (0.9.0, session `default`). A
  throwaway herdr session, if you start one, runs with that candidate binary,
  `env -u CLAUDE_CODE_CHILD_SESSION` and the isolated XDG dirs, and is stopped with that same
  binary's `session stop <name>`. Any `claude` pane in it runs Haiku:
  `-- --model claude-haiku-4-5-20251001 --dangerously-skip-permissions` (Rolf, 19:15).
- Never push. Never merge anything. Never `git stash`. Never touch the root checkout, the
  `review` worktree or worktrees `a0` to `a3`. Never `herdr plugin link`, `herdr plugin
  install`, `cargo install`. Never write under `~/.config`, `~/.herdr-ade`, `~/.pi`,
  `~/.local/bin`, `~/.claude`, `~/.codex`, `~/.cursor`. Never type a `/login` anywhere.
  Never a live Jev call except the one the A4 brief allows (the doctor's models probe).
  Rolf's live server: never `herdr server stop`, `herdr server restart`, `herdr update`.
- Start line (for a restart after GONE):
  `herdr agent start reviewer-b --kind claude --pane <the reviewer-b tab's pane> --parent w1F:p1 -- --model claude-opus-5 --effort high --dangerously-skip-permissions`

## Steps

1. Read, in `/Users/rolfie/projects/herdr`: `tasks/jev/SPEC-jev-picker.md` v2 whole,
   `tasks/pi/SPEC-pi.md` v2 whole, `tasks/SPEC-ADE.md` §4.2, §4.3, D2, D7, D15, D17 and §6
   items 44 to 52 and 61 to 66 (the two lanes' questions with the coordinator's readings).
   Here: the briefs `tasks/ade-picker.md` and `tasks/ade-pi.md`, and the two reports pasted
   below. Do not read the other four lanes' code beyond what these two call.
2. A4 `ade-picker`, in `.worktrees/a4`: adversarial review, then fix in separate
   `review(ade-picker):` commits on `lane/ade-picker`. Attack hardest: `resolver = "off"` is
   the default and shadow mode never launches anything; the daily cap file
   `<project>/.state/jev-calls/<utc-day>.count` (item 49) counts every call and refuses at
   the cap; unknown recipe fields and bad PROJECT.md pins are refused; no test makes a live
   Jev call (FakeRunner everywhere); a model named in the task warns, never overrides; the
   gate table is fixed and a gate's reason clause is plain (D17 R1 to R7); `compact_reason`
   (item 44) is what `ade_last` publishes; `Launch` keeps every `LaunchRecipe` field name;
   `resolve_launch` returns `attempt = 1` and an empty `brief_hash` (item 48); kinds are read
   from the fork's `agent start --help` and `chatgpt` never resolves (item 51); the 3 s models
   probe (item 52) times out cleanly. Apply item 47: an inline D2 role without `plain` gets a
   doctor warning, not a refusal. Item 65 (the Muse model id): take the coordinator's reading,
   the OpenCode Go id `muse-spark-1.3-contributor`, unless the coordinator sends Rolf's
   answer; say which you took. Leave the hidden `picker-doctor` verb in place (the first
   reviewer deletes it when the doctor rows are wired, item 50).
3. A5 `ade-pi`, in `.worktrees/a5`: adversarial review, then fix in separate
   `review(ade-pi):` commits on `lane/ade-pi`. Attack hardest: `defaultProjectTrust: "never"`
   and no `--approve` anywhere in the start line or the wrapper; the guard extension only
   ever pushes `ha waiting`, never `ha done`, and clears on typing; the wrapper
   `~/.local/bin/pi` is the only pi on the login PATH and the probe is `whence -va pi`
   (item 61); prefix isolation (nothing under `~/.pi`, no global npm root); no Cursor route
   (`pi_cursor_forbidden`, no `@cursor/sdk` in `node_modules`); the r2 strip list tokens
   (`--session`, `--fork`, `--no-session`, `-c` boolean, no `-s`); `credentials_not_configured`
   refuses a launch before a pane exists (never a lane that waits for its first prompt);
   the four provider rows are fail-closed (item 63); no Grok row shipped, two withheld
   (item 66); `herdr-pi setup` writes only inside the throwaway prefix. Run A5's deterministic
   §7 rows yourself on the throwaway prefix (T1, T2, T9, T10, T11, T13 as its report lists
   the exact commands, and `sh src/pi/testdata/guard-check.sh` for T7's guard half); T3 and
   T7's pane half need a throwaway herdr session with the candidate binary: run them if you
   can in your time, else mark NOT-RUN with the reason. T4 to T6, T8, T12, T14 need logins or
   the installed r2 fork: NOT-RUN, say so.
4. Contract conformance in both: names, error codes (`gate_duplicate`,
   `recipe_kind_unknown`, `role_args_missing`, `pi_cursor_forbidden`,
   `credentials_not_configured`), exit codes, TOML shapes, `#[serde(default)]` on every
   added field, no `unwrap()` in production code, no dependency added without a reason,
   nothing sensitive written to disk (keys, tokens, auth files) outside the isolated prefix.
   Missing tests the briefs require count as defects. Keep each lane's structure unless it is
   wrong. Do not touch files outside each lane's owned list except its own report; the seams
   (`src/main.rs`, `src/cli.rs`, `src/contracts.rs` blocks) are the first reviewer's.
5. Gates, per lane, in its worktree with its build dir, all green before DONE:
   ```
   cargo +1.89.0 fmt --check -- <the lane's owned files>      (whole-crate fmt fails on base files; the first reviewer formats the crate once)
   cargo +1.89.0 clippy --all-targets --locked -- -D warnings
   cargo +1.89.0 test --locked
   cargo +1.89.0 build --release --locked
   ```
   plus, for a4, the throwaway `doctor` (its report shows the env; ok except herdr 0.9.0 if you
   point it at the installed 0.9.0, ok with the candidate binary) and, for a5,
   `$CARGO_TARGET_DIR/release/herdr-pi setup`, `doctor` and `check <provider>` on the throwaway
   prefix (doctor red only on the four missing logins).
6. Write `tasks/reviews/code-plugin-r1-b.md` and commit it on `lane/ade-pi` (the first
   reviewer's merge carries it into the candidate): a 3-line verdict per package (MERGE /
   MERGE-AFTER-DECISION / REJECT), the gate table per lane with counts, the pi §7 rows table
   (PASS, FAIL, NOT-RUN with the reason), the defects table (severity, file:line, what was
   wrong, what you changed, commit), which reading you took on item 65, and "Needs a
   decision" numbered from 90 (the first reviewer numbers from 77; the coordinator merges
   both lists into SPEC-ADE §6). Commit as `docs(review): plugin round r1 part b verdict
   (A4, A5)`.

## Report and closing steps (verbatim, every turn ends with one of these, also on failure)

Write `/Users/rolfie/projects/herdr-ade/.worktrees/a5/.reports/review-plugin-r1-b-report.md`
(git-ignored, not committed): what you ran per lane, what you did not run and why, the final
commit sha of `lane/ade-picker` and of `lane/ade-pi` (both, on their own lines). Then, with
the `lane/ade-pi` sha as the DONE sha:

```
herdr pane report-metadata $HERDR_PANE_ID --source lane --token lane=review-plugin-r1-b --token done=1
herdr notification show "plugin review b done" --body "reviewer-b" --sound done
herdr agent prompt hcoord "DONE review-plugin-r1-b .reports/review-plugin-r1-b-report.md <lane/ade-pi final sha>" || herdr agent prompt hcoord "DONE review-plugin-r1-b .reports/review-plugin-r1-b-report.md <lane/ade-pi final sha>"
```

If you must stop for something outside the lane:

```
herdr pane report-metadata $HERDR_PANE_ID --source lane --token lane=review-plugin-r1-b --token waiting="<what>"
herdr agent prompt hcoord "WAITING review-plugin-r1-b <what>"
```

A turn that ends without one of these pushes is the one failure nothing catches.

## The two lanes' reports, verbatim

### ade-picker (lane a4, `lane/ade-picker` 0d3e706e)

````markdown
# Lane a4 report: ade-picker

Final commit sha: `0d3e706e302c7120d8bd94533579c888637bfb94`
Branch: `lane/ade-picker`
Worktree: `/Users/rolfie/projects/herdr-ade/.worktrees/a4`
Toolchain: `cargo +1.89.0`, `DEVELOPER_DIR=/Library/Developer/CommandLineTools`,
`CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/a4`.

## What I read

- `tasks/ade-picker.md` (this brief), `tasks/SPEC-ADE.md` v3.1 §1.2 D1–D18, §1.3, §2, §4.2
  A1–A3, §4.3, §6 items 32–35 and 36–42
- `/Users/rolfie/projects/herdr/tasks/jev/SPEC-jev-picker.md` v2 in full (especially §2, §3,
  "Design (normative)", "Plugin touch points by file", §5)
- `tasks/ade/decisions.md`
- A0's report `.worktrees/a0/.reports/ade-contracts-report.md`
- `src/contracts.rs`, `src/plain.rs`, `src/runner.rs`, `src/scenarios.rs`, `src/cli.rs`,
  `src/main.rs`, `src/project.rs`, `src/paths.rs`, `src/herdr.rs`, `src/doctor.rs`,
  `src/tests/cli.rs`
- The check b scripts (`tasks/jev/turns/02-opus-b/rerun_scrubbed.py`) and one cached TypeSafe
  response from jevtest for the exact request/response shape (`answers.<id>.noul`, `model`)
- `herdr agent start --help` on the fork release build: `[possible values: pi, claude, codex,
  gemini, cursor, devin, agy, cline, omp, mastracode, opencode, copilot, kimi, kiro, droid, amp,
  grok, hermes, kilo, qodercli, qwen, maki, muse]` (no `chatgpt`, no `dsh`)

## Commits

1. `cac0215` `contracts(ade-picker): picker types and launch fields`
2. `8f12947` `feat(picker): Jev client and launch resolver (SPEC-jev-picker v2)`
3. `9a139fd` `contracts(ade-picker): bool assert in the picker round-trip test`
4. `6c89c34` `test(picker): plain fixtures for the reason sentences (SPEC-jev-picker §3)`
5. `df34e03` `fix(picker): refuse unknown recipe fields and bad PROJECT.md pins`
6. `0d3e706` `fix(picker): warn when the task names a model, record the ade_last sentence`

## What I built, per owned file

### `src/contracts.rs` (additive, 199 lines added, 0 removed at `cac0215`; one line changed at `9a139fd`; `df34e03` adds `deny_unknown_fields`)

Added, with `#[serde(default)]` on every new field:

- `ResolverMode` (`off|shadow|jev|pin`) — SPEC-jev-picker v2 §2 Config, §2 Output
- `CostClass` (`default|sideways|upgrade`) — §2 Design C
- `Recipe { kind, args, env, ready_timeout_ms, provider, cost, enabled, plain }` —
  "Plugin touch points by file" row 1; `provider` reserved for the pi move (question 25)
- `GateCriteria { true, false }` and `Gate { recipe, cost, threshold, instructions, criteria }`
  — §2 Design C; `threshold: Option<f64>` so a gate can use its class floor
- `Launch`: the D2 row (`kind, args, env, ready_timeout_ms, policy_hash, attempt, brief_hash`)
  plus `recipe_id`, `resolver`, `gate`, `gate_p`, `jev_pick`, `jev_confidence`,
  `jev_probabilities`, `reason`, `compact_reason`, `fallback`, `jev_model`,
  `jev_input_tokens`, `jev_prompt_hash`, `excerpt_version` — §2 Output, D17 item 14
- JSON+TOML round-trip tests for all of them; `Recipe`/`Gate`/`GateCriteria` refuse unknown
  fields (D2 "unknown fields are rejected"), asserted in
  `contracts::tests::resolver_mode_and_cost_class_parse_from_lowercase`.

`compact_reason` is not in the spec's field list; I added it because §3 Publication says the
ticker sets `ade_last` to "<job> runs on <plain>" while D2 forbids rebuilding a launch from
live settings. The sentence is stored on the record.

### `src/jev.rs` (new, 1105 lines)

- Excerpt transform and scrub (§2 Input): drops `herdr agent start` lines, `Start line` lines and
  fenced code; collapses blank runs; cuts at 2,200 characters on a word boundary; case-insensitive
  whole-token replacement with `[agent]` and collapse of runs. Scrub list = every recipe kind,
  every `--model`/`model=` value and its word pieces, every codex `key=value` arg, every recipe
  `plain` phrase, the fixed list `grok opus fable sonnet astra sol gemini antigravity agy kimi
  muse claude codex cursor opencode dsh pi xhigh extra-high`, effort words (`HIGH`, `xhigh`,
  `max`, `medium`, `minimal`, `extra high`, …) and permission flags. `EXCERPT_VERSION = 1`.
- `names_a_model`: the longest recipe kind or model word the raw task names, effort words and
  permission flags excluded (question 16 warning).
- `languages`: extensions in the task, at most 8, sorted.
- `build_state` and `questions`: one Noul per gate keyed by recipe id; state carries only
  `task, title, sentence, role, round, project{name,goal}, repo, languages, policy`.
- `request_body`, `prompt_hash` (instructions + criteria + excerpt version), pin `jev-1.13.0`.
- `call`: `/usr/bin/curl --silent --show-error --max-time <s> --config -` through `Runner`; the
  key reaches the child only on stdin; total budget 3 s including one retry; retry only on
  429/529 and curl exit 6/7/35; never retry 401/402/422 or other statuses; the body is stdout
  minus its last line, which is the status from `write-out`.
- `parse_body`: response `model` must equal the pin (`ModelMismatch`), every answer must be a
  Noul object with a number (`Malformed`), `usage.input_tokens` optional.
- `fired_gate`: highest Noul at or over its threshold; a tie returns `None` (default).
- Key: `TYPESAFE_API_KEY`, else `~/.config/typesafe/api_key`; `key_report` carries source, path,
  mode and group/other readability for the doctor; the key value is never logged.
- `models_probe`: the doctor's `GET /v1/models`, the only live call.
- 15 unit tests with the FakeRunner: 200, 429→200, 401 (one attempt), timeout (one attempt),
  connect error → retry, spawn failure (`NoCurl`), malformed, model mismatch, Noul tie, curl
  stdin escaping, key report, transform, languages, state, prompt hash.

### `src/launch.rs` (new, ~1990 lines)

- `parse_picker_config(config_dir, opted_in)` — §2 Config: `[roles] resolver`, `jev_model`,
  `jev_timeout_ms`, `jev_daily_cap`, `floor{sideways,upgrade}`; `[recipes.<id>]`; per role
  `default`, `allowed`, `escalate`, `gates`. Round-one inline D2 roles become `<role>_inline`
  with `default == allowed == ["<role>_inline"]`; an inline row plus a reference is
  `role_form_mixed`; a role without a default is `role_default_missing`. Defaults: `off`,
  `jev-1.13.0`, 3000 ms, 200/day, floors 0.50/0.70. `policy_hash` = SHA-256 over the resolver
  settings, recipes, role lists, gates and thresholds, `jev_model`, the excerpt version and the
  scrub fingerprint (Design norm 8, D11).
- `validate_config` — §2 Validation: kind is in `herdr agent start --help`
  (`recipe_kind_unknown`); claude Opus/Fable must have `--effort high`
  (`recipe_effort_forbidden`); claude/agy need `--dangerously-skip-permissions` (or the bypass
  flag), cursor needs `--force`/`--yolo` (`recipe_permission_missing`); default in allowed,
  every allowed id exists and is enabled, every gate is in allowed, not the default, has a cost
  class, a threshold in (0,1], both criteria and instructions non-empty and only one gate per
  recipe; every recipe `plain` phrase and every rendered reason template passes `plain::check`
  as a birth sentence (`recipe_reason_not_plain`). Roles `pro` and `coordinator` never resolve
  and are skipped; a `[recipes.*]` row of kind `chatgpt` is still refused on the r2 fork.
- `resolve_launch(ctx, project, input)` — §3 steps 1–8, in order: `--recipe` pin
  (`recipe_unknown`, `recipe_not_allowed`; `resolver = pin`, `fallback = override`); PROJECT.md
  pin (`role_args_missing` when the kind changes without args, `recipe_kind_unknown` for an
  unknown kind; `fallback = project`); validation; the model-name warning; the pair filter;
  `off|not_opted_in|single_row|no_gate`; key; the daily cap; one POST; acceptance; the config
  re-read (`config_changed`). No project lock is held across the HTTP call; the caller must not
  hold it either (documented at the module top).
- Pair filter (§2 Pairs): drops every allowed recipe whose `--model`/`model=` value equals the
  sibling's; if it removes the default, the first remaining allowed row in file order is the
  default; none left is `dialogue_same_model`.
- Daily cap: `<project>/.state/jev-calls/<utc-day>.count`, read and incremented under the
  project lock; a spent cap returns `daily_cap` without a POST.
- Acceptance: `http_<code>`, `timeout`, `no_curl`, `no_key`, `malformed`, `model_mismatch`,
  `error` (a gate id missing from `answers`), `not_allowed`, `no_gate`, `daily_cap`,
  `config_changed`; shadow records `jev_pick` and launches the default with `fallback = shadow`.
- Reason templates (fixed, plain-checked at load): picked, default by choice, pinned, fallback,
  shadow; job nouns (lane/reviewer/critic/drafter/research); clauses for the five shipped
  non-default recipes; `compact_reason` for `ade_last` (≤80 characters).
- `doctor_rows` — §5: mode; key source; key file mode; curl `--version`; the live
  `GET /v1/models` only when the resolver is not `off`; table validation; `zsh -lic
  'command -v <exe>'` per enabled kind; the Codex-quota reminder when a `codex_*` recipe is in a
  list; the Astra-enabled note. `picker_doctor` prints them with doctor's three marks.
- 21 unit tests: off, 200 pick, 200 low Noul, 200 other model, 429→200, timeout/401/malformed,
  daily cap, shadow, `--recipe` pin, PROJECT.md pin (including an empty plain and an unknown
  kind), `recipe_not_allowed`, pair filter, validation refusals, inline D2 role, plain templates
  and the compact form, policy hash moves on a threshold change, doctor rows.

### `src/main.rs` and `src/cli.rs` (marked blocks only)

```rust
// ade-picker begin
#[allow(dead_code)]
mod jev;
#[allow(dead_code)]
mod launch;
// ade-picker end
```

and in `cli.rs` a hidden `picker-doctor` verb that calls `crate::launch::picker_doctor(&ctx)`.
The `#[allow(dead_code)]` matches A0's pattern for not-yet-wired modules; the reviewer can drop
it once `threads.rs` and `doctor.rs` call in. The hidden verb can be deleted once A1's `doctor`
prints the rows.

### `tests/picker_plain.rs` (new) and `plain/vocabulary.txt`

`picker`, `web`, `spec`, `engine`, `draft`, `lookup`, `strongest` were added to
`plain/vocabulary.txt` (sorted, lowercase, unique) because the spec's recipe phrases and reason
clauses need them. `tests/picker_plain.rs` compiles A0's real `src/plain.rs` and
`src/contracts.rs` with `#[path]`, checks every day-one recipe phrase, every rendered reason
template and every compact form as a birth sentence, asserts the templates are the ones
`src/launch.rs` writes (drift guard), and proves a bad fixture still fails.

### `skill/PICKER.md` (new)

How a coordinator pins `--recipe` (with examples), what the record stores, the modes, the daily
ceiling and what the picker never does.

## Tick entry point

None. The picker runs at launch time (`resolve_launch`); nothing runs from the ticker.

## Gates

All on `0d3e706` unless stated. Toolchain `cargo +1.89.0`.

### `cargo fmt --check`

Exit 1, only the ten unowned files A0 named (pre-existing wrapping on main; I formatted only my
own files). My files (`src/jev.rs`, `src/launch.rs`, `src/contracts.rs`, `tests/picker_plain.rs`,
my blocks in `main.rs`/`cli.rs`) are clean:

```
Diff in .../src/adopt.rs
Diff in .../src/coordinator.rs
Diff in .../src/doctor.rs
Diff in .../src/inbox.rs
Diff in .../src/lifecycle.rs
Diff in .../src/project.rs
Diff in .../src/remote.rs
Diff in .../src/thread.rs
Diff in .../src/threads.rs
Diff in .../src/ticker.rs
```

Note for the coordinator: while running this lane, one `cargo +1.89.0 fmt -- src/contracts.rs`
invocation reformatted the whole crate in my working tree at 17:17. I restored those ten files
to the branch tip with `git checkout --` and never committed the crate-wide format.

### `cargo clippy --all-targets --locked -- -D warnings`

Exit 0.

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.04s
```

### `cargo test --locked`

Exit 0. Unit 209, `tests/cli.rs` 4, `tests/picker_plain.rs` 29 (including A0's plain and
contracts tests recompiled by `#[path]`).

```
test result: ok. 209 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.59s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.18s
test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.22s
```

The picker's §1.3 scenario tests are covered: 200 pick, 200 low confidence, 200 other model,
429 then 200, timeout, 401, malformed, daily cap.

### `cargo build --release --locked`

Exit 0. Binary `0.1.0+0d3e706.1789768891` at
`/Users/rolfie/projects/herdr-ade/.target/a4/release/herdr-ade`.

```
    Finished `release` profile [optimized] target(s) in 3.71s
```

### Throwaway `doctor`

Throwaway tree `/var/tmp/ade-a4` (`HERDR_ADE_ROOT=/var/tmp/ade-a4/root`,
`XDG_CONFIG_HOME=/var/tmp/ade-a4/xdg`, `XDG_STATE_HOME=/var/tmp/ade-a4/state`,
`HOME=/var/tmp/ade-a4/home`, `HERDR_BIN_PATH` = the fork release build). The config is the
spec's day-one table (`resolver = "shadow"`, 7 recipes, 5 roles). `HA plain check` of every
phrase passes inside `validate_config`.

`picker-doctor` (the picker rows) with the key read from the throwaway key file:

```
[ok  ] picker: resolver is shadow; 7 recipe(s), 5 role(s)
[ok  ] picker key: at /var/tmp/ade-a4/home/.config/typesafe/api_key
[ok  ] picker key mode: 600
[ok  ] curl: curl 8.7.1 (x86_64-apple-darwin26.0) libcurl/8.7.1 (SecureTransport) LibreSSL/3.3.6 …
[ok  ] picker models: GET /v1/models answered 200
[ok  ] picker recipes: 7 recipe(s) valid
[ok  ] kind agy: /Users/rolfie/.local/bin/agy
[ok  ] kind claude: /Users/rolfie/.local/bin/claude
[ok  ] kind codex: /opt/homebrew/bin/codex
[ok  ] kind cursor: /Users/rolfie/.local/bin/cursor-agent
[warn] codex quota: Codex usage is not visible to the plugin; disable the recipe by hand when …
exit=0
```

The plain `doctor` verb also ran in the same tree (exit 1: the fork binary reports `0.9.0`, the
pre-install state; A0's item 7 and SPEC item 42):

```
[FAIL] herdr: 0.9.0 (/Users/rolfie/projects/herdr/.target/install/release/herdr); 0.9.1 or later is required
[ok  ] session: /Users/rolfie/.config/herdr/herdr.sock (name: -)
[ok  ] git: git version 2.54.0 (Apple Git-157)
[ok  ] ssh: OpenSSH_10.3p1, LibreSSL 3.3.6
[ok  ] rsync: rsync  version 3.5.0  protocol version 32
[ok  ] gh: gh version 2.97.0 (2026-07-31)
[warn] gh auth: You are not logged into any GitHub hosts. …
[ok  ] root: 0 project(s)
[warn] ticker: not running
herdr-ade: some checks failed
exit=1
```

Live-call accounting: 4 `GET /v1/models` probes in total (two on `6c89c34`, two on the final
binary; env-key and file-key branches), all HTTP 200. **Zero System One inference calls**, so the
cost for this lane is **$0.00** against the $0.05 cap. The key file was copied into the
throwaway home for the file path, read only by the throwaway doctor, and deleted afterwards;
nothing was written outside `/var/tmp/ade-a4`. The throwaway herdr session `ade-a4` was never
started, so there was nothing to stop. `herdr status server` (read-only) was called once by the
plain `doctor` gate, the same call A0's gate makes.

## Left for the reviewer (exact lines)

1. `src/contracts.rs`, `struct ThreadRecord` line 59 — carry the picker fields on the launch:
   ```rust
   pub launch: Launch,
   ```
   (today `pub launch: LaunchRecipe,`). Every `LaunchRecipe` field keeps its name and meaning, so
   every existing read of `thread.launch.kind/args/env/ready_timeout_ms/policy_hash/attempt/brief_hash`
   still compiles.
2. `src/threads.rs`, in `thread start`, before the worktree and tab exist and **not** holding
   the project lock (SPEC-jev-picker §3 step 5):
   ```rust
   let launch = crate::launch::resolve_launch(
       &ctx,
       &project,
       &crate::launch::ResolveInput {
           role: role.as_deref().unwrap_or("lane"),
           task: &task,
           recipe: recipe.as_deref(),
           project_pin: /* PROJECT.md kind/args pin for this role, else None */ None,
           sibling: None,
           opted_in: /* PROJECT.md `jev = true`, question 13 */ false,
           round: round.as_deref(),
           title: Some(&title),
           sentence: Some(&plain),
           repo: /* repository basename, else None */ None,
           policy: None,
       },
   )?;
   ```
   Then set `launch.brief_hash` after the brief commit (D9) and write the record under the lock.
   The `--agent <kind>` without `--recipe` refusal (`role_args_missing`, decision 13) stays on the
   verb: `resolve_launch` never sees `--agent`.
3. `src/cli.rs`, `ThreadCommand::Start`:
   ```rust
   /// Which allowed set and gates the picker sees (default: lane)
   #[arg(long, value_name = "NAME")]
   role: Option<String>,
   /// Pin this recipe; the picker is skipped
   #[arg(long, value_name = "ID")]
   recipe: Option<String>,
   ```
   and pass both through `StartArgs`.
4. `src/doctor.rs`, in `report` after the tool rows:
   ```rust
   match crate::launch::doctor_rows(ctx) {
       Ok(rows) => {
           for row in rows {
               check(&mut out, row.ok, &row.label, row.detail.clone());
           }
       }
       Err(error) => check(&mut out, Some(false), "picker", format!("{error:#}")),
   }
   ```
   Then delete my hidden `picker-doctor` block in `src/cli.rs` and drop the two
   `#[allow(dead_code)]` lines in `src/main.rs`.
5. `src/ticker.rs` — launch from the record only (`hp:src/ticker.rs:456` and `:593` today read
   live safety args):
   ```rust
   herdr.agent_start(&t.agent_name, &t.launch.kind, &t.pane_id, &t.launch.args)?;
   ```
   and publish `ade_last` from the stored sentence:
   ```rust
   workspace_token("ade_last", &t.launch.compact_reason)
   ```
6. Round and dialogue verbs (A3's modules): `resolve_launch` for `reviewer`, `drafter`, then
   `critic` with `sibling: Some(&drafter_launch)`. `pro` and `coordinator` never call it.
7. PROJECT.md: add the `jev = true` opt-in to `Settings` (question 13, default off) and pass it
   as `opted_in`. Until question 21 is answered, PROJECT.md pins `kind`/`args` only.

## What I could not do

1. Wire the integration into `project.rs`, `threads.rs`, `thread.rs`, `ticker.rs`, `doctor.rs`,
   the round/dialogue verbs and the `--role`/`--recipe` flags: those files are the reviewer's
   per the brief; exact lines are above.
2. Run a real gate pick against live Jev (by design: the brief allows one live call, and that is
   the doctor's `GET /v1/models`; every resolve test uses the FakeRunner). No System One call was
   made, so no behaviour was verified against the live model.
3. Make crate-wide `cargo fmt --check` exit 0 without touching unowned files (pre-existing on
   main; A0 issue 1, SPEC item 36).
4. `XDG_CONFIG_HOME` isolation: `paths.rs` still uses `$HOME/.config/herdr-ade` (A0 issue 4), so
   the throwaway doctor used a throwaway `HOME` and a copied key file instead.
5. The `command -v` row maps every kind to its own name except `cursor` → `cursor-agent`; the
   fork's `--help` does not print the executable per kind.

## Open questions (numbered from 1 for SPEC-ADE §6)

1. `compact_reason` is a new `Launch` field not in SPEC-jev-picker's Output list. I added it so
   the ticker can publish `ade_last` without rereading live config (D2, D17 item 14). Keep it, or
   move the sentence elsewhere?
2. A PROJECT.md pin has no `plain` phrase; I fall back to the role default's phrase ("You chose
   the usual coding helper for this task."). Should front matter carry a `plain` for a pinned
   row instead?
3. A gate's id is its recipe id, so two gates on one recipe are refused (`gate_duplicate`). A
   custom gate recipe without a fixed reason clause gets the default sentence rather than a
   refusal. Acceptable, or should both be config errors?
4. An inline D2 role without `plain` gets the shipped phrase "the usual helper". Should the
   config be refused instead (`recipe_reason_not_plain`)?
5. `resolve_launch` returns `attempt = 1` and an empty `brief_hash`; the caller fills the hash
   after the brief commit. Confirm that ordering with D9.
6. The daily-cap file lives at `<project>/.state/jev-calls/<utc-day>.count`. Is that the right
   home for a per-project, per-UTC-day counter?
7. The hidden `picker-doctor` verb exists only until A1's `doctor` prints `doctor_rows`; delete it
   then?
8. The r2 fork's `herdr agent start --help` has no `chatgpt` or `dsh`. An inline `[roles.pro]`
   with kind `chatgpt` is skipped from kind validation (it never resolves), but a
   `[recipes.<id>]` row with those kinds is refused `recipe_kind_unknown`. Right for D7/D15?
9. `doctor`'s models probe uses a 3 s budget (the spec names none). Keep 3 s?
````

### ade-pi (lane a5, `lane/ade-pi` d6f71088)

````markdown
# Lane a5 report: ade-pi

Final commit sha: `d6f71088461ebc3910071d5acf04adc625e8adcb` (short `d6f7108`)
Branch: `lane/ade-pi`
Worktree: `/Users/rolfie/projects/herdr-ade/.worktrees/a5`
Toolchain: `rustc 1.89.0` (`cargo +1.89.0`), edition 2024,
`CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/a5`,
`DEVELOPER_DIR=/Library/Developer/CommandLineTools`.

Final commit: `d6f7108 feat(pi): crate-side runner adapter for check and doctor rows`.

Commits on top of A0's `040f755` (merge of `main`):

1. `4594e01 feat(pi): the pinned pi library for herdr-ade (SPEC-pi v2 §3)`
2. `6be7b03 feat(pi): thin herdr-pi binary with its own [[bin]] entry (SPEC-pi v2 §3)`
3. `f0b7018 feat(pi): register the pi module in the herdr-ade binary`
4. `294c19b docs(pi): what a coordinator types, and the one-time logins (SPEC-pi v2 §2, §3.4)`
5. `d6f7108 feat(pi): crate-side runner adapter for check and doctor rows`

## What I read

- `tasks/ade-pi.md` (this brief).
- SPEC-pi v2 `/Users/rolfie/projects/herdr/tasks/pi/SPEC-pi.md`: §3 whole
  (3.1 to 3.10), §4, §7 acceptance, §8 open questions, fold notes.
- A0's report `.worktrees/a0/.reports/ade-contracts-report.md` and A0's pin
  (`src/contracts.rs`, `src/plain.rs`, FakeRunner wiring, acceptance skeleton).
- SPEC-ADE v3.1 §4.2 (my owned files), §1.2 D1 to D18, §1.3 (FakeRunner
  scenarios), §4.3 (reviewer rows), §6 items 32 to 35.
- `tasks/ade/decisions.md` rows 17:50, 18:05, 18:30, 19:25.
- `tasks/pi/research-a.md` (pin, provider catalog, mock provider recipe),
  `tasks/pi/turns/02-opus-a.md` §3.7 guard prototype.
- The fork asset `/Users/rolfie/projects/herdr/src/integration/assets/pi/herdr-agent-state.ts`
  and the isolated pi 0.85.1 extension docs
  (`/var/tmp/pi-research-a/npm/node_modules/@earendil-works/pi-coding-agent/docs/extensions.md`)
  and type declarations (`dist/core/extensions/types.d.ts`, `dist/core/event-bus.d.ts`,
  `dist/core/exec.d.ts`) for the extension API: `pi.on`, `pi.events.emit`,
  `pi.exec`, `agent_end.messages[].stopReason`/`errorMessage`,
  `after_provider_response.status`, `ui_prompt_start/end`, `input`.

## What I built, per owned file (spec lines)

### `src/pi/mod.rs` (§3.1, §3.2, §3.3)

- Constants: `PI_PACKAGE = @earendil-works/pi-coding-agent`, `PI_VERSION = 0.85.1`,
  `MIN_NODE = 22.19.0`, `GUARD_FILE`, `GUARD_MARKER = herdr-pi-guard:version=1`,
  `HERDR_EXTENSION_FILE`.
- `Env` (home + vars, testable), `Layout` (every path under one root:
  `npm/`, `bin/pi`, `agent/{settings,models,auth,trust}.json`,
  `agent/extensions/`, `agent/sessions/`, `lanes/`).
- `resolve_root`: `HERDR_PLUGIN_STATE_DIR` then `HERDR_ADE_ROOT` then
  `~/.herdr-ade`, plus `/pi` (§3.1 "Shared state lives under ADE's
  `HERDR_PLUGIN_STATE_DIR` at .../pi/").
- `check(provider)` — the exact entry A1 calls (`crate::pi::check`), process
  env + real runner, `Err` carrying every failing row (§3.4 refusal list).
- `login_instructions()` — the one-time login steps per provider, printed by
  the binary; never a login itself (§2).

### `src/pi/sh.rs` (new; the seam, not in the brief's file list)

`src/pi/` compiles into both binaries (see "What I could not do", item 2), so
it cannot use `crate::runner`. `sh` is a small `Cmd`/`Output`/`Runner` seam
and a `FakeRunner` with the same first-match scripted shape as
`crate::runner::fake`; `login_shell` runs `zsh -lic`. Unit-tested
(stdout/stderr/exit, stdin closed, `~` expansion).

### `src/pi/install.rs` (§3.2, §3.4 marker)

- `npm_install_args`: `npm install --prefix <pi/npm> --save-exact --no-fund
  --no-audit @earendil-works/pi-coding-agent@0.85.1`; never `-g`.
- `installed_version`, `is_installed_exactly`, `cli_js_exists`,
  `install` (runs npm, asserts 0.85.1 and `dist/bundle/cli.js`).
- `GUARD_TS = include_str!("../../extensions/herdr-pi-guard.ts")`,
  `write_guard`, `guard_ok` (marker check).
- `link_line` (`ln -s <wrapper> ~/.local/bin/pi`), `setup` (install, folder,
  wrapper, guard, `herdr integration install pi`, link line) — the §3.4
  setup action, `PI_CODING_AGENT_DIR` passed in env only.

### `src/pi/folder.rs` (§3.3)

- The exact `settings.json` (`defaultProjectTrust: "never"`,
  `enableInstallTelemetry: false`, `quietStartup`, `skills.enabled: false`,
  `retry.maxRetries: 1`, `retry.provider.maxRetries: 0`,
  `maxRetryDelayMs: 60000`), `models.json` with empty providers (no Pro),
  `trust.json` empty; `auth.json` is never written by setup.
- `ensure` (idempotent, never clobbers settings), `read_settings`
  (`trust_never`, `skills_disabled`, `telemetry_off`, `retries_capped`),
  `trust_has_true`, `extension_state`, `lane_session_dir` (shared-cwd case).

### `src/pi/launch.rs` (§3.2 wrapper, §3.4 start line)

- `agent_dir_for`: uses `PI_CODING_AGENT_DIR` when set and not under
  `~/.pi`; otherwise the baked path; refuses a dir under `~/.pi` (§3.2).
- `wrapper_script`: refuses `install|remove|uninstall|update|config`,
  finds `node` on the given `PATH`, refuses below 22.19.0, sets
  `PI_SKIP_VERSION_CHECK=1` and `PI_TELEMETRY=0`, `exec node <cli.js> "$@"`.
  `write_wrapper` writes it 0755. Real-run evidence below.
- `start_args` / `agent_start_args` / `agent_start_line`: the exact §3.4
  line, `--no-skills` always, `launch.resume_session` appended only as
  `--session <path>` (§3.8), never inside a recipe.
- `validate_args` (`pi_args_forbidden`: the 13 forbidden flags plus `--force`,
  required `--model`/`--no-skills`), `validate_env` (`pi_env_forbidden`),
  `validate_provider_column`, `validate_thinking` (`deepseek-v4-flash`/`k3`
  no `xhigh`, `grok-4.6` no `max`), `flag_value`.

### `src/pi/resume.rs` (§3.8)

- `restore_line` = `pi --session <path>`; `append_resume_session` refuses a
  session flag in the recipe; `SESSION_PICKING` and `r2_strip_list()`
  (`--session`, `--fork`, `--no-session`, `-c`, ...; no `-s`);
  `session_from_pane_get` reads `result.pane.agent_session.value` (and the
  agent shape tolerantly).

### `src/pi/roles.rs` (§3.4, §3.5, §6.2)

- `pi_recipes()`: `pi_deepseek_flash`, `pi_codex_sol_high` (escalate only),
  `pi_codex_astra_xhigh` (`enabled = false`), `pi_opencode_muse`,
  `pi_kimi_k3`. Each row: `kind = "pi"`, `provider`, `model_family`, the
  four-flag `args`, `env = []`, `ready_timeout_ms = 30000`, `enabled`,
  `start_time_allowed`, and a D17 `plain` birth sentence.
- `withheld_recipes()`: `pi_xai_grok_xhigh` and the OpenCode Grok door,
  withheld per decision 19:25 item 6. No `pi_cursor_*` row; `validate()`
  refuses a Cursor provider/add-on (`pi_cursor_forbidden`).
- `enabled_providers()` for doctor; `is_pi_provider`.

### `src/pi/priming.rs` (§3.6)

- `adapter_row()` with the D15 row verbatim (start, receipt, pre-ready and
  post-ready traps, required, untested, `capability: unqualified`),
  `prime_line(role)` = `ha skill <role>`.

### `src/pi/limits.rs` (§3.7, §3.10)

- `LimitClass` (`limit|login|unreachable|error`) and `classify` (429 and
  billing/quota words; 401/403 and credentials/token words; refused/ENOTFOUND/
  timeout words), `waiting_line` (`WAITING <lane> <provider> <class>: <first
  120>`), `blocked_label` (with HTTP status), `Throttle` (once per class per
  10 minutes), `rate_limit_table()` for §3.10.

### `src/pi/doctor.rs` (§3.4, §3.9)

- `Row`/`Level`/`line()` (`[ok  ]`, `[warn]`, `[FAIL]` shape).
- `doctor_rows_with` runs the §3.9 table: node (`zsh -lic`), npm, wrapper
  `pi --version` = 0.85.1, prefix `package.json` exact pin (not `^0.85.1`),
  wrapper on the login PATH and no earlier `pi`, prefix isolation vs
  `npm root -g`, pi folder, settings trust/skills/retry, `trust.json`,
  `herdr integration status` = `pi: current`, guard marker, Cursor
  informational (fails on `@cursor/sdk`/`pi-cursor*`/`herdr-pi-cursor.ts`),
  one row per enabled provider (`pi auth check --provider <p> --json
  --no-refresh`, stdin closed), `~/.codex` bridge URL (fails, pro-bridge
  risk 2), `~/.pi` existence only.
- `doctor_rows()` (process env; what A1 wires). `check_report` returns the
  same checks as JSON (`ok`, `provider`, `checks[]`), `check_with` refuses
  with all failing rows.
- Note: SPEC-pi writes `zsh -lic 'command -v -a pi'`; that is a bash form and
  zsh rejects it (`zsh: command not found: -v`, verified on this Mac). The row
  uses `whence -va pi`, zsh's own list form. See open question 1.

### `src/pi/ade.rs` (crate-side seam; registered only in `herdr-ade`)

- `Adapter` implements `pi::sh::Runner` over `crate::runner::Runner` (the
  real runner or a `FakeRunner`), `check_with`, `doctor_rows_with`,
  `process_prefix`. This is the seam for A1's scripted tests. Unit-tested
  through the plugin's own `FakeRunner`.

### `src/pi/scenarios.rs` (test-only; §1.3, SPEC-pi §7)

FakeRunner scenarios, named `scenario_*`:
`scenario_start_is_the_spec_line_and_never_a_trust_flag`,
`scenario_restart_uses_the_reported_session_and_the_recipe_stays_clean`,
`scenario_the_guard_reports_a_429_as_waiting_once_and_never_done`,
`scenario_the_guard_reports_a_401_as_login_and_a_dead_endpoint_as_unreachable`,
`scenario_setup_then_check_for_deepseek`,
`scenario_check_refuses_a_missing_login_before_any_start`.

### `extensions/herdr-pi-guard.ts` (§3.7)

Marker `herdr-pi-guard:version=1` on line 1. `agent_end` remembers the last
assistant message when `stopReason == "error"`; `after_provider_response`
keeps the HTTP status; `agent_settled` classifies, emits
`pi.events.emit("herdr:blocked", { active: true, label })` once, and runs
`ha waiting "<provider> <class>: <first 120>"` through `pi.exec` at most once
per class per 10 minutes when `HERDR_ADE_LAUNCH` is set. `agent_start` and
`input` clear it; `ui_prompt_start`/`ui_prompt_end` raise/clear a "waiting for
you" reason. When `ha` is not on `PATH` it falls back to
`herdr agent list --json` + `agent prompt <parent>`. Never `ha done`, never a
retry of its own.

### `src/bin/herdr-pi.rs` (§3)

`setup`, `login [provider]` (per-provider on-screen steps; opens pi on the
shared folder only with a TTY; never types `/login`), `doctor`, `check
<provider>` (read-only JSON, exit 1 when not ready). Compiles `src/pi/`
through `#[path = "../pi/mod.rs"]` because the brief allows only the one
`[[bin]]` entry and no library target.

### `skill/PI.md` (§3.4, §3.7, §3.8)

What a coordinator types, the row table, the one-time logins, the check verb,
the guard classes, blocked recovery (`herdr pane send-text` + Enter, never
`agent prompt`), what a restart needs, and the never list.

### `src/pi/testdata/` (test-only fixtures)

`mock-provider.js` (429 / 401 / ok) and `guard-check.sh`, the real run of
the shipping guard against the isolated pi (see gate 6).

## Contract fields added

None. `src/contracts.rs` is untouched. A4 owns `Recipe` (its
`contracts(ade-picker)` commit); my `roles::PiRecipe` is a standalone row
type A4 maps into its table, with the fields its brief lists (`kind`, `args`,
`env`, `ready_timeout_ms`, `provider`, `enabled`, plus `model_family`,
`start_time_allowed`, `plain`).

## Ticker entry point

None. SPEC-pi v2 gives the pi library no per-tick work: the guard is
event-driven inside the pi process, and the pre-launch check runs at launch.
For the same reason I cannot write the exact
`pub fn tick(t: &mut crate::ticker::Ticker)` on this branch: `Ticker` does not
exist yet (A1 owns `src/ticker.rs`), and `src/pi/` also compiles into
`herdr-pi`, where `crate::ticker` cannot exist. If the round wants a call
site anyway, add the function to `src/pi/ade.rs` (crate-only) and wire it in
A1's ticker as `crate::pi_ade::tick(&mut ticker)?;`.

## Gates

All with `CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/a5`,
`DEVELOPER_DIR=/Library/Developer/CommandLineTools`, `cargo +1.89.0`.

### 1. `cargo fmt --check` — exit 1, pre-existing only

248 diffs, all in files I do not own (same set as `main`): `src/adopt.rs`,
`src/coordinator.rs`, `src/doctor.rs`, `src/inbox.rs`, `src/lifecycle.rs`,
`src/project.rs`, `src/remote.rs`, `src/thread.rs`, `src/threads.rs`,
`src/ticker.rs`. Every file I own, `src/main.rs`'s marked block and both
binary roots pass:

```
rustup run 1.89.0 rustfmt --edition 2024 --check --config skip_children=true \
  src/pi/{mod,sh,folder,install,launch,resume,roles,priming,limits,doctor,scenarios,ade}.rs \
  src/bin/herdr-pi.rs src/main.rs
exit 0 (no output)
```

Tail of `cargo fmt --check`:

```
Diff in .../src/ticker.rs:859:
     #[test]
     fn log_is_capped() {
...
248
src/adopt.rs src/coordinator.rs src/doctor.rs src/inbox.rs src/lifecycle.rs src/project.rs src/remote.rs src/thread.rs src/threads.rs src/ticker.rs
```

### 2. `cargo clippy --all-targets --locked -- -D warnings` — exit 0

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.05s
```

### 3. `cargo test --locked` — exit 0

```
     Running unittests src/main.rs (.../herdr_ade-64f5f4c1db523e9f)
test result: ok. 221 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.31s
     Running unittests src/bin/herdr-pi.rs (.../herdr_pi-f58efabeeeb0ebaa)
test result: ok. 51 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
     Running tests/cli.rs (.../cli-26950f38ca2bebef)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.16s
```

169 pre-existing plus my 52 (51 under `pi` and 1 in `pi_ade`; the pi module
also compiles into the `herdr-pi` target, where the same 51 run again).

### 4. `cargo build --release --locked` — exit 0

```
    Finished `release` profile [optimized] target(s) in 0.02s
```

Binaries: `.target/a5/release/herdr-ade`, `.target/a5/release/herdr-pi`.

### 5. Throwaway `herdr-pi setup` + `doctor` + `check`

Isolation: `HOME=/var/tmp/ade-a5/home`,
`HERDR_ADE_ROOT=/var/tmp/ade-a5/prefix`, `XDG_CONFIG_HOME=/var/tmp/ade-a5/xdg`,
`XDG_STATE_HOME=/var/tmp/ade-a5/state`, `HERDR_SESSION=ade-a5`,
`HERDR_BIN_PATH=/Users/rolfie/projects/herdr/.target/install/release/herdr`
(the fork's release build; no live server was contacted). The fake HOME's
`.local/bin/pi` symlink points at the throwaway wrapper. Nothing was written
to `~/.pi`, `~/.codex`, `~/.herdr-ade` or `~/.local`; the fake HOME has no
`.pi` (verified).

`setup` (a real `npm install`, only under `/var/tmp/ade-a5/prefix`):

```
installed @earendil-works/pi-coding-agent@0.85.1 into /var/tmp/ade-a5/prefix/pi/npm (`npm install --prefix /var/tmp/ade-a5/prefix/pi/npm --save-exact --no-fund --no-audit @earendil-works/pi-coding-agent@0.85.1`)
wrote the shared pi folder /var/tmp/ade-a5/prefix/pi/agent (settings.json, models.json, trust.json)
wrote the wrapper /var/tmp/ade-a5/prefix/pi/bin/pi
wrote the guard /var/tmp/ade-a5/prefix/pi/agent/extensions/herdr-pi-guard.ts
installed the herdr state hook into /var/tmp/ade-a5/prefix/pi/agent/extensions
...
ln -s /var/tmp/ade-a5/prefix/pi/bin/pi /var/tmp/ade-a5/home/.local/bin/pi
```

`doctor` tail (exit 1: only the four provider login rows fail, which is the
fail-closed contract):

```
[ok  ] node: v24.19.0
[ok  ] npm: .../fnm_multishells/.../npm
[ok  ] pi version: wrapper 0.85.1
[ok  ] pin: 0.85.1 in /var/tmp/ade-a5/prefix/pi/npm/node_modules/@earendil-works/pi-coding-agent/package.json
[ok  ] wrapper on PATH: /var/tmp/ade-a5/home/.local/bin/pi -> /private/var/tmp/ade-a5/prefix/pi/bin/pi
[ok  ] prefix: /var/tmp/ade-a5/prefix/pi/npm (global npm root: .../node_modules)
[ok  ] pi folder: /var/tmp/ade-a5/prefix/pi/agent
[ok  ] settings trust: defaultProjectTrust: never
[ok  ] settings skills: skills.enabled: false
[ok  ] trust.json: no true entry
[ok  ] herdr extension: pi: current
[ok  ] guard: .../herdr-pi-guard.ts with marker
[ok  ] Cursor: outside pi; native cursor lanes only, then retired
[FAIL] provider deepseek: missing login: credentials_not_configured (run `herdr-pi login deepseek`)
[FAIL] provider openai-codex: missing login: credentials_not_configured ...
[FAIL] provider opencode: missing login: credentials_not_configured ...
[FAIL] provider kimi-coding: missing login: credentials_not_configured ...
[ok  ] ~/.codex: no config.toml
[ok  ] ~/.pi: not present
herdr-pi: some checks failed
```

`check deepseek` (exit 1, JSON tail) and `check cursor` (exit 1):

```
  "check": "login",
  "detail": "missing login: credentials_not_configured (run `herdr-pi login deepseek`)",
--- cursor ---
  "check": "provider",
  "detail": "pi_cursor_forbidden: Cursor stays outside pi (decision 18:30)",
```

Wrapper behaviour (real run):

```
$ /var/tmp/ade-a5/prefix/pi/bin/pi --version
0.85.1                                                  (exit 0)
$ /var/tmp/ade-a5/prefix/pi/bin/pi install foo
herdr-ade pi: 'install' is refused; run herdr-pi setup, which owns the pinned install   (exit 2)
$ PI_CODING_AGENT_DIR=~/.pi/agent .../bin/pi --version
herdr-ade pi: refusing a config dir under ~/.pi; use PI_CODING_AGENT_DIR elsewhere      (exit 2)
$ PI_CODING_AGENT_DIR=/var/tmp/ade-a5/other .../bin/pi --version
0.85.1                                                  (exit 0)
```

The pinned prefix `package.json` holds the exact range (not a caret):

```
{"dependencies":{"@earendil-works/pi-coding-agent":"0.85.1"}}
```

### 6. Guard against the isolated pi 0.85.1 (real run, T7)

`sh src/pi/testdata/guard-check.sh` with
`/var/tmp/pi-research-a/npm/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js`,
`PI_CODING_AGENT_DIR=/var/tmp/ade-a5/guard-test/agent` (guard copied in, mock
`models.json`), a fake `ha` on `PATH`, `HERDR_ADE_LAUNCH` set. Exit 0:

```
PASS T7a 429 becomes blocked + WAITING limit: mock hits 2, ha waiting once (class limit)
PASS T7b 401 becomes blocked + WAITING login: mock hits 1, ha waiting once (class login)
GUARD PASS
```

Recorded `ha waiting` lines (two hits on the 429, one on the 401: pi's v2
retry cap is visible):

```
waiting mock-provider limit: 429: {"message":"Rate limit reached for mock-model","type":"rate_limit_error"}
waiting mock-provider login: 401: {"message":"Incorrect API key provided: mock-key","type":"invalid_request_error"}
```

The pane `blocked` half of T7 needs a herdr session and is the reviewer's row
(the emit is the same `herdr:blocked` channel the herdr asset consumes).

## Left for the reviewer (exact lines)

### A1's `src/doctor.rs` (pi rows)

```rust
    // before `(out, healthy)`:
    match crate::pi::doctor::doctor_rows() {
        Ok((rows, pi_healthy)) => {
            healthy &= pi_healthy;
            for row in rows {
                let _ = writeln!(out, "{}", row.line());
            }
        }
        Err(error) => check(&mut out, Some(false), "pi", format!("{error:#}")),
    }
```

For scripted tests A1 passes its runner through
`crate::pi_ade::doctor_rows_with(runner)` (same rows, `crate::runner::Runner`).

### A1's launch refusal and process check (`src/threads.rs`, `src/herdr.rs`)

```rust
// before `herdr agent start` for a `kind = "pi"` row (provider = the row's column):
crate::pi::check(provider)?;                     // every failing §3.4 row in one refusal
// or, scripted: crate::pi_ade::check_with(ctx.runner, provider)?;

// after start, from `herdr pane process-info <pane>`:
let prefix = crate::pi_ade::process_prefix()?;   // .../pi/npm
if !process.path.starts_with(&prefix.display().to_string()) {
    anyhow::bail!("refused: the pane's pi process is not under {}", prefix.display());
}
```

### A2's adapter table (`src/adapters.rs`)

```rust
let pi_row = crate::pi::priming::adapter_row();  // D15 row, kind "pi"
```

The guard may run `ha waiting`; it never runs `ha done`. Blocked recovery is
`herdr pane send-text` + `send-keys enter`, never `herdr agent prompt`.

### A4's picker table (`src/launch.rs` / `src/project.rs`)

```rust
for row in crate::pi::roles::pi_recipes() { /* recipe_id = row.id, provider = row.provider,
    model_family, args, env, ready_timeout_ms, enabled, plain */ }
let withheld = crate::pi::roles::withheld_recipes();   // pi_xai_grok_xhigh, OpenCode Grok
```

No `pi_cursor_*`; `cursor_grok_xhigh` stays native. `provider` must equal
`--provider` (`crate::pi::launch::validate_provider_column`).

### `herdr-plugin.toml` actions (manifest owner; SPEC-pi §3)

```toml
[[actions]]
id = "pi-doctor"
title = "Pi: check setup"
contexts = ["workspace"]
command = ["target/release/herdr-pi", "doctor"]

[[actions]]
id = "pi-setup"
title = "Pi: install the pinned pi"
contexts = ["workspace"]
command = ["target/release/herdr-pi", "setup"]

[[actions]]
id = "pi-login"
title = "Pi: open a login pane"
contexts = ["workspace"]
command = ["target/release/herdr-pi", "login"]
```

### A3's `scripts/acceptance` (§7 rows, NOT-RUN today)

Suggested lines for `run` (A0's `row` shape; pi kind rows are NOT-RUN until
the coding lane moves, deterministic rows are required):

```sh
row pi-T1 required "herdr-pi doctor: pin exactly 0.85.1, pi: current, prefix isolated, wrapper the only pi on the login PATH, trust never"
row pi-T2 required "herdr-pi check refuses: missing guard, missing herdr extension, trust not never, wrapper not first on PATH, provider not ready"
row pi-T3 required "mock lane in a repo with .pi/extensions: no trust question, kind pi idle at prompt, repo extension did not run"
row pi-T7 required "guard: mock 429 and mock 401 -> pane blocked + one WAITING <lane> <provider> limit|login, typing clears, no done"
row pi-T9 required "no global npm: zsh -lic 'npm root -g' does not contain @earendil-works/pi-coding-agent"
row pi-T10 required "wrong pi earlier on PATH: doctor fails and start refuses (process-info prefix)"
row pi-T11 required "missing login: start refused by pi auth check, never a lane that waits for its first prompt"
row pi-T13 required "no Cursor route: no @cursor/sdk, no pi-cursor*, doctor informational, --provider cursor refused"
row pi-T4 optional "one live lane per enabled provider; priming ha skill records bootstrap acknowledged"
row pi-T5 optional "ha done from a pi lane: DONE line, report hashes, clean tree"
row pi-T6a optional "live handoff: pi pid unchanged, no resume typed"
row pi-T6b optional "cold restore: pi --session through ~/.local/bin/pi, same session file, agent_session reported"
row pi-T8 optional "after r2: exactly one --session, no --approve, no -c"
row pi-T12 optional "two lanes on one OAuth provider with an expired token: both keep working; one auth.json.lock path"
row pi-T14 optional "not a pi gate: Pro pane restore with its -c args, after r2's args replay"
```

Exact commands for the deterministic rows, all on a throwaway with the
isolated env from gate 5 (the T3/T7 rows need the throwaway herdr session and
the mock provider `src/pi/testdata/mock-provider.js`):

- T1: `herdr-pi doctor` (expect exit 1 only while no login exists).
- T2: delete `agent/extensions/herdr-pi-guard.ts` → `herdr-pi check deepseek`
  exit 1 `guard`; set `settings.json` trust `ask` → `settings trust`;
  put `~/.local/bin/pi` after another `pi` → `wrapper on PATH`;
  a fresh root → `login` row.
- T3: `herdr agent start <name> --kind pi --pane <pane> --parent <coord> --
  --provider mock-provider --model mock-model --thinking low --no-skills`
  with the mock `models.json` and `mock-provider.js ok <port>`; assert no
  trust screen and no marker side effect.
- T7: `sh src/pi/testdata/guard-check.sh` (proves the two `ha waiting`
  lines); the pane `blocked` and the clear-on-typing half needs the herdr
  session.
- T9: `zsh -lic 'npm root -g'` from the isolated env.
- T10: `PATH=<earlier-pi-dir>:$PATH herdr-pi doctor` and a start attempt.
- T11: `herdr-pi check deepseek` on a root with no `auth.json`.
- T13: `herdr-pi check cursor` (exit 1 `pi_cursor_forbidden`) and a
  `node_modules` scan.

### r2 strip list (SPEC-pi §3.8)

When `review/durable-r2` merges, drop for pi: `--session <v>`, `--fork <v>`,
`--no-session`, and treat `-c`/`--continue`/`-r`/`--resume` as boolean flags.
Pi has no `-s`. Do not require replay of `--provider` / `--model` /
`--thinking`; never replay `--approve`. `crate::pi::resume::r2_strip_list()`
holds the exact tokens.

### Seam status

`src/cli.rs` untouched (no `herdr-ade` subcommand is needed; the thin binary
covers setup/login/doctor/check). `src/main.rs` has only the `ade-pi begin/end`
block (five lines): `mod pi`, `#[path = "pi/ade.rs"] mod pi_ade`. The
`[[bin]]` entry is the only `Cargo.toml` change.

## Fork must ship (not my files, SPEC-pi §4)

- **Required for v2: nothing on master.** The wrapper plus the one shared
  folder cover cold restore; model and thinking come from the session file.
- **Should ship (defense, not a start gate):** a `blocked` screen rule in
  `src/detect/manifests/pi.toml` for `Trust project folder?` and the
  missing-session-folder `Continue / Cancel` question (they render before the
  extension reports). The settings file stays the control.
- **On `review/durable-r2` when it merges:** `persisted_session_from_launch_args`
  for `Agent::Pi`; `--session`, `--fork` (and `--no-session`) in
  `strip_session_picking_args`, with pi's `-c` as a boolean and no `-s`.
- **Later, shared with pro-bridge:** persist tab `--env` on restore so
  `HERDR_ADE_LAUNCH` survives a server bounce (not needed for
  `PI_CODING_AGENT_DIR` once the wrapper supplies the folder).

Plugin-side, this lane ships: the pinned prefix, the wrapper, setup/login/
doctor/check actions, the shared folder with `settings.json`, the herdr
integration install into it, the guard extension, the D2 rows, the D15 row,
and `crate::pi::check` for the pre-launch refusal.

## What I could not do and why

1. **Run T4, T5, T6a, T6b, T8, T12, T14.** No provider login exists (no
   `/login` was ever typed, by the rules), no herdr session was started for a
   pi lane by this lane, and `review/durable-r2` is not on this branch. These
   are the reviewer's rows; exact lines above.
2. **Expose `pub fn tick(t: &mut crate::ticker::Ticker)`.** `Ticker` does not
   exist on this branch (A1 owns `src/ticker.rs`), and `src/pi/` also compiles
   into `herdr-pi`, where `crate::ticker` cannot exist. There is no per-tick
   pi work in SPEC-pi v2. A crate-side home for it is `src/pi/ade.rs`; the
   wiring line is in "Left for the reviewer".
3. **Add the three pi actions to `herdr-plugin.toml`.** The manifest is not
   in my owned files; the exact TOML is in "Left for the reviewer".
4. **A scripted `doctor_rows` for A1's unit tests.** Provided through
   `crate::pi_ade::doctor_rows_with(runner)`; A1 must call that (its own
   `Runner`) instead of `crate::pi::doctor::doctor_rows()`.
5. **The literal spec probe `command -v -a pi`.** It is not zsh syntax; see
   open question 1 and the `whence -va` note.
6. **Pane `blocked` verification (T7's herdr half), the trust-dialog absence
   (T3) and cold restore (T6b).** They need the throwaway herdr session and
   the reviewer's script; the guard's `ha waiting` half is proven on the real
   isolated pi (gate 6).

## Open questions (numbered from 1 for SPEC-ADE §6)

1. SPEC-pi §3.2/§3.9 write `zsh -lic 'command -v -a pi'`. On this Mac zsh
   answers `command not found: -v` (the `-a` flag is a bash form). I
   implemented the zsh form `whence -va pi` and kept the same semantics
   (first resolution must be `~/.local/bin/pi`, no earlier `pi`). Should the
   spec text change to `whence -va pi`?
2. SPEC-pi question 13 / §3.2 makes `~/.local/bin/pi` always use the harness
   folder, with no `HERDR_ENV` gate (the v1 draft had one). My wrapper follows
   v2: a plain-terminal `pi` uses the harness folder and never `~/.pi`.
   Rolf's 19:25 item 2 reading ("one pi on this Mac, the harness pi") accepts
   this; confirm the no-gate reading.
3. `herdr-pi doctor` fails while any enabled provider has no login. On a
   fresh install that means doctor exits 1 until Rolf's first `/login`. Is
   that the wanted fail-closed reading, or should provider rows be `warn`
   until the roles table actually enables that provider?
4. The guard's `ha`-missing fallback reads `tokens.parent` from
   `herdr agent list --json`. The `tokens` field lands with A1's r2 fork
   (`src/api/schema/agents.rs:208`); on today's 0.9.0 the fallback silently
   does nothing. Acceptable until install day?
5. `pi_opencode_muse` uses model id `muse-spark-1.3` (research a / SPEC-pi
   §3.4). SPEC-pi §3.5 says "when you name the installed id". Is
   `muse-spark-1.3` the installed OpenCode Zen id, or should A4 ship the row
   without an id until Rolf names it?
6. Decisions 19:25 item 6 says workers skip Grok until Rolf says. SPEC-pi
   §3.4/§3.5 still list the OpenCode Grok row and the optional xAI row. I
   shipped no Grok row and listed both as withheld. Confirm that reading.
````
