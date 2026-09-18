# Plugin round r1, part b: verdict on A4 `ade-picker` and A5 `ade-pi`

Reviewer b, 2026-09-18. Lanes reviewed and fixed on their own branches, never
merged, never pushed. `lane/ade-picker` ends at `e6cdaa8`, `lane/ade-pi` at the
commit that carries this file.

## Verdicts

**A4 `ade-picker`: MERGE.**
The curl failure handling was wrong and the lane shipped a launch mode the round does not
allow; both are fixed with tests, and the Opus effort rule no longer fails open.
What is left (items 95 to 99) is wording, naming and one refusal code; none of it blocks the merge.

**A5 `ade-pi`: MERGE-AFTER-DECISION.**
Two defects let a lane reach Rolf's own pi state (`~/.pi` spellings, skills loading on a bare
restore) and one split the pi folder in two; all are fixed and checked for real on a throwaway.
The code now departs from SPEC-pi v2 text in four places (items 90 to 93); Rolf decides those.

## Gates

`DEVELOPER_DIR=/Library/Developer/CommandLineTools`, `cargo +1.89.0`, each lane's own
`CARGO_TARGET_DIR`. `cargo fmt --check -- <files>` checks the whole crate whatever files it is
given, and the unowned files carry `main`'s old wrapping (the lane briefs say so), so fmt is
`rustfmt --edition 2024 --check --config skip_children=true <owned files>`.

### A4 at `e6cdaa8`

| Gate | Exit | Counts |
|---|---|---|
| rustfmt check, `src/jev.rs src/launch.rs src/contracts.rs tests/picker_plain.rs` | 0 | 4 files |
| `clippy --all-targets --locked -- -D warnings` | 0 | 0 warnings |
| `test --locked` | 0 | unit 217, `tests/cli.rs` 4, `tests/picker_plain.rs` 29; 0 failed |
| `build --release --locked` | 0 | |
| throwaway `picker-doctor` (resolver shadow, candidate herdr 0.9.1) | 0 | 9 ok, 2 warn (inline role without plain, item 47; Codex quota note); one `GET /v1/models` → 200 |
| throwaway `doctor` | 0 | herdr 0.9.1 ok; warns only on no session socket, gh auth, ticker not running |

### A5 at `fb5dbdc`

| Gate | Exit | Counts |
|---|---|---|
| rustfmt check, `src/pi/*.rs src/bin/herdr-pi.rs` | 0 | 12 files |
| `clippy --all-targets --locked -- -D warnings` | 0 | 0 warnings |
| `test --locked` | 0 | `herdr-ade` unit 212, `herdr-pi` 41, `tests/cli.rs` 4; 0 failed |
| `build --release --locked` | 0 | |
| `herdr-pi setup` (throwaway root) | 0 | |
| `herdr-pi doctor` | 1 | 15 ok; FAIL only on the four logins (deepseek, openai-codex, opencode-go, kimi-coding) |
| `herdr-pi check deepseek` | 1 | 7 ok; FAIL only `login` |
| `herdr-pi check cursor` | 1 | FAIL `pi_cursor_forbidden`; every other row ok |
| `src/pi/testdata/guard-check.sh` | 0 | T7a 429, T7b 401, T7c context-length |

## Pi §7 rows (throwaway `/var/tmp/ade-revb`, candidate herdr 0.9.1, session `ade-revb`)

| Row | Result | Notes |
|---|---|---|
| T1 setup + doctor | PASS | Red only on the four missing logins (rerun after `fb5dbdc`). |
| T2 doctor names each broken part | PASS | Missing guard, missing herdr extension, trust `ask`, an earlier `pi` on PATH, a fresh root: each named in its own row. |
| T3 no trust dialog, repo `.pi` ignored | PASS | Pi's own "not trusted, ignored" notice only; kind `pi` idle; the repo `.pi` extension's marker never written; `trust.json` unchanged. `working` not seen: the mock answers at once. This run found the skills defect (pi deleted `skills.enabled`). |
| T4 real provider lane | NOT-RUN | Needs a login; no `/login` typed. |
| T5 `ha done` from a pi lane | NOT-RUN | Needs a login and A2's `ha`. |
| T6 cold restore / `--session` | NOT-RUN | Needs the installed r2 fork and cold restore. |
| T7 429 → `blocked` + one `ha waiting`, never `done` | PASS | guard-check T7a/T7b/T7c; in a pane: 2 mock hits, `blocked`, one WAITING line, `agent prompt` refused `agent_blocked`, send-text + Enter back to idle, no `done`. The 401 case ran in guard-check only. |
| T8 | NOT-RUN | Needs a login. |
| T9 global npm install fails the prefix row | PASS | After `0cf67c5`; failed to fail before it. |
| T10 process path under the prefix | Half PASS, half NOT-RUN | The check/doctor half passes. The `pane process-info` half cannot be done: it shows `argv0: "pi"`, `name: "node"` and a pid, and pi sets `process.title`, so neither it nor `ps` shows the `cli.js` path. Item 92. |
| T11 wrapper refuses `~/.pi` | PASS | Every spelling (quoted `~`, relative, `//`, `..`, symlink) refused after `77c4c9e`. |
| T12 | NOT-RUN | Needs the installed r2 fork. |
| T13 Cursor stays outside pi | PASS | `check cursor` / `check Cursor` refused `pi_cursor_forbidden`; no cursor package in the prefix. |
| T14 | NOT-RUN | Needs cold restore. |

