# Plugin round r2: verdict (reviewer)

**MERGE.** Lane b1's `pi_opencode_deepseek` row is right: `opencode-go`, `deepseek-v4.1-flash`, high thinking, the Muse row's shape, and one shared `opencode-go` login in doctor. Every gate is green, and all 45 deterministic acceptance parts pass, as they did on r1.
Two small text defects are fixed: the `opencode-go` login text named only Muse, and the `herdr-pi login` help still offered the deleted Zen provider. The lane added no test and neither did I.
Decisions 101 and 102 are open. Neither blocks the merge, but 102 decides whether a lane that asks for DeepSeek actually gets this row.

Branch `review/plugin-r2`: merge `3ad5aef` of `lane/ade-deepseek-go` (eda1352b), then `6d37c54` and `83e82b2`. The final commit is named in `.reports/review-plugin-r2-report.md`.

## Gates (final code, `83e82b2`)

Env: `CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/review`, `DEVELOPER_DIR=/Library/Developer/CommandLineTools`. The pi gates ran with `HERDR_ADE_ROOT`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME` and `HOME` under `/var/tmp/ade-review2/`, that home's `.local/bin` first on `PATH` (the setup-printed `pi` link lives only there), and `HERDR_BIN_PATH=/Users/rolfie/projects/herdr/.target/review/release/herdr` (0.9.1).

| Gate | Result |
|---|---|
| `cargo +1.89.0 fmt --check` | exit 0 |
| `cargo +1.89.0 clippy --all-targets --locked -- -D warnings` | exit 0 |
| `cargo +1.89.0 test --locked` | exit 0: 357 + 41 + 4 + 29 = 431 passed, 0 failed, 0 ignored |
| `cargo +1.89.0 build --release --locked` | exit 0 |
| `herdr-pi setup` | exit 0, pin 0.85.1 in the throwaway prefix |
| `herdr-pi doctor` | exit 1: every row ok except the three missing logins (`opencode-go`, `openai-codex`, `kimi-coding`, `credentials_not_configured`). One `opencode-go` row, shared by DeepSeek and Muse. |
| `herdr-pi check opencode-go` | exit 1: seven readiness rows ok, `login` fails on `credentials_not_configured` (item 63) |
| `ACC_DIR=/var/tmp/ade-review2/acc sh scripts/acceptance/run` | 45 det, cli and mod parts, all PASS (15 cli, 16 det, 14 mod); 20 live parts NOT-RUN (`ACC_LIVE` not 1). 9/19 counted rows, the rest fail only on live parts, the same as r1. |

Extra checks. `herdr-ade doctor` on a throwaway `config.toml` whose `[roles.lane].allowed` names `pi_opencode_deepseek`: `picker recipes: 6 recipe(s) valid`, so the row joins the picker's table and its reason templates pass. `tests/picker_plain.rs` with the three coding phrases added to `PHRASES` (a local edit, reverted, not committed): all 29 pass. pi's own resolver (`resolveCliModel` from the pinned 0.85.1) was run against a copy of the throwaway pi folder, cold and after one catalog refresh (decision 101). No live acceptance run: no deterministic part failed.

## What was checked, no change needed

- Id `pi_opencode_deepseek`, provider `opencode-go`, model `deepseek-v4.1-flash`. pi.dev's `opencode-go` catalog (the overlay pi 0.85.1 loads) lists that id as "DeepSeek V4.1 Flash", on `openai-completions` with the deepseek thinking format. I did not run the OpenCode CLI (Rolf's home).
- Args are exactly `--provider opencode-go --model deepseek-v4.1-flash --thinking high --no-skills`, built by the same `recipe()` → `start_args` as Muse. `validate_provider_column` passes, and there are seven args. The env is empty, the timeout is 30000, the row is enabled and start-time allowed (the Muse and Kimi shape). pi's catalog maps `high` to `high` for this model.
- `the cheap coding helper` passes R1 to R7 as a phrase and inside A4's default, pinned, usual and fallback templates. The lane said "in the full test suite", but no committed test renders this phrase: `PHRASES` does not carry it, and `check_reasons_are_plain` renders only rows in some `allowed` list. I checked it by hand, as above.
- `enabled_providers()` is now `opencode-go, openai-codex, kimi-coding`. The lane updated the order in the existing test, and doctor checks `opencode-go` once.
- `skill/PI.md`: one row, as briefed.
- Nothing else changed. The lane diff is `src/pi/roles.rs` and `skill/PI.md`. No `Cargo.toml` or `Cargo.lock` change, no new dependency, no `unwrap()`, no serde shape (the row is a `PiRecipe` literal; `builtin_pi_recipes` fills every `Recipe` field). No OpenCode CLI, Zen, direct `deepseek` provider or Grok trace came back.

## Defects

| # | Severity | File:line | What was wrong | What I changed | Commit |
|---|---|---|---|---|---|
| 1 | low | `src/pi/mod.rs:230-231`, `src/pi/launch.rs:16` | `herdr-pi login` called `opencode-go` "the Muse row" and said "Go serves muse-spark-1.3-contributor". With the DeepSeek row on the same key, Rolf would look for a second DeepSeek login. | The login step names both rows and both model ids. The `PROVIDERS` comment does too. | `6d37c54` |
| 2 | low | `src/bin/herdr-pi.rs:36` | `herdr-pi login --help` offered `opencode`, which r1 deleted (2937fd8) and the command refuses: `unknown provider \`opencode\``. This is an r1 leftover, not the lane's. | The help lists `openai-codex, opencode-go, kimi-coding`. | `83e82b2` |

