# Plugin round r1: verdict (reviewer)

**MERGE-AFTER-DECISION.** The merged candidate builds, lints and tests clean. Every det, cli and mod part passes, and live rows 1, 2, 3, 6, 7 and 11 pass against a real Claude coordinator and two Claude lanes on the herdr 0.9.1 candidate.
Rows 4, 5, 8, 9, 10 and pi-T3 are FAIL only because a live part is NOT-RUN (Cursor, Pro, the restart drill, the reviewer thread, the hook-rewrite capture, the `~/.local/bin/pi` link). Each needs an install-day step or a decision below, not a code fix.
Decisions 77 to 88 are open; none blocks the merge except 77 (who starts the reviewer) and 80 (the DeepSeek row Rolf expects on day one).

Branch `review/plugin-r1`, final commit named in `.reports/review-plugin-r1-report.md`. A4 and A5 were reviewed package by package by reviewer-b (`code-plugin-r1-b.md`, kept as is); this file covers the merge, the wiring, A0 to A3, and the whole candidate's gates and acceptance.

## Gates (final commit)

| Gate | Result |
|---|---|
| `cargo +1.89.0 fmt --check` | exit 0 |
| `cargo +1.89.0 clippy --all-targets --locked -- -D warnings` | exit 0, 0 warnings, no `allow` left in `Cargo.toml` |
| `cargo +1.89.0 test --locked` | exit 0: 357 (herdr-ade bin) + 41 (herdr-pi bin) + 4 + 29 = 431 passed, 0 failed, 0 ignored |
| `cargo +1.89.0 build --release --locked` | exit 0 |
| `herdr-ade doctor` (throwaway env, `HERDR_BIN_PATH` = 0.9.1 candidate) | exit 1: 24 ok, 3 warn (gh auth, ticker not running, `pi` not found), 4 FAIL: wrapper on PATH and the three provider logins. All four are install-day steps. |
| `herdr-pi doctor` (throwaway prefix) | exit 1: every row ok except wrapper on PATH and the three logins |
| `herdr-pi check kimi-coding` | exit 1, `credentials_not_configured`, names `herdr-pi login kimi-coding` |
| `pi/guard-check.sh` (throwaway) | GUARD PASS |
| Acceptance, deterministic (2937fd8) | 45 det, cli and mod parts, all PASS; 9/19 counted rows, the rest fail only on live parts |
| Acceptance, live (283a361) | 13/19 counted rows PASS (below) |

The four pi logins in the brief are three now: Rolf deleted the direct DeepSeek row.

## Acceptance (SPEC-ADE §4.3, live run)

Live run at 283a361. The one later commit, 2937fd8, only deletes the Zen provider, and the deterministic run and every other gate are on 2937fd8.

Throwaway session `acc-<pid>` on `/home/agent/projects/herdr/.target/review/release/herdr` (0.9.1), started under `env -u CLAUDE_CODE_CHILD_SESSION` with the isolated XDG dirs. Every Claude pane runs `claude-haiku-4-5-20251001 --dangerously-skip-permissions`; the throwaway roles table pins Haiku for every Claude role. The session is stopped with the candidate's `session stop` and the ticker with `ticker stop` at the end of every run; no process was left.