Leak check after the run: nothing new under `~/.pi`, `~/.local/bin`, `~/.config/herdr-ade`;
no `~/.herdr-ade`. The throwaway session was stopped with the candidate's `session stop ade-revb`.

## Defects

### A4 `ade-picker`

| Sev | File:line (at `e6cdaa8`) | What was wrong | What I changed | Commit |
|---|---|---|---|---|
| High | `src/jev.rs:652` | `curl` prints its write-out on a failed transfer, so a refused connection or `--max-time` ended in `000` and was read as HTTP 0: connect errors never retried, a timeout recorded as `http_0`, doctor said "answered 0". | Only a clean exit carries a status; exit 28 is Timeout. Tests send real 401/402/422/500/529 and real curl failures. | `05dc52f` |
| High | `src/contracts.rs:33`, `src/launch.rs:1081` | The lane shipped `resolver = "jev"`, which launches Jev's pick. The round ships off and shadow only. | First refused (`33481a1`), then deleted under Rolf's rule (see Deleted). | `33481a1`, `5a268f2` |
| High | `src/launch.rs:537` | "Names Opus or Fable" came from `model_value`, which reads only `--model <v>` / `model=<v>`: `-m claude-opus-5 --effort xhigh` or `--model=claude-opus-5 --effort xhigh` passed validation. | The check reads every arg. Both spellings added to `validation_refuses_bad_tables`; it fails on `9ffd60d`. | `e6cdaa8` |
| Med | `src/launch.rs:1102` | `gate_p` stored the gate's threshold, not Jev's Noul. | Stores the Noul; asserted in the shadow scenario. | `c5612ca` |
| Med | `src/launch.rs:1049` | `config_changed` launched the new table's default under the old recipe id. | Launches it under its own id. | `2eaf1da` |
| Med | `src/launch.rs:801` | A same-kind PROJECT.md pin with no args dropped the args, and a pin skipped the flag checks. | The pin passes the same checks as a table row. | `89bd702` |
| Med | `src/jev.rs:116` | The scrub list split codex `key=value` words (`approval_policy=never` scrubbed `never`). | Only model values are split. | `e7477bc` |
| Med | `src/launch.rs:196`, `:1221` | Item 47: an inline role without `plain` was filled silently; the key/curl rows warned with the resolver off. | One doctor warn per such role; key rows only when the picker calls out. | `a8c2504` |
| Med | `src/launch.rs:750` | `pro` and `coordinator` could resolve through the table. | Refused `role_not_resolved`. | `4d82c49` |
| Low | `src/jev.rs:787` | The key went unescaped into the stdin curl config; a key with a newline or quote could add curl settings. | Such a key is not usable. | `bd83157` |
| Low | `src/launch.rs:305` | A misspelt floor class was accepted. | Refused. | `df599d7` |
| Low (missing test) | `src/launch.rs` tests | No test for a task that names a model. | `a_task_that_names_a_model_is_not_a_pin`. | `5dd482a` |
| Low (missing test) | `src/launch.rs:1197` | No test for the daily cap counting. | `every_call_counts_against_the_daily_cap_once`, `the_daily_cap_stops_the_call`. | `191c386` |

### A5 `ade-pi`