No new tests. Neither defect is behaviour a test pins.

## Needs a decision

101. **The DeepSeek id is not in pi 0.85.1's built-in table.** The pinned pi's `opencode-go` table has `deepseek-v4-flash` but not `deepseek-v4.1-flash`. Interactive pi resolves `--model` before its network refresh, from the built-in table plus the pi.dev overlay stored in the shared folder (`models-store.json`). A credentialed refresh stores the overlay. Rolf's `/login` for `opencode-go` runs one right after the login (15 s timeout, "could not be refreshed; using cached models" on failure), and every interactive start refreshes again in the background every 4 hours. If no refresh has ever succeeded, the first DeepSeek lane starts with pi's warning `Model "deepseek-v4.1-flash" not found for provider "opencode-go". Using custom model id.` It then runs on a copy of `kimi-k2.6`'s settings, without `requiresReasoningContentOnAssistantMessages`, which DeepSeek's thinking mode needs across tool-call turns. I reproduced both states with pi's own resolver. Choices: (a) accept it, because the day-one order (login, then lanes) warms the overlay; (b) `herdr-pi check <provider>` refuses while a shipped row's model id does not resolve in the shared folder, a new refusal in SPEC-pi §3.4's list, fail-closed like item 63; (c) the row uses the built-in `deepseek-v4-flash`. My reading: (a) now, (b) in a later round. (c) is not what Rolf's lanes run on.

102. **Which `allowed` list names the row.** SPEC-pi §3.5 says no pi row joins a start-time `allowed` list because it is new, and SPEC-jev-picker's table has no DeepSeek twin. So `--recipe pi_opencode_deepseek` is refused with `recipe_not_allowed` until Rolf's own `~/.config/herdr-ade/config.toml` lists it, and the plugin does not write that file. `skill/PI.md`'s rows table also gives no recipe ids, so a coordinator has to learn `pi_opencode_deepseek` elsewhere. Choices: (a) Rolf adds `pi_opencode_deepseek` to `[roles.lane].allowed`, as I read his words on item 80 ("I don't want the opencode cli"); (b) also to `reviewer` or other roles; (c) leave it out, so DeepSeek lanes keep running on the native OpenCode CLI. In any case, PI.md's table gains a recipe-id column next round. My reading: (a), plus the id column.

Item 100 (the SPEC-pi text that still names the direct row) stands as recorded. The lane's open question 1 is the same item.