| Row | Status | Reason |
|---|---|---|
| 1 ha new/open, claude coordinator, hook before launch, bootstrap acknowledged | PASS | live and mod |
| 2 round open, lanes admitted: tabs, parent, briefs committed first | PASS | two Claude lanes; no Cursor pane (brief) |
| 3 ha done seals events; peek does not acknowledge; waiting from a dirty tree | PASS | both lanes sealed a done event |
| 4 handoff continuity, blocked-then-submitted delivery | FAIL | det and mod PASS; live NOT-RUN: the 21-pane topology and restart drill need herdr server restart in C (lane/restart-core), and the live server is off limits |
| 5 round review: B before the review worktree; V; done --sha V | FAIL | det PASS, B and `review/r1` checked live; live NOT-RUN: nothing starts the reviewer thread that makes C and V (decision 77) |
| 6 round merge stop after merged; resume to H; items 32-34 | PASS | det and mod |
| 7 negative rows | PASS | det and mod |
| 8 pro-mcp, passive adopt, one TURN, artifact event, dialogue commit | FAIL | det PASS; live NOT-RUN: needs a logged-in ChatGPT Pro pane |
| 9 kind rows | FAIL | claude lane receipt PASS (read from row 2's lanes); cursor NOT-RUN: no Cursor pane (quota gone, retired after the port); codex, opencode, agy, dsh NOT-RUN: unqualified kinds |
| 10 plain-language rows | FAIL | every det, cli, mod part PASS; board tokens read back live PASS; hook-rewrite capture NOT-RUN: needs a hook barrier on the live coordinator to catch before/during/after |
| 11 throwaway server stop; doctor unreachable; records intact | PASS | live |
| pi-T1 herdr-pi doctor | PASS | cli |
| pi-T2 check refuses | PASS | mod |
| pi-T3 mock pi lane, no trust question | FAIL | live NOT-RUN: a `kind = "pi"` lane starts `pi` from the login PATH, which needs the `~/.local/bin/pi` link (install day) |
| pi-T7 guard 429/401/context error | PASS | cli |
| pi-T9 no global npm install | PASS | cli, mod |
| pi-T10 wrong pi earlier on PATH | PASS | mod |
| pi-T11 missing login refuses the start | PASS | cli, mod |
| pi-T13 no Cursor route | PASS | cli, mod |
| pi-T4, T5, T6a, T6b, T8, T12, T14 | NOT-RUN | no provider login in a throwaway; `/login` is Rolf's install-day step (not counted) |

## Defects (reviewer; reviewer-b's A4/A5 defects are in `code-plugin-r1-b.md`)

| Sev | Where (now) | What was wrong | What I changed | Commit |
|---|---|---|---|---|
| high | `src/lane.rs:233` `acknowledge_bootstrap` | `HERDR_ADE_LAUNCH` was parsed as JSON; A1 sets the slash form, so every receipt was refused | parse A1's form, compare attempt and brief hash | 1358e30 |
| high | `src/lane.rs`, `src/ops.rs`, `src/steps.rs` | the ADE attempt was read from A1's `launch_attempts` retry counter | `thread.attempt` everywhere | 1358e30 |
| high | `src/coordinator.rs:77` `open`, `src/ticker.rs:788` | coordinator launched on the removed `coordinator_agent` plus safety args, not the roles table | `resolve_role("coordinator")`, stored recipe, env on the tab | 1358e30 |
| med | `src/ticker.rs:617` `tick_cheap` | `prime_pending` cleared on transport, not on the D14 receipt | `prime_sent`; the receipt clears | 1358e30 |
| med | `src/lane.rs:198` `skill` | reviewer, critic, drafter, pickup skills refused | A3's skills wired | 1358e30 |
| med | `src/project.rs:149` `Settings.talk` | new projects wrote `talk = false`, killing item 24's default | `Option<bool>` | 1358e30 |
| low | `src/round.rs`, `src/checkpoint.rs` | tolerant TOML reads of A1's record | typed reads | 1358e30 |
| high | `src/steps.rs:58` `deliver_events` | one stale or unreadable event blocked every event sorted after it (A2 H2) | per-event delivery and recovery | 80505c7 |
| med | `src/steps.rs:58` | an event with a journal line could be typed again (A2 M7) | any journal line stops a retype | 80505c7 |
| med | `src/coordinator.rs:77`, ticker relaunch | open reset `launch_attempts` to 1, so an event sealed for a dead coordinator reached its replacement (A2 M3, X5) | monotonic coordinator generation | 80505c7 |
| med | `src/ops.rs:290` `tick` | X1 kept a reserved op whose lane attempt was superseded | abandoned | 80505c7 |
| high | `src/git.rs:181`, `src/threads.rs:136` | A1's `commit_file_from_parent` never moved the integration branch; the brief was not a commit on it (A1 H3, H4) | one D9 commit `commit_files_locked`; brief committed under the repository lock before `worktree add`; detached or remote base refused | 17653f8 |
| high | `src/threads.rs:291` `place_and_brief` | the record's brief hash was overwritten with the local copy's hash, so receipts could not match (A1 H1) | the hash in `HERDR_ADE_LAUNCH` stays | 17653f8 |
| med | `src/git.rs:181` | a modified tracked HANDOFF.md in the integration checkout was overwritten (A3 M5) | refused unless only untracked copies of the committed files are dirty | 17653f8 |
| low | `src/round.rs:126` `diff_names` | rename detection hid a deleted path from `verdict_scope` (A3 L4) | `--no-renames` | 17653f8 |
| med | `src/herdr.rs:262` `call` | silent verbs (report-metadata, pane run) read as failures (item 71) | empty reply with exit 0 is success | 17653f8 |
| high | `src/threads.rs:136` | a launched lane was primed with the brief path, not the role skill (A1 H2) | `Run <prefix> skill <role>, then read tasks/<id>.md` | 9ddffb0 |
| med | `src/threads.rs` restart | restart reused a pane whose `HERDR_ADE_LAUNCH` named the old attempt (A1 M5) | bare tab closed; new attempt gets its own tab | 9ddffb0 |
| med | `src/cli.rs` adopt-workspace | adopted without the D17 birth sentence | takes and checks `--plain` | 9ddffb0 |
| med | `src/project.rs:498` `resolve_role` | two roles tables (A1 `[roles]` and A4's picker) | one table: the picker's | 7efdeab |
| med | `src/threads.rs:136`, ticker | a `kind = "pi"` launch did not check the provider login (T11) | `pi_ade::check_with` before the tab; the ticker fails the thread | 7efdeab |
| high | `src/dialogue.rs:29`, `src/launch.rs:789` | dialogue start never checked the pair against the picker (A3 item 13) | `PickerPair` drops the drafter's model and pins the critic's recipe | ae1a419 |
| high | `src/hook.rs:549` `publish` | the hook read `asks/<id>.toml`; asks live at `asks/<id>/r<rev>.toml`, so every envelope for a real ask was refused | publishes through `ask::publish_keyed`, checks with `ask::open_revision` | 1733db3 |
| med | `src/hook.rs` | the hook kept its own `publications.jsonl` (no reader) and its own glossary | deleted; talk request marked accepted | 1733db3 |
| med | `src/steps.rs:105`, nudge | outbox and nudge typed without the talk writer lock (A2 M1, writer half) | both take `talk::writer_lock`, back off when suspended | 1733db3 |
| med | `src/coordinator.rs:266` | open never made the talk tab; context listed no open questions | `talk_tab` on both open paths; "Open questions" section | 1733db3 |
| med | `plain/vocabulary.txt`, `src/plain.rs:340` | items 68, 69, 73: round words missing, fixed-text bypass, possessives refused | words added, bypass gone, trailing `'s` stripped | 7369dcb |
| low | `src/thread.rs:236` | lane skills wrote a command prefix the lane could not see | "Commands:" line first; `hp` throughout | b0b0a8a |
| high | `src/round.rs:1456` `tick` | an unreadable events folder unpinned every member; a merging round was refreshed (A3 H1) | skip on error and for a round with a merge record | 272d26d |
| med | `src/events.rs:55` | a sealed event could be seen half written (A3 M1) | tmp file then `hard_link` | 272d26d |
| med | `src/round.rs:1020` `merge` | two merges of one round could run at once (A3 M2) | flock on `rounds/<r>/merge.lock` | 272d26d |
| med | `src/talk.rs:316` | talk typed before journaling uncertain (A3 M3) | uncertain, then submitted or queued | 272d26d |
| med | `src/talk.rs:551` | a digit under a redrawn ask went nowhere (A3 M4) | `ask_redrawn` notice | 272d26d |
| med | `src/board.rs:72` | board values could carry a registry name (A3 M6) | refused | 272d26d |
| med | `src/hook.rs:390` `turn_key` | a new human turn reused the last turn's budget (A2 H3) | fresh key on `stop_hook_active = false` | 272d26d |
| low | `src/hook.rs:22`, `src/coordinator.rs:313`, `src/lane.rs:19` | unbounded read; `--peek` acknowledged; report path with spaces (A2 M4, L2, L5, L7) | 4 MiB limit through the budget; peek reads only; `report_path_invalid` | 272d26d |
| high | `src/plain.rs:340` `check_r4` | tokens with a hyphen or non-ASCII skipped the word list (A0 H1) | split on hyphens and dashes; every part checked | 749e40e |
| high | `src/plain.rs:535` `trim_punct` | wrapping a word in unusual punctuation hid it (A0 H2) | trims every non-alphanumeric | 749e40e |
| med | `src/plain.rs:254`, `:77` | R1 matched names in one case; an empty name panicked (A0 M3, M4) | case-insensitive, empty names skipped | 749e40e |
| low | `src/plain.rs:589` | a long plain number read as a hex id (A0 L11) | all-decimal runs pass | 749e40e |
| high | `src/threads.rs:1066` `removal_gate` | a worktree could be removed under a lane that had not released it (A1 H5) | sealed done or resolved; no foreground process; own tab | 79329a1 |
| med | `src/herdr.rs:222`, `src/thread.rs:515` | identity bound the first process, not the agent's (A1 M2, M3) | `identities()`, verified against every foreground process | 79329a1 |
| med | `src/threads.rs:1128` `tick` | a pane with no stored identity was reparented; lineage items repeated each tick (A1 M4) | never reparented; one item per process | 79329a1 |
| med | `src/steps.rs:58`, `src/inbox.rs:250` | an event typed to a replaced coordinator was lost; inbox done moved items before checking all (A2 M2) | `recipient-changed` item; predecessor's items handled; check-all first | e2eba03 |
| low | `src/doctor.rs:70` | doctor printed part of the D1 tuple (reading 38) | ticker lock, startup, action ids, log hint | 839b331 |
| low | `src/project.rs:164` | a PROJECT.md role override with an unknown key was accepted (A1 M6) | `deny_unknown_fields` | b218187 |
| low | `src/round.rs:720`, `:1201` | two `expect` calls in production code | `round_not_complete`, `checkpoint_missing` errors | caad274 |
| med | `scripts/acceptance/run` | mod patterns named tests that do not exist; wrong hook file; Claude's folder dialog blocked panes; tab threads without `--repo`; an empty glob passed row 3 | real names, `settings.local.json`, the dialog answered in throwaway panes, `--repo`, real waits | 8f73e9a, 98966c5, 40817bf, 283a361 |

No test was added without a row above: every new or extended test names the defect in its row.

## Wiring (step 3)

- ticker slow pass: `crate::ops::tick` and `crate::round::tick` outside the project lock (item 57). 80505c7
- A4's tick entry: the ticker publishes `ade_last` from the stored compact reason at launch. 7efdeab
- `thread start`: `resolve_launch` before any worktree or tab, no project lock, `--role` and `--recipe`; `brief_hash` from the brief commit (item 48). 7efdeab, 17653f8
- doctor prints A4's picker rows and A5's pi rows; `picker-doctor` verb deleted (item 50). 7efdeab
- `pi_ade::check_with(provider)` before every `kind = "pi"` launch; `process_prefix` deleted (item 92). 7efdeab
- A2's adapter table takes the pi row. 7efdeab
- A4's picker table takes A5's pi rows; the withheld list later deleted with Grok. 7efdeab, 98966c5
- `Herdr::call` treats silent success as success (item 71). 17653f8
- coordinator open, outbox writer and correction hook use A3's talk and ask (A3 lines 7 to 11). 1733db3
- `dialogue start` uses A4's picker as the pair filter. ae1a419
- `herdr-plugin.toml`: pi-doctor, pi-setup, pi-login. 7efdeab
- `.git/info/exclude` gets `.worktrees/` and `.herdr-project/` on `thread start`. 17653f8
- `.state/capabilities/` markers: not written. No row was authenticated for a capability qualification, so the markers stay absent (A2 line 7).

Readings applied: 36 (60451da), 37 (7efdeab, 9ddffb0), 38 (839b331), 39 (verified: `$XDG_CONFIG_HOME/herdr-ade`, doctor prints it), 47 (reviewer-b's a8c2504, kept), 55 (17653f8), 61 (`zsh -lic 'whence -va pi'`, verified), 68, 69, 73 (7369dcb), 76 (the guard exists: `forward_lanes` skips a worktree whose head is not the pinned sha and says so in the merge output, `src/round.rs:1363`).

## Items 58 and 65

- **58: reading (a), Claude only.** Real `claude` panes ran as Rolf in the throwaway session, all on Haiku, so no separate throwaway login was needed. Hook install and bootstrap ran for real (rows 1 to 3). The hook-rewrite capture is still NOT-RUN (row 10). Cursor stays unqualified. Claude itself records folder trust for the `/var/tmp/ade-review` paths in `~/.claude.json`; nothing of mine wrote there.
- **65: the OpenCode Go id `muse-spark-1.3-contributor`**, provider `opencode-go`, enabled on day one (item 93, Rolf's choice b).

## Install day, in order

1. `herdr-pi setup` (or the plugin action "Pi: install the pinned pi"): pins pi 0.85.1 into `~/.herdr-ade/pi/npm` and writes the settings, guard and wrapper.
2. `ln -s ~/.herdr-ade/pi/bin/pi ~/.local/bin/pi` so `zsh -lic 'whence -va pi'` finds the wrapper first.
3. The three logins, each `herdr-pi login <provider>` then `/login` inside pi: `openai-codex`, `opencode-go`, `kimi-coding`.
4. `scripts/migration/swap-binary.sh` to move the live server to the 0.9.1 build.
5. The correction hook: `ha open <project>` installs it into the project's own `.claude/settings.local.json` before the coordinator starts. The brief says `settings.json`; the code writes the local file, which stays out of the project's git.
6. Claude's first-run "trust this folder" question, once per project folder not already under a trusted parent (decision 82).
7. `herdr-pi doctor` and `ha doctor`: every row ok except `gh auth` if it is not wanted.

## Needs a decision

77. **Who starts the reviewer.** `round review` commits B, makes `review/r<n>` and prints "start the reviewer thread in that worktree", but no verb starts a role in an existing worktree: `thread start --role reviewer` makes its own. Reading: `round review` starts the reviewer lane itself in the worktree it just made, with the D14 receipt, in the next round. Until then row 5 cannot pass live.
78. **Remote threads.** After 9ddffb0 a remote thread (`--machine`) launches with no parent, from another server, and no acceptance row covers it. Reading: delete remote threads under the no-compat rule unless Rolf still uses them.
79. **A role missing from the table.** `thread start --role x` with no `[roles.x]` is refused `role_unknown`. D2 says an unknown role gets the default. Reading: keep the refusal, since a typo should not launch.
80. **DeepSeek through OpenCode Go.** Rolf said DeepSeek runs only through `opencode-go`, but no such row exists and I found no model id. Reading: add `pi_opencode_deepseek` once Rolf names the id from `opencode models`.
81. **The OpenCode Zen provider.** It had no row and no day-one login, so I deleted it (2937fd8). Reading: stays deleted; a Zen row later brings it back with its login.
82. **Claude's folder-trust question** blocks a new pane until answered. It only appears for a folder not under a trusted parent. The ADE cannot answer it without reading the screen or writing `~/.claude.json`. Reading: no code. Rolf's repos under `~/projects` are covered once `~/projects` is trusted; the acceptance script answers it only in throwaway panes.
83. **Composer check before typing (A2 M1).** The writer lock is in (1733db3). D8 also asks for an empty-composer check, which needs `agent read`, and the brief forbids screen reads. Reading: keep the writer lock only; add the composer check when herdr offers a composer state that is not a screen read.
84. **Hook session binding (A2 M5).** The first stop from any process with the coordinator's pane id claims the session. A `/clear` later turns the hook into a silent no-op. Reading: bind at the bootstrap receipt, and raise an inbox item on a session change. Next round.
85. **Helper pid reuse (A2 M6).** Liveness is `kill -0` with no start time, so a reused pid keeps a reserved op alive. Reading: store the process start time next to the pid. Next round.
86. **A3 lows left.** L3: a crash between the dialogue commit and the record save pins the turn. L5: the ask marker needs both channels, so a failing board re-notifies each tick. Reading: fix both next round.
87. **A0 lows left.** L9: no contraction passes R4 (the word list has none). L10: R5 splits on every `.`, so `e.g.` undercounts. L12: `Requested` is untagged. Reading: L9 add the common contractions to `vocabulary.txt`; L10 and L12 next round.
88. **Unreadable records vanish.** `thread::list` drops a record that does not parse, with no message (19 callers). The nested contract structs have no container `#[serde(default)]` (A0 M5). Reading: no defaults under the no-compat rule, but doctor gets a row naming every unreadable record. Next round.

## Deleted

- `skill/THREAD.md`; legacy `threads::start`, the `Option<AdeStart>` split and `thread start --agent`; legacy local placement (`herdr worktree create`, `place_tab`, `worktree open` for local threads, the local `git worktree remove` fallback); `Thread::is_ade`. 9ddffb0
- `Settings.coordinator_agent`, `thread_agent`, `Safety.coordinator_agent_args`, `thread_agent_args`; the ticker's legacy launch; the coordinator and ticker pre-receipt leniencies. 9ddffb0
- Dead seams: `inbox::done` wrapper, `adapters::may_retry_bootstrap`, `git::is_ancestor`, `git::exclude_plugin_paths`, herdr `tab_create`/`agent_start`, `ticker::Ticker`, round `Verdict.gates`, `contracts::ThreadRecord`, every `#[allow(dead_code)]`, the FakeRunner `type_complexity` allow, the lane merge markers; the legacy start scenario and the smoke tests for these seams. 9ddffb0
- A1's `[roles]` loader, `contracts::RolesTable`, `contracts::LaunchRecipe`; the `picker-doctor` verb and `launch::picker_doctor`; the D2 permission-flag warning; `pi_ade::process_prefix`, `pi::check`, `pi::priming`; `Cargo.toml`'s clippy allows; pi_ade's copy of A4's reason templates. 7efdeab
- A3's `herdr_quiet`; A1's `commit_file_from_parent`. 17653f8
- The hook's `Publication`, `publications.jsonl`, `FIXED_FAILURE_NOTICE` and its own glossary. 1733db3
- `dialogue::AnyPair` from production (test only now). ae1a419
- The direct DeepSeek row `pi_deepseek_flash`, the `deepseek` provider, its login row and clamps; every Grok route: `withheld_recipes`, the `recipe_withheld` check, the `xai` provider and login row, the `grok-4.6` clamp, the Grok lines in PI.md. 98966c5
- The OpenCode Zen provider `opencode` and its login step. 2937fd8
- reviewer-b's deletions in A4 and A5 (jev launch mode, aliases, compat paths, smoke tests) are listed in `code-plugin-r1-b.md`.