| Sev | File:line (at `fb5dbdc`) | What was wrong | What I changed | Commit |
|---|---|---|---|---|
| High | `src/pi/launch.rs:101` | The wrapper compared `PI_CODING_AGENT_DIR` as typed with `$HOME/.pi`; pi expands `~` and resolves relative paths, so `'~/.pi/agent'`, `.pi/agent`, `//`, `..` or a symlink reached Rolf's own `~/.pi`. Baked paths were unquoted. | Expand, absolutise, squeeze, refuse `.`/`..`, compare the physical path too; baked paths single-quoted. Real-run test with a fake node. | `77c4c9e` |
| High | `src/pi/folder.rs:21` | `"skills": { "enabled": false }` does nothing in pi 0.85.1: it is the old object form, which pi migrates away (it deleted it during T3) and which never stopped discovery. A bare `pi --session` restore loads `~/.agents/skills` into a lane. Measured with a probe skill. | `"skills": ["!**"]`; doctor reads that. Test: the old form reads as not disabled. | `fb5dbdc` |
| High | `src/pi/mod.rs:189` | `resolve_root` preferred `HERDR_PLUGIN_STATE_DIR` (set only for plugin actions), so setup as a plugin action and `ha`/ticker/terminal runs used two pi folders; every pi start would refuse "pinned package missing". | ADE's root (`HERDR_ADE_ROOT`, `root` in config.toml, `~/.herdr-ade`) + `/pi`, made absolute. | `a87ac79` |
| Med | `src/pi/roles.rs:86` | Plain phrases were full sentences with product names; A4's templates would fail every pi row `recipe_reason_not_plain`. | Noun phrases; `src/pi/ade.rs` test renders A4's templates through the real checker. | `18e0dde` |
| Med | `src/pi/roles.rs:114` | Item 65: the Muse row named `opencode` + `muse-spark-1.3`; pi serves `muse-spark-1.3-contributor` only under `opencode-go`. | Row on `--provider opencode-go --model muse-spark-1.3-contributor`; providers, logins, doctor rows, skill follow. | `ee5e989` |
| Med | `src/pi/launch.rs:236` | `--model`/`--no-skills` required only for five named providers; `-na`, `--no-session` not refused; `Cursor` passed a case-sensitive compare. | Required for every row; both refused; case-insensitive. | `914444b` |
| Med | `src/pi/doctor.rs:196` | T9: the prefix row searched the `npm root -g` *path string* for the package name, so a global install always passed. | Looks for the package folder inside the global root. | `0cf67c5` |
| Med | `src/pi/sh.rs:148`, `:202`; `src/pi/ade.rs:30` | A timeout killed only `zsh`; a grandchild held the pipe and `doctor`/`check` hung past the deadline. | Own process group; TERM then KILL to the group; the adapter asks for `own_group`. | `a09035a` |
| Med | `extensions/herdr-pi-guard.ts:42` | Any message with "token" was a login problem, so a context-length error told the coordinator to log in. | Only the login token phrases; guard-check T7c (context-length → `error`) fails on the old guard. | `7d61438` |
| Med | `src/pi/resume.rs:53` | The r2 strip list handed the fork bare tokens; the fork takes `-c`/`--continue`/`-r`/`--resume` as value-taking, so a replayed `-c "fix the test"` lost the message. | `R2_STRIP_RULES` carries each token's arity. | `0a223da` |
| Low | `extensions/herdr-pi-guard.ts:105`, `src/pi/testdata/guard-check.sh` | The guard's parent fallback could never run, and guard-check accepted "at least one" WAITING line. | Made reachable in `ab976eb`, then deleted with the rest of the pre-ADE route in `b374e63` (item 94). guard-check now wants exactly one WAITING line and no `done`, with a fake herdr on `HERDR_BIN_PATH`. | `ab976eb`, `b374e63` |
| Low | `src/pi/doctor.rs:580` | `whence -va pi` in `zsh -lic`: any rc line with " is " counted as a pi resolution (false red). | Only lines starting `pi is `. | `6caddca` |

## Item 65

I took the coordinator's reading: the Muse row names the OpenCode Go id
`muse-spark-1.3-contributor`. Pi 0.85.1 knows that id only under `opencode-go`, so the row's
provider moved too (item 93). A4 has no Muse row.

## Deleted

Under Rolf's rules (no backward compatibility, no smoke tests, dropped features deleted).

A4 (`5a268f2`, `32bc929`, `9ffd60d`):
- the jev launch mode: `ResolverMode::Jev`, the `resolver_mode_unavailable` refusal, the launching
  branch of `accepted_launch` (now `shadow_launch`), `FALLBACK_NOT_ALLOWED`, `TEMPLATE_PICKED`,
  `picked_reason`, `reason_clause`, `engine` and `spec` from `plain/vocabulary.txt`;
- aliases nobody named: `--dangerously-bypass-approvals-and-sandbox` for claude/agy, `--yolo` for
  cursor, `-m` read as `--model`, `--effort=<v>`, the stderr fallback of `agent_kinds`, a bare status
  line in `classify`, `ScrubList::fixed` and `ScrubList::patterns`;
- tests: `the_jev_mode_is_refused_in_this_round`, `a_gate_over_its_threshold_switches_in_jev_mode`,
  `a_gate_and_its_criteria_are_sent_verbatim`, `a_gate_over_its_threshold_in_shadow_records_the_pick_only`
  (its asserts moved into the shadow scenario), `PICKED`/`CLAUSES` in `tests/picker_plain.rs`,
  constant-only asserts (fingerprint and prompt-hash length, `Recipe::default()` fields).

A5 (`b374e63`, `fb5dbdc`):
- compatibility: the guard's pre-ADE `notifyParent` route (the guard now acts only with
  `HERDR_ADE_LAUNCH` and runs only `ha waiting`); `session_from_pane_get` reads only
  `/result/pane/agent_session/value`;
- dead code: `src/pi/limits.rs` (a Rust twin of the guard's classifier no production path called),
  `agent_dir_for`, `is_ade_owned_arg`, `restore_line`, `login_path_is_the_way_back`,
  `extension_state`, `is_managed_extension`, `lane_session_dir`, `pi_ade::DEFAULT_TIMEOUT`;
- tests: `layout_paths_are_one_shared_folder`, `the_row_names_the_spec_traps`,
  `restore_line_is_the_bare_pi_session`, `wrapper_script_contains_every_refusal_and_env_line`,
  `wrapper_uses_the_shared_folder_and_refuses_pi_home`, `link_line_is_the_one_line_rolf_types`,
  `guard_is_written_with_its_marker`, `lane_session_dir_is_per_thread`,
  `a_429_is_a_limit_and_a_401_is_a_login`, `the_waiting_line_is_bounded_and_named`,
  `one_report_per_class_per_window`, `the_table_names_every_enabled_provider`,
  `scenario_the_guard_reports_a_429_as_waiting_once_and_never_done`,
  `scenario_the_guard_reports_a_401_as_login_and_a_dead_endpoint_as_unreachable`
  (the last two are covered for real by guard-check T7a/T7b), `settings_follow_the_spec_contract`.

Kept, as the brief says: the hidden `picker-doctor` verb and the `#[allow(dead_code)]` on
`mod jev` / `mod launch`. Their wiring (A1's doctor, `threads.rs`) is not on the lane branch.
`skill/THREAD.md` and the A0 compatibility code are not in A4 or A5; they are the first reviewer's.

## Needs a decision

90. **Pi root.** SPEC-pi v2 §3 and the §3.9 prefix row say `$HERDR_PLUGIN_STATE_DIR/pi`. The code
    uses ADE's root + `/pi` (`a87ac79`) so that setup, `ha`, the ticker and a terminal agree.
    Recommend: change the spec text to ADE's root.
91. **Skills setting.** SPEC-pi v2 §1, §3.3, §3.9 and the fold notes say `skills.enabled: false`,
    which pi 0.85.1 ignores and deletes. The code writes `"skills": ["!**"]` (`fb5dbdc`).
    Recommend: change the spec text.
92. **Post-start process path check** (SPEC-pi v2 §3.4, A1 bullet, D15 row, T10). It cannot be
    built: `herdr pane process-info` has no path, and pi's `process.title` hides `cli.js` from `ps`.
    Recommend: drop it and rely on `check`'s wrapper-on-PATH row (`whence -va`) and the wrapper's
    own refusals; then delete `pi_ade::process_prefix`, which nothing calls.
93. **Muse provider.** With item 65's id the row runs on `opencode-go` (the Go plan), not `opencode`
    (Zen), and doctor now wants an `opencode-go` login. SPEC-pi's recipe table and
    SPEC-jev-picker ("`opencode_muse` stays out of day one") disagree on whether the row ships
    day one. Decide: ship it, or keep the row and leave `opencode-go` out of the enabled logins.
94. **Guard after a server bounce.** With the pre-ADE route deleted, a pi lane whose tab env lost
    `HERDR_ADE_LAUNCH` shows only `blocked` in herdr; no `ha waiting` line. Item 64 is moot.
    Accept that, or have the guard find its launch another way (a file under the pi root).
95. **Jev launch mode.** Deleted in A4 (`5a268f2`) because this round ships off and shadow only.
    The round that turns it on adds it back with its own spec; confirm that is wanted rather than
    keeping the variant.
96. **Resolver-off reason text.** With the resolver off (every launch this round) the reason reads
    "…, the usual choice, because the picker did not answer.", though the picker was never asked.
    Keep it, reuse the "looks like ordinary work" template, or add "<job> runs on <plain>, the usual
    choice."?
97. **`role_default_unavailable` never fires.** Whole-table validation refuses first
    (`recipe_kind_unknown`), and one disabled recipe with an unknown kind refuses the whole table.
    Keep, or skip disabled rows in validation?
98. **Cap file name.** The daily cap file is named by epoch day (`20714.count`). Rename to
    `2026-09-18.count` so Rolf can read it?
99. **Model spellings the table cannot read.** After `e6cdaa8` the effort rule reads every arg,
    but the pair filter and the scrub list still read only `--model <v>` and `model=<v>`: a recipe
    spelled `-m <v>` or `--model=<v>` is never matched to its sibling in a dialogue. Recommend a
    load-time refusal of those spellings; it needs a new error code (`recipe_model_unreadable`),
    which is why I did not add it.
